// Runs the page's own wallet functions on testnet with a local key standing in for Freighter.
// Steps: create a run, sign intent, checkpoint (expect Active), swap (expect Degraded),
// checkpoint again. Requires the oracle to be fresh and the pool deep enough (see demo/ scripts).
//
// Usage: npx tsx scripts/demo-check.ts     (uses the nova-signer-3 CLI identity; never printed)

import { execSync } from "node:child_process";
import { Keypair, TransactionBuilder } from "@stellar/stellar-sdk";
import { NETWORK_PASSPHRASE, fetchObservation, fetchState, submitCheckpoint, submitIntent } from "../src/lib/nova";
import {
  createDemoMarket,
  degradeSwapAmount,
  swapToDegrade,
  balanceOf,
  BASE_TOKEN_ID,
  BOND_TOKEN_ID,
} from "../src/lib/demo";

const secret = execSync("stellar keys show nova-signer-3", { encoding: "utf8" }).trim();
const kp = Keypair.fromSecret(secret);
const wallet = kp.publicKey();

// Same contract as Freighter: take unsigned XDR, return signed XDR.
const signLocally = async (xdr: string) => {
  const tx = TransactionBuilder.fromXDR(xdr, NETWORK_PASSPHRASE);
  tx.sign(kp);
  return tx.toXDR();
};

const ok = (msg: string) => console.log("   ok:", msg);
const die = (msg: string) => {
  console.error("FAILED:", msg);
  process.exit(1);
};

console.log("== balances");
const tbond = await balanceOf(BOND_TOKEN_ID, wallet);
const tlith = await balanceOf(BASE_TOKEN_ID, wallet);
ok(`TBOND ${tbond}, TLITH ${tlith}`);
if (tbond < 10_000_000_000n) die("wallet needs 1,000 TBOND (run demo/fund-wallet.sh)");
if (tlith < 2_000_000_000n) die("wallet needs 200 TLITH (run demo/fund-wallet.sh)");

console.log("== 1. new run");
const { result, marketId } = await createDemoMarket(wallet, signLocally);
ok(`market ${marketId} created, tx ${result.hash}`);
if ((await fetchState(marketId)) !== "Discovery") die("new market is not in Discovery");

console.log("== 2. sign intent");
const intent = await submitIntent(wallet, marketId, signLocally);
if (intent.status !== "SUCCESS") die("intent failed");
ok(`intent recorded, tx ${intent.hash}`);
if ((await fetchState(marketId)) !== "Ready") die("expected Ready after the single signer");

console.log("== 3. checkpoint (expect Active)");
const cp1 = await submitCheckpoint(wallet, marketId, signLocally);
if (cp1.status !== "SUCCESS") die("checkpoint failed");
const state1 = await fetchState(marketId);
ok(`state ${state1}, tx ${cp1.hash}`);
if (state1 !== "Active") die(`expected Active, got ${state1}. Is the oracle fresh and the pool deep?`);

console.log("== 4. swap into the pool (expect Degraded after checkpoint)");
const amount = await degradeSwapAmount();
ok(`sizing the swap at ${amount} base units`);
const sw = await swapToDegrade(wallet, amount, signLocally);
if (sw.status !== "SUCCESS") die("swap failed");
ok(`swap tx ${sw.hash}`);
const cp2 = await submitCheckpoint(wallet, marketId, signLocally);
const state2 = await fetchState(marketId);
const obs = await fetchObservation(marketId);
ok(`state ${state2}, depth ${obs.depth}, spread ${obs.spread_bps} bps, tx ${cp2.hash}`);
if (state2 !== "Degraded") die(`expected Degraded, got ${state2}`);

console.log(`\nPASS: market ${marketId} went Discovery -> Ready -> Active -> Degraded with a wallet-style signer.`);
