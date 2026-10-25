#!/usr/bin/env bash
# Refresh the test oracle price so markets stay live. Operator only.
#
# The oracle is admin-set and expires after 24 hours. Once stale, every market pauses for
# everyone. Run this before a demo, and again if the page shows the stale warning.
#
# Usage: demo/refresh-oracle.sh [price]     price in 7 decimals, default 1.0 = 10000000

set -euo pipefail

NETWORK=testnet
PRICE=${1:-10000000}
DEPLOY="$(cd "$(dirname "$0")/.." && pwd)/testnet/deployment.json"
ORACLE=$(jq -r '.v2.contracts.test_sep40_oracle' "$DEPLOY")

stellar contract invoke --id "$ORACLE" --source nova-deployer --network "$NETWORK" -- \
  set_price --price "$PRICE" --timestamp "$(date +%s)" >/dev/null
echo "oracle price set to $PRICE at $(date -u +%H:%M:%SZ)"
