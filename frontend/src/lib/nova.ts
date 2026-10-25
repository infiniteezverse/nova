import {
  Account,
  Address,
  Contract,
  TransactionBuilder,
  Networks,
  nativeToScVal,
  rpc,
  scValToNative,
  xdr,
} from "@stellar/stellar-sdk";
// The package ships a UMD bundle, so named ESM imports depend on bundler interop.
// Its default export is the same set of functions and works in both Node and the browser.
import freighter from "@stellar/freighter-api";

const { getAddress, isConnected, requestAccess, signTransaction } = freighter;

export const RPC_URL = "https://soroban-testnet.stellar.org";
export const NETWORK_PASSPHRASE = Networks.TESTNET;
// NOVA v2 on testnet (see testnet/deployment.json). Override with VITE_NOVA_CONTRACT_ID.
export const NOVA_CONTRACT_ID: string =
  import.meta.env?.VITE_NOVA_CONTRACT_ID ?? "CDSN2U5SG3YU27YPNKMKHJEPVFVPVA2BYOWDBYRTC72QPG3HYOVHK5ME";
export const EXPLORER_TX = "https://stellar.expert/explorer/testnet/tx/";

// Public account used only as the source for read simulations. It needs no signature and no funds.
const READ_SOURCE = "GABW3E5FDCCM7Q2BBIBHE5LFXR4KOO5FUC7LGWWUPVTV4WQJANDIBV5T";

export const server = new rpc.Server(RPC_URL);
const contract = new Contract(NOVA_CONTRACT_ID);

export type Observation = {
  health: string;
  price: bigint;
  oracle_timestamp: bigint;
  spread_bps: bigint;
  depth: bigint;
};

export type Caps = {
  reference_pricing: boolean;
  spot_trading: boolean;
  collateral_eligible: boolean;
};

export type MarketSummary = {
  symbol: string;
  signers: number;
  state: string;
  last_checkpoint: bigint;
};

/** Unit enum variants decode as one-element arrays, e.g. ["Degraded"]. Return the name. */
function variant(value: unknown): string {
  if (Array.isArray(value)) return String(value[0]);
  return String(value);
}

function u64(value: number | bigint): xdr.ScVal {
  return nativeToScVal(BigInt(value), { type: "u64" });
}

function addr(value: string): xdr.ScVal {
  return new Address(value).toScVal();
}

/** Simulate a read-only call and decode its return value. */
async function read<T>(method: string, args: xdr.ScVal[]): Promise<T> {
  const account = new Account(READ_SOURCE, "0");
  const tx = new TransactionBuilder(account, {
    fee: "100000",
    networkPassphrase: NETWORK_PASSPHRASE,
  })
    .addOperation(contract.call(method, ...args))
    .setTimeout(30)
    .build();

  const sim = await server.simulateTransaction(tx);
  if (!rpc.Api.isSimulationSuccess(sim) || !sim.result) {
    const reason = rpc.Api.isSimulationError(sim) ? sim.error : "no result";
    throw new Error(`${method} failed: ${reason}`);
  }
  return scValToNative(sim.result.retval) as T;
}

export async function fetchState(marketId: number): Promise<string> {
  return variant(await read<unknown>("state", [u64(marketId)]));
}

export function fetchEligible(marketId: number): Promise<boolean> {
  return read<boolean>("is_eligible", [u64(marketId)]);
}

export function fetchCaps(marketId: number): Promise<Caps> {
  return read<Caps>("caps", [u64(marketId)]);
}

export async function fetchObservation(marketId: number): Promise<Observation> {
  const o = await read<Observation & { health: unknown }>("observe", [u64(marketId)]);
  return { ...o, health: variant(o.health) };
}

export async function fetchMarket(marketId: number): Promise<MarketSummary> {
  const m = await read<{
    symbol: string;
    signers: number;
    state: unknown;
    last_checkpoint: bigint;
  }>("get_market", [u64(marketId)]);
  return {
    symbol: m.symbol,
    signers: m.signers,
    state: variant(m.state),
    last_checkpoint: m.last_checkpoint,
  };
}

export type WriteResult = { hash: string; status: "SUCCESS" | "FAILED" };

/** Takes an unsigned transaction XDR and returns the signed XDR. */
export type SignXdr = (xdrString: string, wallet: string) => Promise<string>;

/** Default signer: Freighter in the browser. */
export const freighterSign: SignXdr = async (xdrString, wallet) => {
  const signed = await signTransaction(xdrString, {
    networkPassphrase: NETWORK_PASSPHRASE,
    address: wallet,
  });
  if (signed.error) {
    throw new Error(`Freighter: ${signed.error.message ?? signed.error}`);
  }
  return signed.signedTxXdr;
};

/** Build, sign, submit, and wait for the ledger result. */
async function write(
  wallet: string,
  method: string,
  args: xdr.ScVal[],
  sign: SignXdr,
): Promise<WriteResult> {
  const account = await server.getAccount(wallet);
  const tx = new TransactionBuilder(account, {
    fee: "100000",
    networkPassphrase: NETWORK_PASSPHRASE,
  })
    .addOperation(contract.call(method, ...args))
    .setTimeout(60)
    .build();

  const prepared = await server.prepareTransaction(tx);
  const signedXdr = await sign(prepared.toXDR(), wallet);

  const signedTx = TransactionBuilder.fromXDR(signedXdr, NETWORK_PASSPHRASE);
  const sent = await server.sendTransaction(signedTx);
  if (sent.status === "ERROR") {
    throw new Error(`submit rejected: ${sent.errorResult?.toXDR("base64") ?? "unknown"}`);
  }

  for (let attempt = 0; attempt < 30; attempt++) {
    const res = await server.getTransaction(sent.hash);
    if (res.status === rpc.Api.GetTransactionStatus.SUCCESS) {
      return { hash: sent.hash, status: "SUCCESS" };
    }
    if (res.status === rpc.Api.GetTransactionStatus.FAILED) {
      return { hash: sent.hash, status: "FAILED" };
    }
    await new Promise((resolve) => setTimeout(resolve, 1500));
  }
  throw new Error(`timed out waiting for ${sent.hash}`);
}

/** Permissionless: anyone can record the current health. */
export function submitCheckpoint(
  wallet: string,
  marketId: number,
  sign: SignXdr = freighterSign,
): Promise<WriteResult> {
  return write(wallet, "checkpoint", [u64(marketId)], sign);
}

/** Wallet-signed: records this wallet as one unique signer. */
export function submitIntent(
  wallet: string,
  marketId: number,
  sign: SignXdr = freighterSign,
): Promise<WriteResult> {
  return write(wallet, "signal_intent", [u64(marketId), addr(wallet)], sign);
}

export async function connectWallet(): Promise<string> {
  const connected = await isConnected();
  if (!connected.isConnected) {
    throw new Error("Freighter is not installed or not reachable");
  }
  const access = await requestAccess();
  if (access.error) {
    throw new Error(`Freighter: ${access.error.message ?? access.error}`);
  }
  return access.address;
}

export async function currentWallet(): Promise<string | null> {
  const res = await getAddress();
  return res.error ? null : res.address || null;
}
