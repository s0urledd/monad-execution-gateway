/**
 * Example: Monitor parallel execution efficiency and state contention
 *
 * Uses the /v1/ws/contention channel to receive only contention analytics.
 *
 * Shows per-block metrics:
 * - Parallel efficiency (% of execution time saved by parallel execution)
 * - Contended storage slots
 * - Most contended contracts
 * - Contract co-access patterns
 *
 * Usage:
 *   npx ts-node examples/monitor-contention.ts ws://your-validator:8443
 */

import { GatewayClient } from "../sdk/typescript/src";

const url = process.argv[2] || "ws://127.0.0.1:8443";
const client = new GatewayClient({ url, channel: "contention" });

client.on("contention", (data) => {
  console.log(`\n═══ Block ${data.block_number} ═══`);
  console.log(
    `  Parallel efficiency: ${data.parallel_efficiency_pct.toFixed(1)}%`
  );
  console.log(
    `  Wall time: ${(data.block_wall_time_ns / 1e6).toFixed(2)}ms | Total TX time: ${(data.total_tx_time_ns / 1e6).toFixed(2)}ms`
  );
  console.log(
    `  Contention: ${data.contended_slot_count}/${data.total_unique_slots} slots (${(data.contention_ratio * 100).toFixed(2)}%)`
  );
  console.log(`  Transactions: ${data.total_txn_count}`);

  if (data.top_contended_contracts.length > 0) {
    console.log(`  Top contended contracts:`);
    for (const c of data.top_contended_contracts.slice(0, 5)) {
      console.log(
        `    ${c.address} — ${c.contended_slots}/${c.total_slots} slots, score=${c.contention_score.toFixed(3)}`
      );
    }
  }

  if (data.contract_edges.length > 0) {
    console.log(`  Contract co-access edges:`);
    for (const edge of data.contract_edges.slice(0, 3)) {
      console.log(
        `    ${edge.contract_a} ↔ ${edge.contract_b} (${edge.shared_txn_count} shared txns)`
      );
    }
  }
});

client.on("connected", () =>
  console.log("Connected — streaming contention data...")
);
client.connect().catch(console.error);
