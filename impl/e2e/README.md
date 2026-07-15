# E2E Tests

The `.ckb-cli` folder contains a pre-existing account whose private key is stored in `pk1`. Its cells are pre-funded by the devnet. All operations below are based on this account.

## Devnet

Steps to run e2e tests on devnet:

1. Run `start.sh` to start the CKB node and miner.
2. Use `deploy.sh` in the `deployment/devnet` folder to deploy or upgrade scripts. For a first-time deployment, remove the `migrations` folder; keep it for upgrades. After the first deployment, update `config.ts` in the SDK, then run `pnpm dev check` to verify the config.
3. Run `dao-deposit.sh` to deposit some into the DAO. Vote weight is based on the deposit amount.
4. In `sdk`, run `pnpm dev create-proposal` to create a proposal.
5. In `sdk`, run `pnpm dev vote` to cast a vote on the proposal.
6. In `sdk`, run `pnpm dev consume-proposal` to create a proposal.


