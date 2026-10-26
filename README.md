# NOVA Protocol

NOVA is a State-Bound Asset (SBA) protocol on Stellar. An SBA is a token whose transfers and
collateral use depend on the market's live health. Instead of trading freely while its pool
thins out, the asset checks its own rules on-chain, and when a rule fails, the parts of the
market that depend on it halt.

> **Status: working testnet prototype.** Unaudited hackathon code. The oracle is a test feed
> controlled by one admin key. Slashing and the early-withdrawal pause are not built.
> Test tokens have no value.

## Contents

- [What it does](#what-it-does)
- [Architecture](#architecture)
- [How a market works](#how-a-market-works)
- [The constitution](#the-constitution)
- [Lending vault and liquidation](#lending-vault-and-liquidation)
- [Live deployment (testnet)](#live-deployment-testnet)
- [Running it](#running-it)
- [Running the demo](#running-the-demo)
- [Status](#status)
- [Known gaps and limits](#known-gaps-and-limits)
- [Trust model](#trust-model)
- [Repository layout](#repository-layout)

## What it does

Emerging assets, such as a lithium index or a carbon credit, have thin markets. A plain token
keeps trading while its pool drains, and lenders find out later. NOVA ties a token's utility
to checks that anyone can verify on-chain:

- **Oracle freshness:** the price feed must be recent.
- **Pool spread and depth:** the asset must still be exitable at a reasonable price.
- **Signer count:** a minimum number of unique participants must sign intents before the market activates.

When a check fails, the market moves to Degraded (trading halts, reference pricing stays on)
or Paused (everything halts). Lending contracts call `is_eligible()` before accepting the
asset as collateral, so they stop taking new risk too.

## Architecture

| Component | Path | What it does |
| --- | --- | --- |
| NOVA contract | `src/` | Market registry, bond escrow, signer gate, constitution, SBA balances, `is_eligible()`, `settle_transfer` |
| Soroswap adapter | `adapters/soroswap/` | Reads a pool's bid, ask and quote depth through the Soroswap router. Bound to one pair at deploy |
| Lending vault | `vault/` | Takes SBA as collateral, lends a loan token, and liquidates underwater positions |
| Frontend | `frontend/` | Vite + React terminal. Reads live state from Soroban RPC and sends wallet transactions through Freighter |
| Demo scripts | `demo/` | Operator scripts: live demo run, wallet funding, oracle refresh, pool reset |
| Testnet record | `testnet/` | Deployed addresses (`deployment.json`) and the superseded v1 record |

Contracts are plain Rust compiled to Soroban WASM. The frontend has no backend. It simulates
read-only calls directly against Soroban testnet RPC and sends signed transactions from the browser.

## How a market works

| Step | Function | Who | Effect |
| --- | --- | --- | --- |
| 1 | `create_market` | creator | Locks a bond of at least 1,000 units in escrow with a time lock, mints the SBA supply to the creator. Starts in `Discovery` |
| 2 | `signal_intent` | anyone, once per address | Counts unique signers. At the target, the market moves to `Ready` |
| 3 | `checkpoint` | anyone | Reads the health rules and writes the state to the ledger. `Ready` becomes `Active` when healthy |
| 4 | `observe` | anyone | The same evaluation, read-only. Returns the numbers without writing |
| 5 | `is_eligible` / `caps` / `state` | other contracts | Read-only. `is_eligible` is true only when the effective state is `Active` |
| 6 | `transfer` | holders | SBA transfer. Reverts unless the effective `spot_trading` capability is on |
| 7 | `withdraw_bond` | creator | Returns the bond after `unlock_time`. Reverts before it |

A market's **effective** state is what callers see. If the last checkpoint is older than the
market's oracle-age limit, the effective state is `Paused` even if the stored state is `Active`.
Nothing has to checkpoint for the halt to take effect, and a fresh checkpoint restores it.

## The constitution

Each market stores its thresholds at creation. The rules are evaluated by `checkpoint` and `observe`.

| Rule | Check | Result |
| --- | --- | --- |
| 01 / 02 | Oracle price missing, non-positive, or older than `max_oracle_age_secs` | `Paused` |
| 03 | Bid-ask spread above `max_spread_bps` | `Degraded` |
| 04 | Quote-side depth below `min_depth` | `Degraded` |
| 06 | Unique signers below `target_signers` | Stays in `Discovery` |
| Bond | Bond stays locked until `unlock_time` | Enforced by `withdraw_bond` |
| 09 (early withdrawal) | Pausing issuance on an early withdrawal | **Not implemented.** See gaps |
| Fraud | Slashing the bond on proven fraud | **Not implemented.** See gaps |

Capabilities by state:

| State | Reference pricing | Spot trading | Collateral |
| --- | --- | --- | --- |
| Active | on | on | on |
| Discovery, Ready, Degraded | on | off | off |
| Paused | off | off | off |

## Lending vault and liquidation

The vault accepts SBA as collateral and lends a loan token against it.

| Function | Gate | Notes |
| --- | --- | --- |
| `supply` / `withdraw_supply` | none | Lenders deposit the loan token. Withdrawals are limited by pool cash |
| `deposit_collateral` | `is_eligible` | Moves SBA into the vault through NOVA's `transfer` |
| `borrow` | `is_eligible`, LTV | Limit = collateral × oracle price / 1e7 × LTV (50% here) |
| `repay` | none | Allowed in every state |
| `withdraw_collateral` | debt covered after withdrawal | Fails while Degraded, because NOVA halts SBA transfers |
| `liquidate` | position unhealthy at the oracle price; market Active or Degraded | Liquidator repays up to 50% of debt and receives collateral at oracle price plus a 5% bonus |

**Liquidation math:** a position is liquidatable when `debt > collateral × price / 1e7 × LTV`.
Repay is capped at `ceil(debt × 50%)`. Seized collateral is
`repay × 1e7 / price × (10000 + bonus) / 10000`, capped at the borrower's collateral.

**Settlement path (why liquidation works while Degraded):** trading stays halted in Degraded,
but liquidation needs to move collateral. NOVA's `settle_transfer` allows this for lending
contracts on a fixed `settlers` list:

- Only the settler's own balance can move. `from` must equal the settler, so a vault can
  release its own collateral and never a user's balance.
- It works in Active and Degraded, and it refuses Paused, because a dead oracle leaves no valid price.
- Deposits and new borrows still require `is_eligible`. A Degraded market gets no new risk.
- Ordinary user-to-user transfers stay halted in Degraded.

**Deployment order.** NOVA and the vault reference each other, so both must be registered before
either is initialized. Register NOVA and the vault, initialize NOVA with the vault in `settlers`,
then initialize the vault with NOVA's address. The integration test shows this order.

## Live deployment (testnet)

Authoritative addresses are in [`testnet/deployment.json`](testnet/deployment.json) (`"current": "v2"`).

| Contract | Address |
| --- | --- |
| NOVA v2 | `CDSN2U5SG3YU27YPNKMKHJEPVFVPVA2BYOWDBYRTC72QPG3HYOVHK5ME` |
| Lending vault | `CCMVZHUOGXOOM6ANLESIDX4S3KRQHKQQOIPIXIXS7UCQJFGOXJQYEV2N` |
| Soroswap adapter | `CBUXWJUFVOYJ4TQGXDOHBN7EZFG52CGVZVGWE5E4QCOY2Y5EVLEVEYV4` |
| Test oracle (admin-set) | `CD6TISZMHSGNIUZXCESUIGZHXRLOY6QMCLT34ZS66LDLLRFUH2IYJIJR` |
| Soroswap router (built and deployed by this project) | `CBAIS4UEFI4YC6K7CRE2XAFSKR3FDT7KCDA5A3SNXIN4H3DDF44MY7D2` |
| Soroswap factory (built and deployed by this project) | `CDFD6WZN5JP4O62SSTX6J4X322BNGLBBLGQSAYEOU36MK7OGOYJOVXEY` |

Test tokens: TLITH (the pool's base and the SBA's pool asset) `CCJ7PJWGNO26P3YL76F2SUXCNV6EOBM6JS6PN5MAUQEZSLBHXPCBLCWY`,
TUSDC (the loan token and pool quote) `CBGRV2MJHBM2U3SNWGBGITJLKCTI7PBROUTI7HWQGD2RPPT4TLEXBHQY`,
TBOND (bonds) `CDESIITZRCATXVFJS4DQOFY47457EEFWUQVXHPS6LTNWJUKO2SIPWCHS`.

Verified on testnet:

- A full liquidation while Degraded (market 2): repaid 25,000, seized 43,749, and the liquidator's
  ordinary transfer refused with error #9. Transaction `c7b11c9e53f5550b32592fab82ba20268bf2aa21d83a2c9cf8ca23c6ea4dec67`.
- Each demo run is a new market. A browser run with Freighter completed create, intent, checkpoint,
  and swap. Reclaim refuses until the one-hour lock has passed, as intended.

The live site is https://nova-protocol-astragrowth-5196s-projects.vercel.app. Its snapshot page
(`/snapshot.html`) explains the state and what is not shown.

## Running it

Prerequisites: Rust with the `wasm32v1-none` target, the Stellar CLI, Node.js 20 or later.

```bash
# NOVA contract
stellar contract build                          # -> target/wasm32v1-none/release/nova_sba.wasm
cargo test                                      # 15 unit tests

# Soroswap adapter
cd adapters/soroswap && cargo test              # 6 tests (mock router and pair)
cd adapters/soroswap && stellar contract build

# Lending vault
cd vault && stellar contract build && cargo test

# Integration: NOVA, vault, adapter, and the Soroswap WASMs (9 tests)
cd integration && cargo test

# Frontend
cd frontend && npm install && npm run dev      # http://localhost:5173
cd frontend && npm run build                   # static build for hosting
```

The integration tests run against WASM files in `integration/wasm/`. Those Soroswap WASMs
were built from [github.com/soroswap/core](https://github.com/soroswap/core) with Rust 1.84 and
`RUSTFLAGS="--cap-lints=warn -C target-feature=-reference-types"`, because the Soroswap contracts
pin soroban-sdk 20 and the host rejects WASM with reference types. The factory WASM must also be
copied to `soroswap_factory.optimized.wasm`, because the router's `contractimport!` expects that name.

Reads in the frontend need no wallet. Wallet actions use Freighter on Testnet.

## Running the demo

The live terminal runs the demo for a viewer. Each step is a real testnet transaction:

1. **Create Market.** Creates a fresh market with a 1,000 TBOND bond and a one-signer constitution.
2. **Sign intent.** The market moves to Ready.
3. **Checkpoint.** The market moves to Active, if the pool is healthy.
4. **Swap into pool.** Sizes a swap from the live pool and drains quote depth. The next checkpoint moves the market to Degraded.
5. **Reclaim bond.** Returns the bond after the one-hour lock.

Operator scripts (all run against the deployer identity on testnet):

| Script | When to run it | What it does |
| --- | --- | --- |
| `demo/refresh-oracle.sh` | Before each session | Refreshes the test oracle. The oracle expires after 24 hours, and every market pauses when it's stale |
| `demo/fund-wallet.sh <G-address>` | Before a viewer's run | Sends 3,000 TBOND and 1,200 TLITH, and tops up XLM through Friendbot |
| `demo/reset-pool.sh` | After any degrade swap | Adds liquidity until quote depth clears the 900-unit threshold, so a new market can activate |
| `demo/run-demo.sh` | To produce a scripted proof run | Runs the full three-signer sequence and writes `demo/last-run.json` |

`demo/RUNBOOK.md` has the talk track and the exact steps for a recorded demo.

## Status

**Verified**
- Contract logic, with 15 unit tests for NOVA, 6 for the adapter, and 9 integration tests against compiled WASM.
- The full liquidation path on testnet, shown above.
- A live browser run of the wallet flow.
- The read-only terminal shows live state from testnet, with no wallet needed.

**Not verified**
- Multiple wallet runs in a row. The second run needs a pool reset first.
- A real SEP-40 oracle on testnet. None was found in the docs, so the test oracle is used.
- Mainnet. Nothing here is deployed to mainnet.

## Known gaps and limits

**Open design gaps**
- **Rule 09 (early withdrawal pause) is partly implemented.** A reverting Soroban call rolls back
  everything it wrote, so the contract can refuse an early withdrawal but cannot record a pause in
  the same call. Fixing this needs a separate, non-reverting function.
- **Slashing is not implemented.** The spec's fraud penalty needs a way to prove fraud, and nothing
  in the contract does that.
- **Degraded liquidation depends on exit liquidity.** A liquidator receives SBA that can only be
  sold after the market recovers. A long Degraded period can leave bad debt unclaimed.
- **Paused freezes liquidation.** With a dead oracle there is no valid price, so bad debt can build
  while the market is Paused.
- **Collateral is locked while Degraded.** Users can repay but cannot withdraw collateral until the
  market recovers, because NOVA halts SBA transfers outside the settlement path.
- **The SBA's internal balances and the pool's token are separate.** A real deployment has to decide
  how the pooled asset relates to the SBA.

**Known bugs**
- **Swap sizing when the pool is already shallow.** `degradeSwapAmount` falls back to a 10-TLITH swap
  when the pool is already below target, so the degrade step does nothing. Its fix is to skip the
  swap and tell the viewer to reset the pool.

**Operational limits**
- **Oracle expiry.** The test oracle expires after 24 hours, and every market pauses when it's stale.
- **Pool depth decays.** Each degrade swap leaves the pool base-heavy, so the next swap costs more.
  A rebalancing step (a reverse swap in the reset script) is not built.
- **Bonds lock for one hour** in the demo, so bonds from earlier runs stay locked until then.
- **No interest.** Lenders earn nothing and borrowers pay nothing.
- **Price units are assumed.** The vault treats 1 SBA unit as oracle price / 1e7 loan units.
- **Vault-as-sender auth was verified on testnet,** but only with the operator's keys.

**Design limits**
- **The allowlists are fixed at deploy.** Oracles, adapters and settlers are set once. Changing
  any of them means redeploying NOVA.
- **Timestamps are assumed to be seconds** in SEP-40 price data. Confirm this against the oracle you bind.
- **Spread and depth come from one source,** the adapter. A Soroswap-backed adapter needs its own integrity story.
- **Integrity of the adapter is a trust boundary.** A malicious adapter can report healthy numbers.
  The allowlist stops arbitrary adapters, but the allowlisted one is trusted.

**Not built**
- A production oracle. The test feed is admin-controlled.
- A protocol reserve for bad debt.
- An audit. Tests cover the paths we considered, not an independent review.

## Trust model

- **NOVA's contract logic has no admin key.** Market rules are fixed when a market is created, and
  checkpoints are permissionless.
- **The test oracle has an admin key.** One identity sets its price. Anyone who controls that key
  can halt or unhalt every market, so this is a demo component and not a trust-minimized one.
- **Allowlists are set once at deploy.** Oracles, adapters, and settlers cannot be changed afterward.
- **The deployer identity** (`nova-deployer`) is the test token admin and the oracle admin. The
  three signer identities are test keys. None of the keys are in the repo, and none hold value.
- **The frontend holds no keys.** Freighter signs in the browser, and the page never handles
  a secret.

## Repository layout

```
src/                   NOVA contract (lib.rs, test.rs)
adapters/soroswap/     Soroswap liquidity adapter
vault/                 Lending vault contract
integration/           Integration tests and the WASM files they run against
testnet-oracle/        The test SEP-40 oracle contract
frontend/              Vite + React terminal (src/, scripts/, public/snapshot.html)
demo/                  Demo script, runbook, operator scripts, snapshot page
testnet/               Deployment record (deployment.json) and v1 superseded record
```
