# NOVA: State-Bound Asset (SBA) Registry on Stellar

A Soroban contract (`nova-sba`) that mints an asset whose transferability and
collateral eligibility are gated by on-chain health checks. There is no admin key.

## Build and test

```bash
stellar contract build                       # -> target/wasm32v1-none/release/nova_sba.wasm
cargo test                                   # 15 NOVA core tests
cd adapters/soroswap && cargo test           # 6 adapter tests
cd adapters/soroswap && stellar contract build
cd integration && cargo test                 # 1 end-to-end test: NOVA + real Soroswap WASMs
```

### Integration test (`integration/`)

Runs NOVA, the adapter, and the Soroswap factory, pair and router as compiled WASMs.
It seeds a real pool, activates a market, then runs a real swap that drops quote-side
depth below `min_depth`. The market should then go `Degraded`, and transfers should halt.
The only Rust mock is the SEP-40 oracle, because no SEP-40 oracle WASM is checked in.

The Soroswap WASMs in `integration/wasm/` were built from github.com/soroswap/core
with these steps:

- Use Rust 1.84 (`rustup toolchain install 1.84.0 -t wasm32-unknown-unknown`).
  Soroswap pins soroban-sdk 20, which fails to compile on the current stable Rust.
- Build in Makefile order (token, pair, factory, router) with
  `RUSTFLAGS="--cap-lints=warn -C target-feature=-reference-types"`.
  The host rejects WASM that uses reference types.
- Copy `factory/.../soroswap_factory.wasm` to `soroswap_factory.optimized.wasm`.
  The router's `contractimport!` expects that name.

The `ext/` clone is only needed to rebuild the Soroswap WASMs. It is not needed to run the test.

## Lifecycle

| Step | Function | Who | Effect |
| --- | --- | --- | --- |
| 1 | `create_market` | creator | Locks the bond (SEP-41), mints SBA supply to the creator, state `Discovery` |
| 2 | `signal_intent` | anyone, once per address | Counts unique signers. At target: `Ready` |
| 3 | `checkpoint` | anyone | Evaluates health and writes the state. `Ready` becomes `Active` when healthy |
| 4 | `observe` | anyone | Same evaluation, read-only, returns the numbers |
| 5 | `is_eligible` / `caps` | DeFi protocols | Pure read. `is_eligible` is true only when effectively `Active` |
| 6 | `transfer` | holders | SBA transfer, reverts unless effective `spot_trading` is on |
| 7 | `withdraw_bond` | creator | Reverts before `unlock_time` |

## Constitution rules implemented

- **Oracle dead, missing, or stale** (rules 01/02): `Paused`. Everything halts.
- **Spread above `max_spread_bps`** (rule 03): `Degraded`. Spot trading and collateral off, reference pricing on.
- **Depth below `min_depth`** (rule 04): `Degraded`.
- **Stale checkpoint**: if the last checkpoint is older than `max_oracle_age_secs`, the effective state is `Paused` for `transfer`, `is_eligible`, `caps`, and `state`. Nobody has to checkpoint for the halt to take effect. A checkpoint with fresh data heals it.

## Gaps versus the pasted spec

These need a decision before this is more than a hackathon demo.

1. **Oracle and liquidity allowlists (addressed, with a tradeoff).** `initialize(oracles,
   liquidity)` stores the approved SEP-40 oracle addresses and liquidity-adapter addresses
   once. `create_market` rejects any other oracle or adapter. Neither list has an add, remove,
   or admin function, so there is still no admin key. The cost: changing either list means
   redeploying. Both lists must be populated with verified addresses at deploy time. This
   repo does not contain any.
2. **Rule 09 cannot persist a pause.** On Soroban a reverting call rolls back its state
   changes, so `withdraw_bond` before unlock can revert but cannot also pause issuance.
   The spec needs a separate, non-reverting function to record a default.
3. **No slashing.** The spec says the bond is "slashed or frozen" on proven fraud. It does
   not say who proves fraud or how. `withdraw_bond` only releases after the lock.
