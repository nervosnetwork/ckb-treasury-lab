#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
SDK_DIR="$REPO_ROOT/impl/sdk"

source "$SCRIPT_DIR/env.sh"

DURATION="${DURATION:-3}"
DESCRIPTION="${1:-test1}"
PK_FILE="$SCRIPT_DIR/pk1"

# Poll until a TX is committed on-chain; prints its decimal block number to stdout.
poll_tx_committed() {
  local tx_hash="$1"
  local deadline=$(( SECONDS + 60 ))
  while [ "$SECONDS" -lt "$deadline" ]; do
    local resp
    resp=$(curl -s "$CKB_RPC" -X POST -H 'Content-Type: application/json' \
      -d "{\"jsonrpc\":\"2.0\",\"method\":\"get_transaction\",\"params\":[\"$tx_hash\"],\"id\":1}")
    local status
    status=$(echo "$resp" | python3 -c \
      "import json,sys; r=(json.load(sys.stdin).get('result') or {}); print(r.get('tx_status',{}).get('status','none'))" \
      2>/dev/null || echo "none")
    if [ "$status" = "committed" ]; then
      echo "$resp" | python3 -c \
        "import json,sys; print(int(json.load(sys.stdin)['result']['tx_status']['block_number'],16))" \
        2>/dev/null
      return 0
    fi
    sleep 0.5
  done
  echo "ERROR: TX $tx_hash not committed within 60 s" >&2
  return 1
}

# Fetch the hash of a block by its decimal block number.
get_block_hash() {
  local block_num="$1"
  local hex_num
  hex_num=$(printf '0x%x' "$block_num")
  curl -s "$CKB_RPC" -X POST -H 'Content-Type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"method\":\"get_block_by_number\",\"params\":[\"$hex_num\"],\"id\":1}" \
    | python3 -c "import json,sys; print(json.load(sys.stdin)['result']['header']['hash'])" \
    2>/dev/null
}

# Poll until the chain tip is at or beyond target_block.
wait_for_block() {
  local target="$1"
  while true; do
    local tip
    tip=$(curl -s "$CKB_RPC" -X POST -H 'Content-Type: application/json' \
      -d '{"jsonrpc":"2.0","method":"get_tip_block_number","params":[],"id":1}' \
      | python3 -c "import json,sys; print(int(json.load(sys.stdin)['result'],16))" \
      2>/dev/null || echo "0")
    if [ "$tip" -ge "$target" ]; then
      return 0
    fi
    sleep 0.5
  done
  return 1
}

echo "==> Creating proposal (duration=$DURATION, description=\"$DESCRIPTION\")..."
create_output=$(cd "$SDK_DIR" && pnpm dev create-proposal \
  --private-key-file "$PK_FILE" \
  --duration "$DURATION" \
  --description "$DESCRIPTION")
echo "$create_output"

PROPOSAL_TX_HASH=$(echo "$create_output" | grep 'outpoint:' | sed 's/.*outpoint:[[:space:]]*//' | cut -d: -f1)
PROPOSAL_TX_INDEX=$(echo "$create_output" | grep 'outpoint:' | sed 's/.*outpoint:[[:space:]]*//' | cut -d: -f2)
if [[ -z "$PROPOSAL_TX_HASH" ]]; then
  echo "ERROR: failed to parse proposal tx hash from output" >&2
  exit 1
fi
echo ""
echo "  proposal tx hash:  $PROPOSAL_TX_HASH"
echo "  proposal tx index: $PROPOSAL_TX_INDEX"

echo ""
echo "==> Waiting for proposal TX to be committed..."
PROPOSAL_BLOCK=$(poll_tx_committed "$PROPOSAL_TX_HASH")
echo "  proposal committed in block $PROPOSAL_BLOCK"
START_BLOCK_HASH=$(get_block_hash "$PROPOSAL_BLOCK")
echo "  start block hash:  $START_BLOCK_HASH"
END_BLOCK=$(( PROPOSAL_BLOCK + DURATION ))

echo ""
echo "==> Voting yes on proposal $PROPOSAL_TX_HASH..."
(cd "$SDK_DIR" && pnpm dev vote \
  --private-key-file "$PK_FILE" \
  --proposal-tx-hash "$PROPOSAL_TX_HASH" \
  --vote yes)

echo ""
echo "==> Waiting for voting window to close (need tip ≥ block $END_BLOCK)..."
wait_for_block "$END_BLOCK"
echo "  tip reached block $END_BLOCK"
END_BLOCK_HASH=$(get_block_hash "$END_BLOCK")
echo "  end block hash:    $END_BLOCK_HASH"

INFO_FILE="$SCRIPT_DIR/info.txt"
cat > "$INFO_FILE" <<EOF
proposal_tx_hash=$PROPOSAL_TX_HASH
proposal_tx_index=$PROPOSAL_TX_INDEX
start_block_hash=$START_BLOCK_HASH
end_block_hash=$END_BLOCK_HASH
EOF
echo ""
echo "  info saved to $INFO_FILE"

echo ""
echo "==> Consuming proposal $PROPOSAL_TX_HASH..."
(cd "$SDK_DIR" && pnpm dev consume-proposal \
  --private-key-file "$PK_FILE" \
  --proposal-tx-hash "$PROPOSAL_TX_HASH" \
  --proposal-index "$PROPOSAL_TX_INDEX" \
  --start-block-hash "$START_BLOCK_HASH" \
  --end-block-hash "$END_BLOCK_HASH")
