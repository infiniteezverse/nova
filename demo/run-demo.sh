#!/usr/bin/env bash
# NOVA v2 live demo on Stellar testnet. Rerunnable end to end.
#
# Each run creates a NEW market (the id is read from the chain), so reruns never collide.
# Contract addresses come from testnet/deployment.json (.v2). Any mismatch stops the run.
# The Soroswap pool is shared across runs, so the script tops up depth when it's low, and it
# swaps in steps until the market is actually Degraded instead of assuming fixed sizes.
#
# Usage:  demo/run-demo.sh
# Needs:  stellar CLI, jq, and identities nova-deployer and nova-signer-1..3 on this machine.
# Writes: demo/last-run.json

set -euo pipefail

NETWORK=testnet
HERE=$(cd "$(dirname "$0")" && pwd)
NOVA_DIR=$(cd "$HERE/.." && pwd)
DEPLOY="$NOVA_DIR/testnet/deployment.json"
RESULTS="$HERE/last-run.json"

DEPLOYER=nova-deployer
SIGNERS=(nova-signer-1 nova-signer-2 nova-signer-3)
LIQUIDATOR=nova-signer-2
DEADLINE=4102444800

# Must match the constitution and vault settings used in the v2 deployment.
BONUS_BPS=500
CLOSE_FACTOR_BPS=5000
MIN_DEPTH=9000000000
COLLATERAL=100000
BORROW=50000
REPAY_REQUEST=60000
LENDER_LIQUIDITY=500000
PRICE_ON=10000000        # 1.0 at 7 decimals
PRICE_DROP=6000000       # 0.6
SWAP_CHUNK=2000000000    # base units per swap step while degrading
MAX_SWAP_STEPS=12
POOL_MARGIN=1000000000       # clear the threshold by this much before activation
POOL_TOPUP_AMOUNT=8000000000
MAX_TOPUPS=5
TOKEN_MIN=30000000000
TOKEN_TOPUP=100000000000

say() { printf '\n== %s\n' "$*"; }
ok()  { printf '   ok: %s\n' "$*"; }
die() { printf '\nFAILED: %s\n' "$*" >&2; exit 1; }

command -v stellar >/dev/null || die "stellar CLI not found"
command -v jq >/dev/null || die "jq not found"
[ -f "$DEPLOY" ] || die "missing $DEPLOY"

NOVA=$(jq -r '.v2.contracts.nova' "$DEPLOY")
VAULT=$(jq -r '.v2.contracts.lending_vault' "$DEPLOY")
ADAPTER=$(jq -r '.v2.contracts.soroswap_adapter' "$DEPLOY")
ORACLE=$(jq -r '.v2.contracts.test_sep40_oracle' "$DEPLOY")
ROUTER=$(jq -r '.v2.contracts.soroswap_router' "$DEPLOY")
BASE=$(jq -r '.v2.tokens.base_TLITH' "$DEPLOY")
LOAN=$(jq -r '.v2.tokens.quote_TUSDC_loan_token' "$DEPLOY")
BOND=$(jq -r '.v2.tokens.bond_TBOND' "$DEPLOY")

DEPLOYER_ADDR=$(stellar keys address "$DEPLOYER")
LIQUIDATOR_ADDR=$(stellar keys address "$LIQUIDATOR")

# call writes its results to globals. It must never run in a $(...) subshell,
# because that would lose LAST_TX and RET.
#   RET     the last line of the CLI output (the return value, JSON-quoted for strings)
#   LAST_TX the transaction hash, if the call was a write
RET=""
LAST_TX=""
call() {
  local src=$1 id=$2 fn=$3; shift 3
  local out
  if ! out=$(stellar contract invoke --id "$id" --source "$src" --network "$NETWORK" -- "$fn" "$@" 2>&1); then
    printf '%s\n' "$out" >&2
    die "call failed: $fn on $id"
  fi
  if printf '%s' "$out" | grep -q "❌"; then
    printf '%s\n' "$out" >&2
    die "call reported an error: $fn"
  fi
  LAST_TX=$(printf '%s' "$out" | grep -oE 'tx/[0-9a-f]{64}' | tail -1 | cut -d/ -f2 || true)
  RET=$(printf '%s\n' "$out" | grep -v '^[[:space:]]*$' | tail -1)
}

# Strip JSON quotes from a scalar return value, e.g. "Active" -> Active.
scalar() { tr -d '"' <<<"$1"; }

# Expect a command to fail. If a pattern is given, the failure text must match it.
expect_fail() {
  local label=$1 pattern=$2; shift 2
  local out
  if out=$("$@" 2>&1); then
    die "expected a rejection ($label) but it succeeded"
  fi
  if [ -n "$pattern" ] && ! printf '%s' "$out" | grep -qE "$pattern"; then
    printf '%s\n' "$out" | tail -5 >&2
    die "rejection ($label) did not match: $pattern"
  fi
  ok "$label rejected as expected"
}

