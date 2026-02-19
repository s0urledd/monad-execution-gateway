/**
 * Example: Track storage access patterns for a specific contract
 *
 * Uses client-driven subscriptions to filter for AccountAccess events
 * for a specific address.
 *
 * Monitors which storage slots are being read/written for a given address.
 * Useful for understanding a contract's hot storage paths.
 *
 * Usage:
 *   ALLOW_UNRESTRICTED_FILTERS=1 must be set on the gateway for StorageAccess events.
 *
 *   npx ts-node examples/track-storage.ts ws://your-validator:8443 0xYourContract
 */

import { GatewayClient } from "../sdk/typescript/src";

const url = process.argv[2] || "ws://127.0.0.1:8443";
const targetAddress = (process.argv[3] || "").toLowerCase();

if (!targetAddress) {
  console.error("Usage: track-storage.ts <ws-url> <contract-address>");
  process.exit(1);
}

const client = new GatewayClient({ url });

// Track slot access counts
const slotCounts = new Map<string, number>();
let currentBlock = 0;

client.on("connected", () => {
  console.log(`Connected — subscribing to events for ${targetAddress}...`);

  // Use client-driven subscription to only receive the events we need
  client.subscribe({
    events: ["BlockStart", "AccountAccess"],
    filters: [
      {
        event_name: "AccountAccess",
        field_name: "address",
        field_value: targetAddress,
      },
    ],
  });
});

client.on("event", (event) => {
  // Track current block
  if (
    event.event_name === "BlockStart" &&
    event.payload.type === "BlockStart"
  ) {
    if (currentBlock > 0 && slotCounts.size > 0) {
      // Print summary for previous block
      const sorted = [...slotCounts.entries()].sort((a, b) => b[1] - a[1]);
      console.log(
        `\n[Block ${currentBlock}] ${targetAddress} — ${sorted.length} slots accessed:`
      );
      for (const [slot, count] of sorted.slice(0, 10)) {
        console.log(`  ${slot}: ${count}x`);
      }
      slotCounts.clear();
    }
    currentBlock = event.payload.block_number;
  }

  // Track account accesses for our target
  if (
    event.event_name === "AccountAccess" &&
    event.payload.type === "AccountAccess"
  ) {
    if (event.payload.address.toLowerCase() === targetAddress) {
      console.log(
        `  [Block ${currentBlock}] Account access: balance=${event.payload.balance} nonce=${event.payload.nonce}`
      );
    }
  }
});

client.connect().catch(console.error);
