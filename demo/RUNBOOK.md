# NOVA demo runbook (testnet, v2)

The live sequence is recorded in `testnet/deployment.json` (v2). Every step below ran on Stellar testnet.

## Live sequence

1. **Market.** Market 0 is created on NOVA v2 with a 1,000-unit bond.
2. **Gate.** Three signed intents move it to Ready. A checkpoint moves it to Active.
3. **Lending.** 100,000 SBA is deposited as collateral. 50,000 is borrowed.
4. **Price shock.** The test oracle drops to 0.6. The position is underwater.
5. **Liquidity shock.** A live Soroswap swap of 8e9 base drops quote depth to 7.4e9, below the 9e9 threshold. A checkpoint moves the market to Degraded.
6. **Settlement.** A liquidator repays 25,000 (the 50% close factor) and receives 43,749 SBA. Tx `54b2b5436e29ef0982de7adc102f0b34133e64a2777a82ab0fac5280e5ce589f`.
7. **Halt.** The liquidator's ordinary transfer is rejected with `TransferHalted` (#9).

## Talk track (what to say, and what not to say)

- **Hook.** NOVA is a State-Bound Asset protocol. An asset's utility is tied to on-chain health checks. It is not a launchpad or a DEX.
- **Architecture.** The constitution runs in Soroban. There is no admin key. The allowlists are set once at deploy time. Show the live constitution panel.
- **Proof, step 5 to 7.** A real swap shrinks the pool. NOVA degrades the market and halts retail trading. Only the allowlisted vault can settle. Ordinary transfers still fail.
- **Composable shield.** `is_eligible()` decides whether the vault accepts new deposits or borrows. A Degraded market gets no new risk.

**Do not say:**
- "Production-ready" or "bulletproof." The oracle is a test feed. Freighter signing hasn't been tested in a browser.
- "Trustless oracle." The feed is admin-set. The UI says so, and the demo should too.
- "Insolvency gap closed." Settlement works while Degraded, but it depends on a liquidator willing to hold SBA that can't be sold until recovery. Paused positions stay frozen.

## Known limits to name on camera

- Test oracle, admin-set.
- Liquidation incentive depends on the market recovering.
- Paused freezes liquidation by design.
- Soroswap contracts were built and deployed by this project, not the official testnet deployment.

## Rerun the full sequence (scripted)

```bash
bash demo/run-demo.sh
```

The script reads the v2 addresses from `testnet/deployment.json`. It needs the `stellar` CLI,
`jq`, and the identities `nova-deployer` and `nova-signer-1..3` on the machine. It takes about
2 minutes. Each run does the following:

- Tops up pool depth in a loop until it clears the 9e9 threshold by a margin, before creating
  a market. This keeps a run from leaving a market that can never activate.
- Creates a new market. Market ids keep counting up, so reruns never collide.
- Swaps in steps until the market is actually Degraded, instead of assuming a fixed swap size.
- Checks every number against the expected values: repaid, seized, debt, collateral, and the
  liquidator's balance. It fails if any number is off.
- Confirms that a new deposit is refused and the liquidator's ordinary transfer returns `#9`.
- Writes `demo/last-run.json` with the market id and the liquidation transaction hash.

Notes:

- Each run locks a 1,000-unit bond for one day. The deployer's test tokens are topped up
  automatically, since the deployer is the token admin.
- A failed run can leave a market behind. Market 1 is from a first attempt that failed
  the depth check before the top-up loop was added. It is not used.
- The script is a testnet tool. It has no mainnet guard beyond the network being hardcoded.