ensure_tokens() {
  local tok=$1 name=$2 bal
  call "$DEPLOYER" "$tok" balance --id "$DEPLOYER_ADDR"; bal=$(scalar "$RET")
  if [ "$bal" -lt "$TOKEN_MIN" ]; then
    call "$DEPLOYER" "$tok" mint --to "$DEPLOYER_ADDR" --amount "$TOKEN_TOPUP"
    ok "topped up $name (was $bal)"
  fi
}

observe_json() { call "$DEPLOYER" "$NOVA" observe --market_id "$1"; printf '%s' "$RET"; }

say "Preflight"
for s in "${SIGNERS[@]}" "$LIQUIDATOR"; do stellar keys address "$s" >/dev/null || die "missing identity $s"; done
ensure_tokens "$BASE" "base TLITH"
ensure_tokens "$LOAN" "loan TUSDC"
ensure_tokens "$BOND" "bond TBOND"
ok "NOVA $NOVA"
ok "vault $VAULT"

say "Pool depth (must clear the threshold before any market is created)"
POOL_TARGET=$(( MIN_DEPTH + POOL_MARGIN ))
for attempt in $(seq 1 "$MAX_TOPUPS"); do
  call "$DEPLOYER" "$ADAPTER" snapshot --market_id 0
  POOL_DEPTH=$(jq -r '.depth' <<<"$RET")
  if [ "$POOL_DEPTH" -ge "$POOL_TARGET" ]; then
    ok "depth $POOL_DEPTH clears the $POOL_TARGET target"
    break
  fi
  call "$DEPLOYER" "$ROUTER" add_liquidity --token_a "$BASE" --token_b "$LOAN" \
    --amount_a_desired "$POOL_TOPUP_AMOUNT" --amount_b_desired "$POOL_TOPUP_AMOUNT" \
    --amount_a_min 0 --amount_b_min 0 --to "$DEPLOYER_ADDR" --deadline "$DEADLINE"
  ok "top-up $attempt: depth was $POOL_DEPTH"
  [ "$attempt" -lt "$MAX_TOPUPS" ] || die "pool depth stays below $POOL_TARGET after $MAX_TOPUPS top-ups"
done

say "Vault liquidity"
call "$DEPLOYER" "$VAULT" cash
CASH=$(scalar "$RET")
if [ "$CASH" -lt "$BORROW" ]; then
  call "$DEPLOYER" "$VAULT" supply --lender "$DEPLOYER_ADDR" --amount "$LENDER_LIQUIDITY"
  ok "lender supplied $LENDER_LIQUIDITY (cash was $CASH)"
else
  ok "vault cash $CASH covers the borrow"
fi

say "Create market"
call "$DEPLOYER" "$ORACLE" set_price --price "$PRICE_ON" --timestamp "$(date +%s)"
call "$DEPLOYER" "$NOVA" create_market --creator "$DEPLOYER_ADDR" --symbol LITH \
  --oracle "$ORACLE" --oracle_asset "{\"Stellar\":\"$BASE\"}" --liquidity "$ADAPTER" \
  --constitution "{\"max_oracle_age_secs\":86400,\"max_spread_bps\":350,\"min_depth\":\"$MIN_DEPTH\",\"target_signers\":3}" \
  --bond_token "$BOND" --bond_amount '10000000000' --lock_secs 86400 --supply '1000000'
MARKET=$RET
ok "market $MARKET created"
PRE=$(jq -r '.health' <<<"$(observe_json "$MARKET")")
[ "$PRE" = "Active" ] || die "the pool cannot support the market before activation (health $PRE)"
ok "pool health before intents: $PRE"

say "Three signed intents, then checkpoint to Active"
for s in "${SIGNERS[@]}"; do
  call "$s" "$NOVA" signal_intent --market_id "$MARKET" --signer "$(stellar keys address "$s")"
done
[ "$RET" = "3" ] || die "expected 3 signers, got $RET"
ok "3 unique signers"
call "$DEPLOYER" "$ORACLE" set_price --price "$PRICE_ON" --timestamp "$(date +%s)"
call "$DEPLOYER" "$NOVA" checkpoint --market_id "$MARKET"
STATE=$(scalar "$RET")
[ "$STATE" = "Active" ] || die "expected Active after checkpoint, got $STATE"
ok "state Active"

say "Lending: deposit and borrow"
call "$DEPLOYER" "$VAULT" deposit_collateral --user "$DEPLOYER_ADDR" --market_id "$MARKET" --amount "$COLLATERAL"
call "$DEPLOYER" "$VAULT" borrow --user "$DEPLOYER_ADDR" --market_id "$MARKET" --amount "$BORROW"
call "$DEPLOYER" "$VAULT" position --user "$DEPLOYER_ADDR" --market_id "$MARKET"
[ "$(jq -r .collateral <<<"$RET")" = "$COLLATERAL" ] || die "collateral mismatch: $RET"
[ "$(jq -r .debt <<<"$RET")" = "$BORROW" ] || die "debt mismatch: $RET"
ok "collateral $COLLATERAL, debt $BORROW"

say "Price drop to 0.6 (the position is now underwater)"
call "$DEPLOYER" "$ORACLE" set_price --price "$PRICE_DROP" --timestamp "$(date +%s)"
ok "oracle price $PRICE_DROP"

