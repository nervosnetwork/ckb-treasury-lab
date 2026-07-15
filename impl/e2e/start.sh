#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEVNET_DIR="$(cd "$SCRIPT_DIR/../devnet" && pwd)"

echo "ckb:     $DEVNET_DIR/ckb"
echo "ckb-cli: $(which ckb-cli)"
$DEVNET_DIR/ckb --version
ckb-cli --version

if pgrep -f "ckb miner" > /dev/null 2>&1; then
  echo "WARNING: A 'ckb miner' process is already running. Please stop it before starting a new one."
  exit 1
fi

$DEVNET_DIR/ckb miner -C "$DEVNET_DIR" >/dev/null 2>&1 &

exec $DEVNET_DIR/ckb run -C "$DEVNET_DIR"