4. **Draft state is collapsed.** `create_market` goes straight to `Discovery`.
5. **Soroswap adapter (built, mock-tested only).** `adapters/soroswap` is a separate contract.
   It is bound to one token pair at `initialize`. `snapshot` probes the router for bid and
   ask at a fixed `probe_base` size and reads quote-side reserves from the pair. Its 6 tests
   use mock router and pair contracts. The router and pair signatures come from
   github.com/soroswap/core source. Mainnet router is
   `CAG5LRYQ5JVEUI5TEID72EYOVX44TTUJT5BQR2J6J77FH65PCCFAJDDH`. I did not find a testnet
   router ID, and the docs do not include one. Not yet verified against a live deployment.
   If the router or pair is missing, `snapshot` reverts, so no healthy state can be written.
   The market then goes stale and is effectively `Paused`.
6. **SEP-40 call shape.** The playbook shows `let (price, timestamp) = oracle_client.lastprice(&asset);`.
   The SEP-40 `lastprice` returns `Option<PriceData>`, so the playbook snippet would not
   compile. This contract declares its own client trait with the correct shape. I did not
   verify the `sep-40-oracle` crate the playbook names, so I did not depend on it.
7. **Timestamp units.** The contract assumes SEP-40 timestamps are in seconds. Confirm this
   against the specific oracle you bind to.
8. **Spread and depth come from one source.** The contract trusts the adapter for
   bid, ask, and depth. A Soroswap-backed adapter would need its own integrity story.
9. **The SBA ledger and the pool token are separate.** NOVA's internal SBA balances are not
   the token the Soroswap pool trades. The integration test uses a separate SAC token for
   the pool (`base`). A real deployment must decide how the pooled asset relates to the
   SBA. This is an open design question, not a solved one.
10. **Integration test is testnet-free.** It runs in the local test environment against
    WASMs. It does not prove a live testnet or mainnet deployment works.

## Not built

- Frontend (Constitution panel, lending vault demo).
- Stellar testnet deployment.
- Events use `#[contractevent]` structs (`MarketCreated`, `IntentSignaled`, `StateChanged`,
  `SbaTransfer`, `BondWithdrawn`). The build has no warnings.

## Testnet deployment

Addresses, test tokens, and the market-0 history are in `testnet/deployment.json`.

NOVA (`CCKDS3RAH5T2XYCZWVRPVK7MXYODB4ZGBZX7MNLOQEIWZNHOXOF4JUS4`) and the Soroswap
adapter (`CBUXWJUFVOYJ4TQGXDOHBN7EZFG52CGVZVGWE5E4QCOY2Y5EVLEVEYV4`) run against a
Soroswap router and pool that this project deployed to testnet. Market 0 went
through create, three intents, activation, a real swap that drove it to `Degraded`,
and then a halted transfer.

Known limits of this deployment:

- The oracle is `test_sep40_oracle`, a test-only contract whose price is set by one admin.
  It proves the wiring only. No verified SEP-40 testnet feed was found in the docs, so
  this stack cannot be called production-ready until a real oracle address is verified and allowlisted.
- The Soroswap contracts were built from source and deployed by this project. They are not
  confirmed to be the official Soroswap testnet deployment.
- The testnet deployer key is the local CLI identity `nova-deployer`. The three signer
  keys are `nova-signer-1..3`. None of these are in the repo.

## Frontend (`frontend/`)

A Vite + React + TypeScript terminal for the testnet market. It reads the market,
constitution, capabilities, and `is_eligible()` from the live contract by simulating
read-only calls. Its two actions, sign intent and checkpoint, go through Freighter.

```bash
cd frontend
npm install
npm run dev        # http://localhost:5173
```

What has been checked:

- Reads: `frontend/scripts/read-check.ts` runs the same client functions the UI uses
  against testnet. The UI renders the live market (Degraded, eligibility false).
- Writes: `frontend/scripts/write-check.ts` runs the same prepare, sign, submit, and poll
  path with a local testnet key. A checkpoint succeeded on-chain. An intent on a market past
  Discovery was rejected by the contract, as intended.
- Freighter: not tested in a browser. Its `signTransaction` returns `{ signedTxXdr }`,
  and the client handles that. The wallet flow still needs a manual run with the extension.

The lending vault is deployed on testnet, but the terminal does not drive it. The vault panel
shows the `is_eligible()` check the vault makes. It does not submit deposits or borrows.
The UI also labels the oracle as a test feed, since it is admin-set and not trustless.

## Lending vault (`vault/`)

A minimal lending vault that accepts NOVA State-Bound Assets as collateral. It calls
`is_eligible()` before every deposit and borrow, so an unhealthy market cannot add risk.

