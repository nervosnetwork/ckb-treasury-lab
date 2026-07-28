#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/env.sh"

BLOCK=$(ckb-cli --url "$CKB_RPC" rpc get_block_by_number --number 0 --output-format json 2>/dev/null)

TX0=$(echo "$BLOCK" | python3 -c "import json,sys; txs=json.load(sys.stdin)['transactions']; print(txs[0]['hash'])")
TX1=$(echo "$BLOCK" | python3 -c "import json,sys; txs=json.load(sys.stdin)['transactions']; print(txs[1]['hash'])")

echo "GENESIS_TX0=$TX0"
echo "GENESIS_TX1=$TX1"
