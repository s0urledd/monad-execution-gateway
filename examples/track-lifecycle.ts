/**
 * Example: Track block lifecycle through Monad's consensus stages
 *
 * Watch blocks progress: Proposed → Voted → Finalized → Verified
 * Each stage transition includes timing data showing consensus performance.
 *
 * Usage:
 *   npx ts-node examples/track-lifecycle.ts ws://your-node:8443
 */

import { GatewayClient, type BlockLifecycleUpdate } from "../sdk/typescript/src";

const url = process.argv[2] || "ws://127.0.0.1:8443";

// Connect to the dedicated lifecycle channel — only stage transitions, no noise
const client = new GatewayClient({ url, channel: "lifecycle" });

const stageEmoji: Record<string, string> = {
  Proposed: "📦",
  Voted: "🗳️",
  Finalized: "✅",
  Verified: "🔒",
  Rejected: "❌",
};

client.on("lifecycle", (update: BlockLifecycleUpdate) => {
  const icon = stageEmoji[update.to_stage] || "?";
  const from = update.from_stage ? ` (was ${update.from_stage})` : "";
  const timing = update.time_in_previous_stage_ms
    ? ` | stage time: ${update.time_in_previous_stage_ms.toFixed(1)}ms`
    : "";

  console.log(
    `${icon} Block ${update.block_number} → ${update.to_stage}${from}` +
      ` | age: ${update.block_age_ms.toFixed(1)}ms` +
      ` | txns: ${update.txn_count}` +
      timing
  );
});

client.on("connected", () =>
  console.log("Connected — watching block lifecycle transitions\n")
);
client.on("disconnected", () => console.log("\nDisconnected"));

client.connect().catch(console.error);
