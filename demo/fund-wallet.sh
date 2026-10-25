#!/usr/bin/env bash
# Fund a Freighter wallet for the live demo on testnet. Operator only.
#
# Usage: demo/fund-wallet.sh <G-address>
# Sends test tokens from the deployer, who is the token admin, and tops up XLM from Friendbot.
# The amounts cover several runs: 3,000 TBOND for bonds and 1,200 TLITH for swaps.
# Nothing here has value. Never point this at mainnet.

set -euo pipefail

[ $# -eq 1 ] || { echo "usage: $0 <G-address>" >&2; exit 1; }
WALLET=$1
case "$WALLET" in G*) ;; *) echo "expected a G... address" >&2; exit 1 ;; esac

NETWORK=testnet
DEPLOYER=nova-deployer
DEPLOY="$(cd "$(dirname "$0")/.." && pwd)/testnet/deployment.json"
BOND=$(jq -r '.v2.tokens.bond_TBOND' "$DEPLOY")
BASE=$(jq -r '.v2.tokens.base_TLITH' "$DEPLOY")
DEPLOYER_ADDR=$(stellar keys address "$DEPLOYER")

echo "funding XLM via Friendbot"
curl -s -o /dev/null -w "   friendbot HTTP %{http_code}\n" "https://friendbot.stellar.org/?addr=$WALLET" || true

for pair in "$BOND:30000000000:TBOND" "$BASE:12000000000:TLITH"; do
  IFS=: read -r token amount name <<<"$pair"
  stellar contract invoke --id "$token" --source "$DEPLOYER" --network "$NETWORK" -- \
    transfer --from "$DEPLOYER_ADDR" --to "$WALLET" --amount "$amount" >/dev/null
  echo "   sent $amount $name"
done
echo "done. Check the wallet balances on the page."
