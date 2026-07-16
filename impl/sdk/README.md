# CKB Vote SDK

Off-chain TypeScript SDK for the CKB on-chain voting system. Provides an API and CLI to create proposals, cast votes, and settle passed proposals.

Built on [CCC](https://github.com/ckb-devrel/ccc) (`@ckb-ccc/shell`).

## Architecture

- **Proposal type script** — embedded in the CKB node (like Type ID). No on-chain binary code required. The node performs vote tally natively from its own block storage.
- **Vote type script** — on-chain script referenced by `cell_dep`. Verified by CKB-VM.
- **Settlement** — triggered by a transaction with two `header_deps` (start and end blocks). No witness, no proof needed.

## Installation

```sh
cd impl/sdk
pnpm install    # or  bun install
```

## Building

### TypeScript (tsc) — library build with declarations

```sh
pnpm build      # or  bun run build
```

This produces `dist/` with `.js`, `.d.ts`, and `.js.map` files suitable for publishing.

### Bun build — fast library bundle

```sh
bun run build:bun
```

Generates type declarations via `tsc --emitDeclarationOnly` then bundles the SDK into a single `dist/index.js` with `bun build`.

## CLI Usage

Run directly without a build step:

```sh
# with Bun — runs .ts files natively:
bun run dev:bun <command> [options]
bun run src/cli/index.ts <command> [options]
# or after `bun link`:
ckb-vote <command> [options]

# with Node — uses tsx for on-the-fly execution:
pnpm dev <command> [options]
./node_modules/.bin/tsx src/cli/index.ts <command> [options]
# or after `pnpm link --global`:
ckb-vote <command> [options]
```

The `bin/ckb-vote.js` entry point auto-detects the runtime (Bun vs Node) and runs the CLI accordingly.

### Global options

All commands share:

| Option            | Description                                                  |
| ----------------- | ------------------------------------------------------------ |
| `--rpc-url <url>` | CKB RPC endpoint (default: `http://127.0.0.1:8114`)          |
| `--config <path>` | JSON NetworkConfig file (defaults to built-in devnet config) |

The `--config` file has the same shape as `NetworkConfig`:

```json
{
  "ckbRpcUrl": "http://127.0.0.1:8114",
  "alwaysSuccess": {
    "codeHash": "0x...",
    "hashType": "type",
    "outPoint": { "txHash": "0x...", "index": 0 }
  },
  "proposalTypeScript": {
    "codeHash": "0x...",
    "hashType": "type",
    "outPoint": { "txHash": "0x...", "index": 0 }
  },
  "voteTypeScript": {
    "codeHash": "0x...",
    "hashType": "type",
    "outPoint": { "txHash": "0x...", "index": 0 }
  },
  "feeRate": 1500,
  "knownScripts": {
    "Secp256k1Blake160": { ... }
  }
}
```

`--rpc-url` takes precedence over `config.ckbRpcUrl`.

### Quick Start (devnet)

```sh
# Create a proposal (20-block window)
bun run dev:bun create-proposal \
  --private-key-file ../tools/e2e/pk1 \
  --duration 20 \
  --description "test1"
# or with pnpm:
pnpm dev create-proposal \
  --private-key-file ../tools/e2e/pk1 \
  --duration 20 \
  --description "test1"

# Vote YES on the proposal
bun run dev:bun vote \
  --private-key-file ../tools/e2e/pk1 \
  --proposal-tx-hash 0x<TX_HASH_FROM_ABOVE> \
  --vote yes

# Consume / settle after voting window closes
bun run dev:bun consume-proposal \
  --private-key-file ../tools/e2e/pk1 \
  --proposal-tx-hash 0x<TX_HASH_FROM_ABOVE> \
  --start-block-hash 0x<START> \
  --end-block-hash 0x<END>
```

---

### `create-proposal`

Create a proposal cell on-chain. The proposal type script args are a 20-byte blake160 Type ID, computed automatically from the first input's outpoint and the output index.

```sh
ckb-vote create-proposal \
  --private-key-file ./my-key.txt \
  --duration 8640 \
  --description "Fund infrastructure work Q3 2026" \
  [--receiver ckt1qzda0cr...] \
  [--amount 1000] \
  [--minimal-requirement 5000] \
  [--rpc-url http://127.0.0.1:8114] \
  [--config ./network.json]
```

| Option                  | Description                                                            |
| ----------------------- | ---------------------------------------------------------------------- |
| `--private-key-file`    | Path to hex private key file                                           |
| `--duration`            | Voting window in blocks (~10s/block)                                   |
| `--description`         | Plain-text description                                                 |
| `--receiver`            | CKB address for funds if proposal passes; defaults to signer's address |
| `--amount`              | CKB to transfer on success; defaults to 500                            |
| `--minimal-requirement` | Minimum CKB vote weight to pass; defaults to 10000                     |

Output:

```
Proposal created successfully.
  outpoint:     0x...:0
  duration:     20
  code_hash:    0x...
  hash_type:    type
  args:         0x... (20-byte Type ID)
```

---

### `vote`

Cast a YES or NO vote on a proposal. All DAO deposit cells owned by the signer are discovered automatically and recorded in `dao_index`.

```sh
ckb-vote vote \
  --private-key-file ./my-key.txt \
  --proposal-tx-hash 0xABC... \
  --vote yes \
  [--proposal-index 0] \
  [--rpc-url http://127.0.0.1:8114] \
  [--config ./network.json]
```

| Option               | Description                                    |
| -------------------- | ---------------------------------------------- |
| `--private-key-file` | Path to hex private key file                   |
| `--proposal-tx-hash` | Tx hash of the proposal cell                   |
| `--vote`             | `yes` or `no`                                  |
| `--proposal-index`   | Output index of the proposal cell (default: 0) |

Output:

```
Vote (yes) submitted successfully.
  vote cell:  0x...:0
  capacity voted: N CKB
```

---

### `consume-vote`

Recycle a vote cell at any time to reclaim its occupied CKB.

```sh
ckb-vote consume-vote \
  --private-key-file ./my-key.txt \
  --vote-tx-hash 0xABC... \
  [--vote-index 0]
```

---

### `consume-proposal`

Settle a passed proposal. The node performs the vote tally natively using its embedded proposal type script. No SP1 proof or witness is required — the transaction just needs two `header_deps` to define the voting block range.

```sh
ckb-vote consume-proposal \
  --private-key-file ./my-key.txt \
  --proposal-tx-hash 0xABC... \
  --start-block-hash 0xSTART... \
  --end-block-hash 0xEND... \
  [--proposal-index 0]
```

| Option               | Description                                                       |
| -------------------- | ----------------------------------------------------------------- |
| `--proposal-tx-hash` | Tx hash of the proposal cell                                      |
| `--start-block-hash` | Hash of the block containing the proposal cell (`header_deps[0]`) |
| `--end-block-hash`   | Hash of the end block (start + duration, `header_deps[1]`)        |

---

## API Usage

```typescript
import { ccc } from "@ckb-ccc/shell";
import {
  buildClient,
  createProposal,
  createVote,
  consumeProposal,
  loadNetworkConfig,
  DEVNET_CONFIG,
} from "@ckb-vote/sdk";

const client = buildClient("http://127.0.0.1:8114");
const signer = new ccc.SignerCkbPrivateKey(client, "0xYOUR_PRIVATE_KEY");

// Create a proposal
const { txHash, proposalOutPoint } = await createProposal(signer, {
  duration: 8640,
  description: "Fund Q3 infrastructure work",
  amount: 1000n * 100_000_000n, // 1000 CKB in shannon
  minimalRequirement: 5000n * 100_000_000n,
});

// Vote YES (DAO deposits are auto-discovered)
const voteResult = await createVote(signer, {
  proposalOutPoint,
  vote: "yes",
});

// Consume / settle after voting window closes
const { txHash: settleTx } = await consumeProposal(signer, {
  proposalOutPoint,
  startBlockHash: "0xSTART_HASH...",
  endBlockHash: "0xEND_HASH...",
});
```

---

## Network Configuration

The SDK ships with `DEVNET_CONFIG` for the local devnet. To use a different network, either:

**Option A — pass a config override:**

```typescript
import { DEVNET_CONFIG, type NetworkConfig } from "@ckb-vote/sdk";

await createProposal(signer, {
  duration: 8640,
  description: "...",
  config: {
    ckbRpcUrl: "https://testnet.ckb.dev",
    proposalTypeScript: { codeHash: "0xTEST...", hashType: "type", outPoint: { ... } },
    voteTypeScript: { codeHash: "0xTEST...", hashType: "type", outPoint: { ... } },
  },
});
```

**Option B — load from a JSON file:**

```typescript
import { loadNetworkConfig } from "@ckb-vote/sdk";

const config = loadNetworkConfig("./testnet-config.json");

await createProposal(signer, { ..., config });
```

The JSON file may contain a partial config — only fields present in the file override the devnet defaults.

---

## Transaction Structure

### Create Proposal

```
Inputs:   [signer cells]
Outputs:  [0] Proposal cell
              lock: always-success
              type: proposal-type-script (args = 20-byte blake160 Type ID)
              data: molecule-encoded Proposal
Cell deps: always-success contract
```

### Vote

```
Cell deps: [0] vote-type-script contract
           [1] proposal cell
           [2..] DAO deposit cells  ← dao_index points here
Inputs:   [signer cells for fee]
Outputs:  [0] Vote cell
              lock: voter's lock
              type: vote-type-script (args = blake160(proposalTypeScript))
              data: molecule-encoded Vote {vote, amount, dao_index}
```

### Consume Proposal

```
Inputs:   [0] Proposal cell (always-success lock)
          [signer cells for fee]
Header deps: [startBlockHash, endBlockHash]
Outputs:  [0] Receiver cell (Proposal.amount capacity, Proposal.receiver lock)
Cell deps: always-success contract
Witness:  <none> — proposal type script reads no witness
```
