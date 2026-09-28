# CKB Voting System SDK

TypeScript SDK and `ckb-vote` CLI for the on-chain voting system of
`contracts/`: the config cell, the proposal lifecycle, the vote cells and the
counting cells. It is built on [`@ckb-ccc/shell`](https://github.com/ckb-devrel/ccc)
and runs **only under Bun** (developed with `bun 1.4.2`).

The SDK builds and sends the transactions the four type scripts expect, and
queries the cells they produce. Every rule it enforces locally comes from
`docs/*.md` and from `contracts`.

```
src/
  config.ts       deployment config (JSON) loading and validation
  client.ts       client / signer construction
  codec.ts        molecule codecs of the four cell payloads
  constants.ts    statuses, directions, lengths, system script hashes
  utils.ts        hashing, Type ID, script/cell/header helpers
  query.ts        cell discovery and decoding
  config-cell.ts  create / update the config cell, check a deployment
  proposal.ts     create, finalize, pass, challenge, recycle, veto, grant
  vote.ts         cast and withdraw a vote
  counting.ts     create and consume a counting cell
  dao.ts          create the Nervos DAO deposits a vote is backed by
  deploy.ts       publish the contract binaries
  devnet.ts       derive a devnet's known-script cell deps from block 0
  cli/            the ckb-vote CLI
```

## Install

```sh
cd sdk
bun install
```

## Quick start on a local devnet

The examples assume a devnet node on `http://127.0.0.1:8114` and a funded
private key file (`pk`), e.g. the first genesis account of
`docs/knowledge/devnet.md`.

```sh
# 1. Derive the devnet's genesis cell deps (they differ per devnet).
bun run src/cli/index.ts devnet-scripts --out ../deployment/devnet.json

# 2. Publish the five binaries from ../build/release and complete the config.
bun run src/cli/index.ts deploy \
  --config ../deployment/devnet.json \
  --private-key-file pk \
  --out ../deployment/devnet.json

# 3. Mint the config cell and record its Type ID.
bun run src/cli/index.ts create-config \
  --config ../deployment/devnet.json --write ../deployment/devnet.json \
  --private-key-file pk \
  --minimal-proposal-capacity 200 --yes-threshold 100 \
  --minimal-vote-amount 100 \
  --vote-duration 5 --vote-window 1000 --challenge-time 5

# 4. A DAO deposit older than the proposal is what a vote is backed by.
#    (Create one with any DAO tool; the SDK only consumes deposits.)

# 5. Create a proposal, vote and collect the votes.
PROPOSAL=$(bun run src/cli/index.ts create-proposal \
  --config ../deployment/devnet.json --private-key-file pk \
  --description "Fund the docs" --requested-amount 150 --json | jq -r .proposalCell.txHash)

bun run src/cli/index.ts vote \
  --config ../deployment/devnet.json --private-key-file pk \
  --proposal $PROPOSAL:0 --direction yes

COUNTING=$(bun run src/cli/index.ts create-counting \
  --config ../deployment/devnet.json --private-key-file pk \
  --proposal $PROPOSAL:0 --direction yes \
  --start-hash 0 --end-hash 65535 --wait --json | jq -r .countingCell.txHash)

# 6. Finalize after vote_duration, pass after challenge_time, claim the grant.
bun run src/cli/index.ts finalize-proposal \
  --config ../deployment/devnet.json --private-key-file pk \
  --proposal $PROPOSAL:0 --counting $COUNTING:0 --wait
bun run src/cli/index.ts pass-proposal \
  --config ../deployment/devnet.json --private-key-file pk \
  --proposal <FINALIZED>:0 --wait
bun run src/cli/index.ts receive-grant \
  --config ../deployment/devnet.json --private-key-file pk \
  --proposal <PASSED>:0
```

Every command accepts `--config <path>`, `--rpc-url <url>` and `--json`;
`--private-key` or `--private-key-file` is needed to send a transaction. Run
`bun run src/cli/index.ts --help` for the full list.

## End-to-end run

`e2e` performs the whole quick start in one command. It starts the devnet
itself - `ckb run` and `ckb miner`, with `devnet/`. And run:

```sh
cd sdk
bun run src/cli/index.ts e2e
```

It deploys the five contracts then runs `create-proposal -> vote -> create-counting -> finalize-proposal -> pass-proposal`. 

The deployment config it writes (and uses) is `./devnet.config`; `--config` / `--rpc-url` override the defaults.

## Deployment config

Nothing about a deployment is hard-coded: scripts, code cells and the config
cell all live in one JSON file (`deployment.example.json` shows the shape).
`deploy` and `devnet-scripts` generate it; `create-config --write` and
`update-config --write` keep the config cell coordinates up to date.

```jsonc
{
  "rpcUrl": "http://127.0.0.1:8114",
  "feeRate": 1500,
  "scripts": {
    "config":        { "codeHash": "0x..", "hashType": "data2", "args": "0x<Type ID>",
                       "cellDep": { "txHash": "0x..", "index": 0, "depType": "code" } },
    "proposal":      { "codeHash": "0x..", "hashType": "data2", "cellDep": { ... } },
    "vote":          { ... },
    "counting":      { ... },
    "alwaysSuccess": { ... }
  },
  "configCell": { "txHash": "0x..", "index": 0 },
  "knownScripts": { "Secp256k1Blake160": { ... }, "NervosDao": { ... } }
}
```

- `scripts.<name>.codeHash` / `hashType` are what the SDK writes in a script
  that references the deployed one; `cellDep` is the code cell to list in
  `cell_deps`.
- `scripts.config.args` is the Type ID of the config type script. It is
  produced by `create-config` and is required before a proposal can be created.
- `configCell` is optional: when it is missing (or stale, after an update) the
  config cell is searched by type script, which needs a node indexer.
- `knownScripts` overrides CCC's built-in system script cell deps, which differ
  on a devnet. `devnet-scripts` derives them from block 0.

## API

```ts
import { ccc } from "@ckb-ccc/shell";
import {
  loadConfig,
  buildClient,
  createProposal,
  castVote,
  createCountingCell,
  finalizeProposal,
  receiveGrant,
  findProposalCells,
  findVoteCells,
  findCountingCells,
  ProposalStatus,
} from "@ckb-vote/sdk";

const config = loadConfig("devnet.json");
const signer = new ccc.SignerCkbPrivateKey(buildClient(config), privateKey);

const { proposalCell, proposalId } = await createProposal(signer, config, {
  description: "Fund the docs",
  requestedAmount: 150n * 100_000_000n,
  recipient: "ckt1q...", // defaults to the signer's lock
  capacity: 200n * 100_000_000n, // the bond, defaults to the configured minimum
});

await castVote(signer, config, {
  proposalOutPoint: proposalCell,
  direction: "yes",
  // `deposits` selects the DAO deposits; by default every deposit of the voter
  // that is older than the proposal is used.
});

const counting = await createCountingCell(signer, config, {
  proposalOutPoint: proposalCell,
  direction: "yes",
  startHash: 0,
  endHash: 0xffff,
  wait: true, // collect only after config.vote_duration elapsed
});

await finalizeProposal(signer, config, {
  proposalOutPoint: proposalCell,
  countingOutPoints: [counting.countingCell],
  wait: true, // wait until config.vote_duration elapsed
});

for await (const { cell, data } of findProposalCells(client, config)) {
  console.log(cell.outPoint, ProposalStatus[data.status], data.description);
}
```