say "Liquidity shock: swap in steps until the market is Degraded"
HEALTH=""
DEPTH=""
for step in $(seq 1 "$MAX_SWAP_STEPS"); do
  call "$DEPLOYER" "$ROUTER" swap_exact_tokens_for_tokens --amount_in "$SWAP_CHUNK" --amount_out_min 0 \
    --path "[\"$BASE\",\"$LOAN\"]" --to "$DEPLOYER_ADDR" --deadline "$DEADLINE"
  OBS=$(observe_json "$MARKET")
  HEALTH=$(jq -r '.health' <<<"$OBS")
  DEPTH=$(jq -r '.depth' <<<"$OBS")
  ok "swap $step: depth $DEPTH, health $HEALTH"
  [ "$HEALTH" = "Degraded" ] && break
done
[ "$HEALTH" = "Degraded" ] || die "the market did not degrade within $MAX_SWAP_STEPS swaps"
call "$DEPLOYER" "$NOVA" checkpoint --market_id "$MARKET"
STATE=$(scalar "$RET")
[ "$STATE" = "Degraded" ] || die "checkpoint did not record Degraded (got $STATE)"
[ "$DEPTH" -lt "$MIN_DEPTH" ] || die "depth $DEPTH is not below $MIN_DEPTH"
ok "state Degraded, depth $DEPTH is below $MIN_DEPTH"

say "Degraded: a new deposit is refused"
expect_fail "deposit while Degraded" "" \
  stellar contract invoke --id "$VAULT" --source "$DEPLOYER" --network "$NETWORK" -- \
  deposit_collateral --user "$DEPLOYER_ADDR" --market_id "$MARKET" --amount 1000

say "Liquidation while Degraded (the settlement path)"
call "$DEPLOYER" "$LOAN" transfer --from "$DEPLOYER_ADDR" --to "$LIQUIDATOR_ADDR" --amount 100000
call "$LIQUIDATOR" "$VAULT" liquidate --liquidator "$LIQUIDATOR_ADDR" --borrower "$DEPLOYER_ADDR" \
  --market_id "$MARKET" --repay_amount "$REPAY_REQUEST"
LIQ=$RET
LIQ_TX=$LAST_TX
[ -n "$LIQ_TX" ] || die "liquidation returned no transaction hash"

# Expected values use the same integer math as the contract. The debt is a whole number,
# so the close-factor cap is exact.
EXPECT_REPAID=$(( BORROW * CLOSE_FACTOR_BPS / 10000 ))
EXPECT_SEIZED=$(( EXPECT_REPAID * 10000000 / PRICE_DROP * (10000 + BONUS_BPS) / 10000 ))
REPAID=$(jq -r '.[0]' <<<"$LIQ")
SEIZED=$(jq -r '.[1]' <<<"$LIQ")
[ "$REPAID" = "$EXPECT_REPAID" ] || die "repaid $REPAID, expected $EXPECT_REPAID"
[ "$SEIZED" = "$EXPECT_SEIZED" ] || die "seized $SEIZED, expected $EXPECT_SEIZED"
ok "repaid $REPAID, seized $SEIZED (tx $LIQ_TX)"

call "$DEPLOYER" "$VAULT" position --user "$DEPLOYER_ADDR" --market_id "$MARKET"
[ "$(jq -r .debt <<<"$RET")" = "$(( BORROW - EXPECT_REPAID ))" ] || die "debt after liquidation: $RET"
[ "$(jq -r .collateral <<<"$RET")" = "$(( COLLATERAL - EXPECT_SEIZED ))" ] || die "collateral after liquidation: $RET"
call "$DEPLOYER" "$NOVA" balance --market_id "$MARKET" --who "$LIQUIDATOR_ADDR"
LIQ_BAL=$(scalar "$RET")
[ "$LIQ_BAL" = "$EXPECT_SEIZED" ] || die "liquidator SBA $LIQ_BAL, expected $EXPECT_SEIZED"
ok "position and liquidator balances match"

say "Degraded: the liquidator's ordinary transfer is halted"
expect_fail "liquidator open transfer" 'Error\(Contract, #9\)' \
  stellar contract invoke --id "$NOVA" --source "$LIQUIDATOR" --network "$NETWORK" -- \
  transfer --market_id "$MARKET" --from "$LIQUIDATOR_ADDR" --to "$DEPLOYER_ADDR" --amount 1

say "Write results"
jq -n --arg market "$MARKET" --arg repaid "$REPAID" --arg seized "$SEIZED" --arg tx "$LIQ_TX" \
  --arg nova "$NOVA" --arg vault "$VAULT" --arg at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  '{network:"testnet", at:$at, nova:$nova, vault:$vault, market:$market,
    liquidation:{repaid:$repaid, seized:$seized, tx:$tx}, result:"PASS"}' > "$RESULTS"
ok "wrote $RESULTS"
printf '\nPASS: market %s liquidated while Degraded (tx %s)\n' "$MARKET" "$LIQ_TX"
