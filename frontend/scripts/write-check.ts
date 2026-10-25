import { execSync } from "node:child_process";
import { Keypair, TransactionBuilder, Networks } from "@stellar/stellar-sdk";
import { submitCheckpoint, submitIntent, NETWORK_PASSPHRASE } from "../src/lib/nova";

// Local test signer: the secret stays in memory and is never printed.
const secret = execSync("stellar keys show nova-signer-1", { encoding: "utf8" }).trim();
const kp = Keypair.fromSecret(secret);
const wallet = kp.publicKey();
const signLocally = async (xdr: string) => {
  const tx = TransactionBuilder.fromXDR(xdr, NETWORK_PASSPHRASE);
  tx.sign(kp);
  return tx.toXDR();
};

const cp = await submitCheckpoint(wallet, 0, signLocally);
console.log("checkpoint:", cp.status, cp.hash);

try {
  const it = await submitIntent(wallet, 0, signLocally);
  console.log("intent (unexpected success):", it.status, it.hash);
} catch (err) {
  console.log("intent rejected as expected:", (err as Error).message.slice(0, 160));
}
