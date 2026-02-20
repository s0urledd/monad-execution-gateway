use std::collections::{HashMap, VecDeque};

use alloy_primitives::B256;
use serde::{Deserialize, Serialize};

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Block Commit Stage — Monad's official block state model
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Public block commit stages aligned with Monad's consensus model.
///
/// Happy path:  Proposed → Voted → Finalized → Verified
///
/// Mapping from internal execution events:
///   BlockStart     → Proposed  (block proposed for execution)
///   BlockQC        → Voted     (quorum certificate received)
///   BlockFinalized → Finalized (committed to canonical chain)
///   BlockVerified  → Verified  (state verification complete, terminal)
///   BlockReject    → Rejected  (dropped at any point, terminal)
///
/// Internal events like BlockEnd / BlockPerfEvm* update execution
/// metadata but do NOT advance the public commit stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "PascalCase")]
pub enum BlockStage {
    Proposed = 0,
    Voted = 1,
    Finalized = 2,
    Verified = 3,
    Rejected = 4,
}

impl BlockStage {
    /// Terminal stages — no further transitions expected.
    pub fn is_terminal(&self) -> bool {
        matches!(self, BlockStage::Verified | BlockStage::Rejected)
    }

    /// Returns true if transitioning from `self` to `next` is valid.
    /// Stages must advance monotonically; Rejected can happen from any stage.
    pub fn can_transition_to(&self, next: BlockStage) -> bool {
        if next == BlockStage::Rejected {
            return true; // Rejection is always valid
        }
        next > *self
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Per-Block Lifecycle
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Full lifecycle state for a single block, keyed by block_hash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockLifecycle {
    /// Consensus block ID (proposal hash) — primary key.
    pub block_hash: B256,
    pub block_number: u64,
    pub current_stage: BlockStage,

    /// Nanosecond timestamps for each public stage reached.
    pub stage_timestamps: HashMap<BlockStage, u64>,

    // ─── Execution metadata (updated by internal events) ─────────
    pub txn_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gas_used: Option<u64>,
    /// Ethereum-style block hash (available after BlockEnd).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eth_block_hash: Option<B256>,
    /// Nanosecond timestamp when execution finished (BlockEnd).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_end_ns: Option<u64>,
}

impl BlockLifecycle {
    fn new(block_hash: B256, block_number: u64, timestamp_ns: u64) -> Self {
        let mut stage_timestamps = HashMap::new();
        stage_timestamps.insert(BlockStage::Proposed, timestamp_ns);
        Self {
            block_hash,
            block_number,
            current_stage: BlockStage::Proposed,
            stage_timestamps,
            txn_count: 0,
            gas_used: None,
            eth_block_hash: None,
            execution_end_ns: None,
        }
    }

    /// Duration (ms) from Proposed to a given stage.
    pub fn time_to_stage_ms(&self, stage: BlockStage) -> Option<f64> {
        let start = self.stage_timestamps.get(&BlockStage::Proposed)?;
        let target = self.stage_timestamps.get(&stage)?;
        Some((*target as f64 - *start as f64) / 1_000_000.0)
    }

    /// Duration (ms) spent in a specific stage before transitioning to the next.
    pub fn duration_in_stage_ms(&self, stage: BlockStage) -> Option<f64> {
        let entered = self.stage_timestamps.get(&stage)?;
        let next_ts = self
            .stage_timestamps
            .iter()
            .filter(|(&s, _)| s > stage && s != BlockStage::Rejected)
            .map(|(_, &ts)| ts)
            .min();
        next_ts.map(|ts| (ts as f64 - *entered as f64) / 1_000_000.0)
    }

    /// Total lifecycle duration from Proposed to current stage (ms).
    pub fn total_age_ms(&self) -> Option<f64> {
        let start = self.stage_timestamps.get(&BlockStage::Proposed)?;
        let current = self.stage_timestamps.get(&self.current_stage)?;
        Some((*current as f64 - *start as f64) / 1_000_000.0)
    }

    /// Execution time (ms) — time from Proposed to execution end (BlockEnd).
    pub fn execution_time_ms(&self) -> Option<f64> {
        let start = self.stage_timestamps.get(&BlockStage::Proposed)?;
        let end = self.execution_end_ns?;
        Some((end as f64 - *start as f64) / 1_000_000.0)
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Lifecycle Update (emitted to WebSocket clients)
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Emitted every time a block transitions to a new public commit stage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockLifecycleUpdate {
    pub block_hash: B256,
    pub block_number: u64,
    /// Previous stage (`None` for the initial `Proposed` transition).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_stage: Option<BlockStage>,
    pub to_stage: BlockStage,
    /// How long the block spent in the *previous* stage (ms).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_in_previous_stage_ms: Option<f64>,
    /// Total elapsed time since Proposed (ms).
    pub block_age_ms: f64,
    pub txn_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gas_used: Option<u64>,
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Lifecycle Summary (for REST responses)
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Compact summary of a block's lifecycle for REST endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockLifecycleSummary {
    pub block_hash: B256,
    pub block_number: u64,
    pub current_stage: BlockStage,
    pub txn_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gas_used: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eth_block_hash: Option<B256>,
    /// Milliseconds from Proposed → each public stage reached.
    pub stage_timings_ms: HashMap<BlockStage, f64>,
    /// Execution time (ms) — Proposed to BlockEnd.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_time_ms: Option<f64>,
    /// Total age from Proposed to current stage (ms).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_age_ms: Option<f64>,
}

impl From<&BlockLifecycle> for BlockLifecycleSummary {
    fn from(lc: &BlockLifecycle) -> Self {
        let start_ns = lc
            .stage_timestamps
            .get(&BlockStage::Proposed)
            .copied()
            .unwrap_or(0);
        let stage_timings_ms: HashMap<BlockStage, f64> = lc
            .stage_timestamps
            .iter()
            .filter(|(&stage, _)| stage != BlockStage::Proposed)
            .map(|(&stage, &ts)| (stage, (ts as f64 - start_ns as f64) / 1_000_000.0))
            .collect();

        Self {
            block_hash: lc.block_hash,
            block_number: lc.block_number,
            current_stage: lc.current_stage,
            txn_count: lc.txn_count,
            gas_used: lc.gas_used,
            eth_block_hash: lc.eth_block_hash,
            stage_timings_ms,
            execution_time_ms: lc.execution_time_ms(),
            total_age_ms: lc.total_age_ms(),
        }
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Lifecycle Tracker
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Tracks block lifecycle transitions across all active blocks.
///
/// Primary key is `block_hash` (B256) for reorg safety.
/// A secondary index maps `block_number → block_hash` for events
/// that only carry a block number (e.g. BlockVerified).
pub struct BlockLifecycleTracker {
    /// Active (non-terminal) blocks keyed by block_hash.
    active_blocks: HashMap<B256, BlockLifecycle>,
    /// block_number → block_hash lookup for events without a hash.
    number_to_hash: HashMap<u64, B256>,
    /// Recently completed (Verified/Rejected) blocks.
    completed_blocks: VecDeque<BlockLifecycle>,
    /// Max completed blocks to retain.
    max_completed: usize,
}

impl BlockLifecycleTracker {
    pub fn new() -> Self {
        Self {
            active_blocks: HashMap::new(),
            number_to_hash: HashMap::new(),
            completed_blocks: VecDeque::new(),
            max_completed: 128,
        }
    }

    /// Resolve a block_number to the tracked block_hash.
    pub fn resolve_hash(&self, block_number: u64) -> Option<B256> {
        self.number_to_hash.get(&block_number).copied()
    }

    /// Get the current commit stage for a block (by hash).
    pub fn current_stage(&self, block_hash: &B256) -> Option<BlockStage> {
        self.active_blocks
            .get(block_hash)
            .map(|lc| lc.current_stage)
            .or_else(|| {
                self.completed_blocks
                    .iter()
                    .find(|lc| lc.block_hash == *block_hash)
                    .map(|lc| lc.current_stage)
            })
    }

    /// Get the current commit stage by block_number.
    pub fn current_stage_by_number(&self, block_number: u64) -> Option<BlockStage> {
        let hash = self.resolve_hash(block_number)?;
        self.current_stage(&hash)
    }

    // ─── Event handlers ──────────────────────────────────────────────

    /// BlockStart → Proposed.
    ///
    /// Creates a new lifecycle entry. Returns the initial lifecycle update.
    pub fn on_block_proposed(
        &mut self,
        block_hash: B256,
        block_number: u64,
        timestamp_ns: u64,
    ) -> BlockLifecycleUpdate {
        let lifecycle = BlockLifecycle::new(block_hash, block_number, timestamp_ns);
        let update = BlockLifecycleUpdate {
            block_hash,
            block_number,
            from_stage: None,
            to_stage: BlockStage::Proposed,
            time_in_previous_stage_ms: None,
            block_age_ms: 0.0,
            txn_count: 0,
            gas_used: None,
        };
        self.active_blocks.insert(block_hash, lifecycle);
        self.number_to_hash.insert(block_number, block_hash);
        update
    }

    /// BlockEnd — internal signal. Updates execution metadata but does NOT
    /// advance the public commit stage.
    pub fn on_execution_end(
        &mut self,
        block_number: u64,
        timestamp_ns: u64,
        gas_used: u64,
        eth_block_hash: B256,
    ) {
        let Some(hash) = self.number_to_hash.get(&block_number).copied() else {
            return;
        };
        if let Some(lc) = self.active_blocks.get_mut(&hash) {
            lc.execution_end_ns = Some(timestamp_ns);
            lc.gas_used = Some(gas_used);
            lc.eth_block_hash = Some(eth_block_hash);
        }
    }

    /// Advance a block to a new public commit stage.
    ///
    /// Enforces **monotonic** progression: rejects backward transitions.
    /// Returns `None` if the block is unknown or the transition is invalid.
    pub fn advance_stage(
        &mut self,
        block_hash: B256,
        block_number: u64,
        new_stage: BlockStage,
        timestamp_ns: u64,
    ) -> Option<BlockLifecycleUpdate> {
        // Try to find by hash first, fall back to number lookup
        let hash = if self.active_blocks.contains_key(&block_hash) {
            block_hash
        } else if let Some(h) = self.number_to_hash.get(&block_number).copied() {
            h
        } else {
            return None;
        };

        let lifecycle = self.active_blocks.get_mut(&hash)?;

        // Enforce monotonic progression
        if !lifecycle.current_stage.can_transition_to(new_stage) {
            return None;
        }

        let from_stage = lifecycle.current_stage;
        let time_in_previous_stage_ms = lifecycle.duration_in_stage_ms(from_stage);

        // Record the transition
        lifecycle.stage_timestamps.insert(new_stage, timestamp_ns);
        lifecycle.current_stage = new_stage;

        let block_age_ms = lifecycle.total_age_ms().unwrap_or(0.0);

        let update = BlockLifecycleUpdate {
            block_hash: lifecycle.block_hash,
            block_number: lifecycle.block_number,
            from_stage: Some(from_stage),
            to_stage: new_stage,
            time_in_previous_stage_ms,
            block_age_ms,
            txn_count: lifecycle.txn_count,
            gas_used: lifecycle.gas_used,
        };

        // If terminal, move to completed
        if new_stage.is_terminal() {
            if let Some(completed) = self.active_blocks.remove(&hash) {
                self.completed_blocks.push_back(completed);
                while self.completed_blocks.len() > self.max_completed {
                    if let Some(old) = self.completed_blocks.pop_front() {
                        self.number_to_hash.remove(&old.block_number);
                    }
                }
            }
        }

        Some(update)
    }

    /// Increment the transaction count for a block (by number).
    pub fn record_txn(&mut self, block_number: u64) {
        let Some(hash) = self.number_to_hash.get(&block_number).copied() else {
            return;
        };
        if let Some(lc) = self.active_blocks.get_mut(&hash) {
            lc.txn_count += 1;
        }
    }

    // ─── REST query helpers ──────────────────────────────────────────

    /// Get lifecycle summaries for all recent blocks (active + completed).
    pub fn get_all_lifecycles(&self) -> Vec<BlockLifecycleSummary> {
        let mut result: Vec<BlockLifecycleSummary> = self
            .completed_blocks
            .iter()
            .map(BlockLifecycleSummary::from)
            .collect();

        let mut active: Vec<BlockLifecycleSummary> = self
            .active_blocks
            .values()
            .map(BlockLifecycleSummary::from)
            .collect();
        active.sort_by_key(|s| s.block_number);

        result.extend(active);
        result
    }

    /// Get the full lifecycle for a specific block by hash.
    pub fn get_lifecycle_by_hash(&self, block_hash: &B256) -> Option<BlockLifecycleSummary> {
        if let Some(lc) = self.active_blocks.get(block_hash) {
            return Some(BlockLifecycleSummary::from(lc));
        }
        self.completed_blocks
            .iter()
            .find(|lc| lc.block_hash == *block_hash)
            .map(BlockLifecycleSummary::from)
    }

    /// Get the full lifecycle for a specific block by number.
    pub fn get_lifecycle_by_number(&self, block_number: u64) -> Option<BlockLifecycleSummary> {
        let hash = self.number_to_hash.get(&block_number)?;
        self.get_lifecycle_by_hash(hash)
    }

    /// Get current state of all active (non-terminal) blocks as lifecycle
    /// updates. Used for replay on WebSocket connect so new clients see
    /// in-progress blocks immediately.
    pub fn get_active_updates(&self) -> Vec<BlockLifecycleUpdate> {
        let mut updates: Vec<BlockLifecycleUpdate> = self
            .active_blocks
            .values()
            .map(|lc| BlockLifecycleUpdate {
                block_hash: lc.block_hash,
                block_number: lc.block_number,
                from_stage: None,
                to_stage: lc.current_stage,
                time_in_previous_stage_ms: None,
                block_age_ms: lc.total_age_ms().unwrap_or(0.0),
                txn_count: lc.txn_count,
                gas_used: lc.gas_used,
            })
            .collect();
        updates.sort_by_key(|u| u.block_number);
        updates
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Tests
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[cfg(test)]
mod tests {
    use super::*;

    fn test_hash(n: u8) -> B256 {
        let mut bytes = [0u8; 32];
        bytes[31] = n;
        B256::from(bytes)
    }

    #[test]
    fn test_happy_path_lifecycle() {
        let mut tracker = BlockLifecycleTracker::new();
        let hash = test_hash(1);

        // BlockStart → Proposed at t=0
        let update = tracker.on_block_proposed(hash, 100, 0);
        assert_eq!(update.to_stage, BlockStage::Proposed);
        assert!(update.from_stage.is_none());
        assert_eq!(update.block_hash, hash);

        // 3 transactions
        tracker.record_txn(100);
        tracker.record_txn(100);
        tracker.record_txn(100);

        // BlockEnd → internal (no public stage change)
        let eth_hash = test_hash(0xEE);
        tracker.on_execution_end(100, 50_000_000, 21000, eth_hash);

        // Verify still Proposed
        assert_eq!(tracker.current_stage(&hash), Some(BlockStage::Proposed));

        // BlockQC → Voted at t=100ms
        let update = tracker
            .advance_stage(hash, 100, BlockStage::Voted, 100_000_000)
            .unwrap();
        assert_eq!(update.from_stage, Some(BlockStage::Proposed));
        assert_eq!(update.to_stage, BlockStage::Voted);
        assert_eq!(update.txn_count, 3);
        assert_eq!(update.gas_used, Some(21000));
        assert!((update.block_age_ms - 100.0).abs() < 0.001);

        // BlockFinalized → Finalized at t=200ms
        let update = tracker
            .advance_stage(hash, 100, BlockStage::Finalized, 200_000_000)
            .unwrap();
        assert_eq!(update.to_stage, BlockStage::Finalized);
        assert!((update.block_age_ms - 200.0).abs() < 0.001);
        // Time in Voted stage: 200ms - 100ms = 100ms
        assert!((update.time_in_previous_stage_ms.unwrap() - 100.0).abs() < 0.001);

        // Still active
        assert!(tracker.active_blocks.contains_key(&hash));

        // BlockVerified → Verified at t=300ms (terminal)
        let update = tracker
            .advance_stage(hash, 100, BlockStage::Verified, 300_000_000)
            .unwrap();
        assert_eq!(update.to_stage, BlockStage::Verified);
        assert!((update.block_age_ms - 300.0).abs() < 0.001);

        // Moved to completed
        assert!(!tracker.active_blocks.contains_key(&hash));
        assert_eq!(tracker.completed_blocks.len(), 1);

        // REST query still works
        let summary = tracker.get_lifecycle_by_hash(&hash).unwrap();
        assert_eq!(summary.current_stage, BlockStage::Verified);
        assert_eq!(summary.eth_block_hash, Some(eth_hash));
        assert!(summary.execution_time_ms.unwrap() > 0.0);
    }

    #[test]
    fn test_monotonic_enforcement() {
        let mut tracker = BlockLifecycleTracker::new();
        let hash = test_hash(2);

        tracker.on_block_proposed(hash, 200, 0);
        tracker
            .advance_stage(hash, 200, BlockStage::Voted, 100_000_000)
            .unwrap();

        // Try to go backward: Voted → Proposed — should be rejected
        let result = tracker.advance_stage(hash, 200, BlockStage::Proposed, 200_000_000);
        assert!(result.is_none());

        // Current stage should still be Voted
        assert_eq!(tracker.current_stage(&hash), Some(BlockStage::Voted));
    }

    #[test]
    fn test_rejected_from_any_stage() {
        let mut tracker = BlockLifecycleTracker::new();
        let hash = test_hash(3);

        tracker.on_block_proposed(hash, 300, 0);

        // Reject directly from Proposed
        let update = tracker
            .advance_stage(hash, 300, BlockStage::Rejected, 10_000_000)
            .unwrap();
        assert_eq!(update.to_stage, BlockStage::Rejected);

        // Terminal — moved to completed
        assert!(!tracker.active_blocks.contains_key(&hash));
        assert_eq!(tracker.completed_blocks.len(), 1);
    }

    #[test]
    fn test_hash_based_keying() {
        let mut tracker = BlockLifecycleTracker::new();
        let hash = test_hash(4);

        tracker.on_block_proposed(hash, 400, 0);

        // Lookup by number should resolve to hash
        assert_eq!(tracker.resolve_hash(400), Some(hash));

        // Stage query by number
        assert_eq!(
            tracker.current_stage_by_number(400),
            Some(BlockStage::Proposed)
        );

        // Advance using B256::ZERO (unknown hash) but with known block_number
        let update = tracker
            .advance_stage(B256::ZERO, 400, BlockStage::Voted, 100_000_000)
            .unwrap();
        assert_eq!(update.block_hash, hash); // Resolved to correct hash
        assert_eq!(update.to_stage, BlockStage::Voted);
    }

    #[test]
    fn test_unknown_block_returns_none() {
        let mut tracker = BlockLifecycleTracker::new();
        let result = tracker.advance_stage(test_hash(99), 999, BlockStage::Voted, 0);
        assert!(result.is_none());
    }

    #[test]
    fn test_completed_blocks_bounded() {
        let mut tracker = BlockLifecycleTracker::new();
        tracker.max_completed = 3;

        for i in 0u8..5 {
            let hash = test_hash(i);
            tracker.on_block_proposed(hash, i as u64, i as u64 * 1_000_000);
            tracker.advance_stage(hash, i as u64, BlockStage::Verified, (i as u64 + 1) * 1_000_000);
        }

        // Only last 3 retained
        assert_eq!(tracker.completed_blocks.len(), 3);
        assert_eq!(tracker.completed_blocks[0].block_number, 2);
        assert_eq!(tracker.completed_blocks[2].block_number, 4);
    }

    #[test]
    fn test_execution_end_metadata() {
        let mut tracker = BlockLifecycleTracker::new();
        let hash = test_hash(5);
        let eth_hash = test_hash(0xAA);

        tracker.on_block_proposed(hash, 500, 0);
        tracker.record_txn(500);

        // BlockEnd updates metadata
        tracker.on_execution_end(500, 30_000_000, 50000, eth_hash);

        let summary = tracker.get_lifecycle_by_number(500).unwrap();
        assert_eq!(summary.gas_used, Some(50000));
        assert_eq!(summary.eth_block_hash, Some(eth_hash));
        assert!((summary.execution_time_ms.unwrap() - 30.0).abs() < 0.001);
        // Public stage is still Proposed (BlockEnd is internal)
        assert_eq!(summary.current_stage, BlockStage::Proposed);
    }

    #[test]
    fn test_stage_ordering() {
        assert!(BlockStage::Proposed < BlockStage::Voted);
        assert!(BlockStage::Voted < BlockStage::Finalized);
        assert!(BlockStage::Finalized < BlockStage::Verified);
    }

    #[test]
    fn test_skip_stage_allowed() {
        // It's valid to jump from Proposed → Finalized (skipping Voted)
        // This can happen if the gateway starts mid-stream
        let mut tracker = BlockLifecycleTracker::new();
        let hash = test_hash(6);

        tracker.on_block_proposed(hash, 600, 0);
        let update = tracker
            .advance_stage(hash, 600, BlockStage::Finalized, 100_000_000)
            .unwrap();
        assert_eq!(update.from_stage, Some(BlockStage::Proposed));
        assert_eq!(update.to_stage, BlockStage::Finalized);
    }
}
