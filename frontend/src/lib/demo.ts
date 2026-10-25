import { xdr } from "@stellar/stellar-sdk";
import {
  NOVA_CONTRACT_ID,
  SignXdr,
  WriteResult,
  addr,
  freighterSign,
  i128,
  readFrom,
  u64,
  writeTo,
} from "./nova";

// Addresses from testnet/deployment.json (v2). Test assets with no value.
export const ADAPTER_ID = "CBUXWJUFVOYJ4TQGXDOHBN7EZFG52CGVZVGWE5E4QCOY2Y5EVLEVEYV4";
export const ORACLE_ID = "CD6TISZMHSGNIUZXCESUIGZHXRLOY6QMCLT34ZS66LDLLRFUH2IYJIJR";
export const ROUTER_ID = "CBAIS4UEFI4YC6K7CRE2XAFSKR3FDT7KCDA5A3SNXIN4H3DDF44MY7D2";
export const BASE_TOKEN_ID = "CCJ7PJWGNO26P3YL76F2SUXCNV6EOBM6JS6PN5MAUQEZSLBHXPCBLCWY"; // TLITH, the SBA's pool asset
export const LOAN_TOKEN_ID = "CBGRV2MJHBM2U3SNWGBGITJLKCTI7PBROUTI7HWQGD2RPPT4TLEXBHQY"; // TUSDC
export const BOND_TOKEN_ID = "CDESIITZRCATXVFJS4DQOFY47457EEFWUQVXHPS6LTNWJUKO2SIPWCHS"; // TBOND

// Demo market terms. Keep these in step with the constitution used in demo/run-demo.sh.
// Demo markets need ONE signer, so a single wallet can clear the intent gate. The three-signer
// rule is exercised by the scripted demo, not by this page.
export const DEMO = {
  maxOracleAgeSecs: 86_400,
  maxSpreadBps: 350,
  minDepth: 9_000_000_000n,
  targetSigners: 1,
  bondAmount: 10_000_000_000n, // 1,000 units at 7 decimals
  lockSecs: 3_600, // the bond unlocks after one hour, so a run can be reclaimed the same day
  supply: 1_000_000n,
} as const;

/** Constitution struct as a Soroban map. Keys are sorted alphabetically, as the contract expects. */
function constitutionVal(): xdr.ScVal {
  const entries: [string, xdr.ScVal][] = [
    ["max_oracle_age_secs", u64(DEMO.maxOracleAgeSecs)],
    ["max_spread_bps", xdr.ScVal.scvU32(DEMO.maxSpreadBps)],
    ["min_depth", i128(DEMO.minDepth)],
    ["target_signers", xdr.ScVal.scvU32(DEMO.targetSigners)],
  ];
  return xdr.ScVal.scvMap(
    entries.map(
      ([key, val]) => new xdr.ScMapEntry({ key: xdr.ScVal.scvSymbol(key), val }),
    ),
  );
}

/** SEP-40 asset enum, Stellar(address), as a Soroban vec of [variant, value]. */
function stellarAssetVal(token: string): xdr.ScVal {
  return xdr.ScVal.scvVec([xdr.ScVal.scvSymbol("Stellar"), addr(token)]);
}

export async function balanceOf(tokenId: string, wallet: string): Promise<bigint> {
  return readFrom<bigint>(tokenId, "balance", [addr(wallet)]);
}

/** Step 1 of a run: create a fresh market. Each run gets its own market, so this is the reset. */
export async function createDemoMarket(
  wallet: string,
  sign: SignXdr = freighterSign,
): Promise<{ result: WriteResult; marketId: number }> {
  const result = await writeTo(
    NOVA_CONTRACT_ID,
    wallet,
    "create_market",
    [
      addr(wallet),
      xdr.ScVal.scvSymbol("DEMO"),
      addr(ORACLE_ID),
      stellarAssetVal(BASE_TOKEN_ID),
      addr(ADAPTER_ID),
      constitutionVal(),
      addr(BOND_TOKEN_ID),
      i128(DEMO.bondAmount),
      u64(DEMO.lockSecs),
      i128(DEMO.supply),
    ],
    sign,
  );
  if (result.status !== "SUCCESS") {
    throw new Error("create_market failed on-chain. See the transaction in the explorer.");
  }
  const marketId = Number(result.value);
  if (!Number.isInteger(marketId)) {
    throw new Error("create_market did not return a market id");
  }
  return { result, marketId };
}

/**
 * How much base to swap in so quote depth falls to about 8.5e9, under the 9e9 threshold.
 *
 * The pool is constant-product, so after swapping x base in, quote left is Q * B / (B + x).
 * Solving for x gives x = B * (Q / target - 1). The base reserve B is not read directly, so it is
 * estimated from the adapter's ask for a small probe: ask ≈ probe * Q / (B - probe).
 * A 10% margin covers the fee and estimation error. Sizing from the live pool keeps the
 * degrade step working after the pool has been reset.
 */
export async function degradeSwapAmount(): Promise<bigint> {
  const snap = await readFrom<{ ask: bigint; depth: bigint }>(ADAPTER_ID, "snapshot", [u64(0)]);
  const quote = Number(snap.depth);
  const probe = 10_000_000; // matches the adapter's probe_base
  const ask = Number(snap.ask);
  if (ask <= 0 || quote <= 0) {
    throw new Error("the pool has no usable quote, so the swap size cannot be worked out");
  }
  const base = (probe * quote) / ask + probe;
  const target = 8_500_000_000;
  const amount = Math.ceil(base * (quote / target - 1) * 1.1);
  return BigInt(Math.max(amount, 100_000_000));
}

/** Step 4 of a run: swap base into the pool, which drains quote depth. */
export function swapToDegrade(
  wallet: string,
  amountIn: bigint,
  sign: SignXdr = freighterSign,
): Promise<WriteResult> {
  const deadline = Math.floor(Date.now() / 1000) + 3_600;
  return writeTo(
    ROUTER_ID,
    wallet,
    "swap_exact_tokens_for_tokens",
    [
      i128(amountIn),
      i128(0n),
      xdr.ScVal.scvVec([addr(BASE_TOKEN_ID), addr(LOAN_TOKEN_ID)]),
      addr(wallet),
      u64(deadline),
    ],
    sign,
  );
}

/** Step 5 of a run: the creator reclaims the bond after the lock expires. */
export function reclaimBond(
  wallet: string,
  marketId: number,
  sign: SignXdr = freighterSign,
): Promise<WriteResult> {
  return writeTo(NOVA_CONTRACT_ID, wallet, "withdraw_bond", [u64(marketId)], sign);
}

// Per-viewer convenience only: which markets this browser created. Never trusted for state.
const RUNS_KEY = "nova.demo.runs";

export type DemoRun = { marketId: number; createdAt: string; wallet: string };

export function loadRuns(): DemoRun[] {
  try {
    const raw = window.localStorage.getItem(RUNS_KEY);
    return raw ? (JSON.parse(raw) as DemoRun[]) : [];
  } catch {
    return [];
  }
}

export function saveRun(run: DemoRun): void {
  try {
    const runs = loadRuns().filter((r) => r.marketId !== run.marketId);
    window.localStorage.setItem(RUNS_KEY, JSON.stringify([run, ...runs].slice(0, 25)));
  } catch {
    // Storage can be blocked. The page still works, it just forgets the run list.
  }
}
