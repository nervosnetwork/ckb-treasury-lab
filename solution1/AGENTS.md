# CKB Voting System — Agent Instructions
This guide helps AI agents to implement a CKB voting system, including on-chain scripts, SDK and tools.

## Project Structure

```
├── contracts/          # CKB On-chain scripts (compiled to RISC-V for CKB VM)
├── crates/             # Shared Rust library crates
├── sdk/                # TypeScript SDK and CLI for interacting with on-chain contracts
├── tests/              # Integration tests for on-chain scripts
├── docs/               # Design documents and knowledge base
│   └── knowledge/      # How-to references (CCC, ckb-cli, devnet, RPC)
├── devnet/             # Local CKB devnet runtime data and chain specs
├── deployment/         # Deployment configs and cell descriptors for devnet scripts
├── scripts/            # Build helper scripts (clang discovery, reproducible Docker build)
└── build/              # Compiled contract binaries (output of `make build-contracts`)
```

The code for on-chain scripts lives mainly in the `contracts` and `crates` folders. For a guide on on-chain scripts, refer to ./docs/knowledge/on-chain-scripts.md.

## Other libs and tools
* When using the CCC library, refer to ./docs/knowledge/ccc.md.
* When using the ckb-cli tool, refer to ./docs/knowledge/ckb-cli.md.
* When working with the devnet, refer to ./docs/knowledge/devnet.md.
* When working with CKB RPC, refer to ./docs/knowledge/rpc.md.

## Deployment
For testing purposes (devnet or testnet), deploy the binary with hash_type = Type or hash_type = Data2. Don't use Data0 or Data1.


## Tests
When on-chain scripts are updated, consider adding test cases in the `tests` folder. There is no need to add tests in the `sdk` folder.

After any change, always run the following to verify:
```
make build
make test
```

Run `e2e` tests only when requested.

## Small Changes
Unless requested, don't add extra comments when adding small features or fixing bugs. 