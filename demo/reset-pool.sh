#!/usr/bin/env bash
# Restore pool depth after a demo swap so the next run can activate. Operator only.
#
# Each degrade swap leaves the shared Soroswap pool shallow, and a new market then can't
# activate. This adds liquidity with the deployer until quote depth clears the threshold.
# Usage: demo/reset-pool.sh

set -euo pipefail

NETWORK=testnet
TARGET=10000000000          # clear the 9e9 threshold with margin
DEPLOY="$(cd "$(dirname "$0")/.." && pwd)/testnet/deployment.json"
ADAPTER=$(jq -r '.v2.contracts.soroswap_adapter' "$DEPLOY")
ROUTER=$(jq -r '.v2.contracts.soroswap_router' "$DEPLOY")
BASE=$(jq -r '.v2.tokens.base_TLITH' "$DEPLOY")
LOAN=$(jq -r '.v2.tokens.quote_TUSDC_loan_token' "$DEPLOY")
ME=$(stellar keys address nova-deployer)

for attempt in 1 2 3 4 5; do
  DEPTH=$(stellar contract invoke --id "$ADAPTER" --source nova-deployer --network "$NETWORK" -- \
    snapshot --market_id 0 2>/dev/null | grep '^{' | tail -1 | jq -r .depth)
  echo "pool depth $DEPTH"
  if [ "$DEPTH" -ge "$TARGET" ]; then
    echo "pool ready"
    exit 0
  fi
  stellar contract invoke --id "$ROUTER" --source nova-deployer --network "$NETWORK" -- add_liquidity \
    --token_a "$BASE" --token_b "$LOAN" --amount_a_desired 8000000000 --amount_b_desired 8000000000 \
    --amount_a_min 0 --amount_b_min 0 --to "$ME" --deadline 4102444800 >/dev/null
done
echo "pool still below $TARGET after 5 top-ups" >&2
exit 1
