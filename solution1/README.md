# CKB Draft Voting System

A candidate solution, See [design document](./docs/design.md)

## Crates and contracts

| Path | Description |
| --- | --- |
| `contracts/config-type-script` | the singleton config cell ([spec](./docs/config-type-script-spec.md)) |
| `contracts/proposal-type-script` | proposal / finalized proposal / passed proposal ([spec](./docs/proposal-type-script-spec.md)) |
| `contracts/vote-type-script` | a DAO backed vote ([spec](./docs/vote-type-script-spec.md)) |
| `contracts/counting-type-script` | an aggregated batch of votes ([spec](./docs/counting-type-script-spec.md)) |
| `contracts/always-success` | the lock of a finalized proposal cell, so that it can be challenged |
| `crates/ckb-vote-types` | the molecule types of the system |
| `crates/ckb-vote-common` | shared script code: config loading, errors, hashes, `since` |

```bash
make build   # builds every contract for riscv64imac-unknown-none-elf
make test    # runs the integration tests from tests/ on ckb-vm
```

## Web
A testnet deployment is available [here](https://treasury-testnet.ckbapp.dev/).