| Function | Gate | Notes |
| --- | --- | --- |
| `supply` / `withdraw_supply` | none | Lenders deposit the loan token. Withdrawals are limited by pool cash. |
| `deposit_collateral` | `is_eligible` | Moves SBA into the vault through NOVA's `transfer`. |
| `borrow` | `is_eligible`, LTV | Limit = collateral × oracle price / 1e7 × LTV. |
| `repay` | none | Allowed in every state, so users can always reduce risk. |
| `withdraw_collateral` | debt covered by the remaining collateral | Fails while the market is Degraded, because NOVA halts SBA transfers. |
| `liquidate` | position unhealthy at the oracle price, market Active or Degraded, price > 0 | Liquidator repays up to 50% of debt and receives collateral at oracle price plus the bonus. Seizure uses NOVA's `settle_transfer`. |

Liquidation math: `limit = collateral × price / 1e7 × LTV`. A position is liquidatable when
`debt > limit`. Repay is capped at `ceil(debt × 50%)`. Seized collateral is
`repay × 1e7 / price × (10000 + bonus) / 10000`, capped at the borrower's collateral.

### Degraded-state liquidation: settlement path

Open trading stays halted in Degraded. Liquidation no longer needs it. NOVA has a
`settle_transfer` function for allowlisted lending contracts (the `settlers` list, set
at `initialize`):

- Only the settler's own balance can move. `from` must equal the settler, so a vault can
  release the collateral it holds and never a user's balance.
- It works in Active and Degraded. It refuses Paused, because a dead oracle leaves no valid price.
- Deposits and new borrows still require `is_eligible`. A Degraded market gets no new risk.
- Ordinary user-to-user transfers stay halted in Degraded.

Deployment order, because NOVA and the vault reference each other: register both
contracts, initialize NOVA with the vault in `settlers`, then initialize the vault with
NOVA's address. This changes NOVA's `initialize` signature, so the testnet NOVA must be
redeployed. The old address cannot be reused.

Tests (`integration/`, 9 total) run the vault against the real NOVA, adapter and Soroswap
WASMs. They cover the gate before activation, the LTV limit, the freeze while degraded,
release after liquidity returns, a liquidation with exact amounts after a price drop, a
rejected healthy position, liquidation during Degraded, a blocked liquidation while Paused,
and `settle_transfer` restrictions (unapproved settler, and a settler moving a user's balance).

Known gaps:

- **Degraded liquidation depends on a liquidator who can use the collateral.** The liquidator
  receives SBA that cannot be traded until the market returns to Active. The bonus only
  attracts liquidators if they can sell it, so a long Degraded period can leave bad debt
  unclaimed. A protocol reserve or a market-maker arrangement would close this, and that is
  a design decision.
- **Paused still freezes positions.** With a dead oracle there is no valid price, so there is
  no safe liquidation. This is a deliberate choice, but it means bad debt can build during a
  Paused period.
- **Liquidation trusts the oracle price.** A manipulated or stale price changes who can be
  liquidated. The Soroswap adapter and the oracle allowlist limit this, but they do not remove it.
- **Collateral is locked while Degraded.** The spec says users should be able to unwind in
  Degraded. NOVA's transfer gate blocks it, so this vault freezes collateral until the market
  recovers. Resolving this needs a NOVA change, which is a design decision.
- **No interest.** Lenders earn nothing and borrowers pay nothing.
- **Price units are assumed.** The vault treats 1 SBA unit = oracle price / 1e7 loan units.
  The LTV math depends on that assumption.
- **Vault-as-sender auth is only tested under `mock_all_auths`.** The vault calls NOVA's
  `transfer` with itself as `from`. Confirm that auth works on a live network before
  relying on it.
- **Not deployed to testnet.**

## Current testnet deployment (v2)

Authoritative addresses are in `testnet/deployment.json` (`"current": "v2"`). v2 NOVA is
`CDSN2U5SG3YU27YPNKMKHJEPVFVPVA2BYOWDBYRTC72QPG3HYOVHK5ME`, and the lending vault is
`CCMVZHUOGXOOM6ANLESIDX4S3KRQHKQQOIPIXIXS7UCQJFGOXJQYEV2N`. v1 is kept as
`testnet/deployment-v1-superseded.json`.

v2 runs the full path live: deposit, borrow, a price drop, a Degraded market from a real swap,
and a liquidation that settles through `settle_transfer`. The vault-as-sender auth works on
live testnet. The demo runbook is in `demo/RUNBOOK.md`.
