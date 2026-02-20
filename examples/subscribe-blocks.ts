/**
 * Example: Subscribe to block lifecycle events
 *
 * Uses the /v1/ws/blocks channel to receive only block events + TPS.
 *
 * Usage:
 *   npx ts-node examples/subscribe-blocks.ts ws://your-validator:8443
 */

import { GatewayClient } from "../sdk/typescript/src/index";

const url = process.argv[2] || "ws://127.0.0.1:8443";
const client = new GatewayClient({ url, channel: "blocks" });

client.onEvent("BlockStart", (event) => {
  if (event.payload.type === "BlockStart") {
    console.log(
      `[BLOCK ${event.payload.block_number}] Started | gas_limit=${event.payload.gas_limit} beneficiary=${event.payload.beneficiary}`
    );
  }
});

client.onEvent("BlockEnd", (event) => {
  if (event.payload.type === "BlockEnd") {
    console.log(
      `[BLOCK ${event.block_number}] Ended | gas_used=${event.payload.gas_used}`
    );
  }
});

client.onEvent("BlockQC", (event) => {
  if (event.payload.type === "BlockQC") {
    console.log(
      `[BLOCK ${event.payload.block_number}] QC received | round=${event.payload.round}`
    );
  }
});

client.onEvent("BlockFinalized", (event) => {
  if (event.payload.type === "BlockFinalized") {
    console.log(`[BLOCK ${event.payload.block_number}] Finalized`);
  }
});

client.on("tps", (tps) => {
  console.log(`[TPS] ${tps}`);
});

client.on("connected", () => console.log("Connected to gateway (blocks channel)"));
client.on("disconnected", () => console.log("Disconnected"));

client.connect().catch(console.error);
