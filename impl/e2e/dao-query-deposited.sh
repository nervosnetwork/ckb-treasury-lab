#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/env.sh"

echo "==> Querying deposited DAO cells for ${FROM_ADDRESS}..."
ckb-cli --url "$CKB_RPC" dao query-deposited-cells \
  --address "$FROM_ADDRESS"
