#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/env.sh"
AMOUNT="${1:?Usage: $0 <amount_ckb>}"

echo "==> Depositing ${AMOUNT} CKB into NervosDAO from ${FROM_ADDRESS}..."
ckb-cli --url "$CKB_RPC" dao deposit \
  --from-account "$FROM_ADDRESS" \
  --capacity "$AMOUNT" \
  --fee-rate 1000
