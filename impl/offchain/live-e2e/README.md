# V7 live-chain E2E

This runner starts an isolated Dummy-PoW CKB node and executes the current V7
Treasury voting lifecycle against real RPC, tx-pool, block assembly, and block
verification paths.

```bash
cd /Users/yukang/code/ckb
cargo build --bin ckb

cd impl
make build
CKB_BIN=/Users/yukang/code/ckb/target/debug/ckb \
CKB_REPO=/Users/yukang/code/ckb \
cargo run -p live-e2e
```

The scenario covers:

- consensus-created Treasury Cells;
- DAO deposits, proposal creation/closing, and on-chain votes;
- compact VoteEventCell position proofs with vote-time DAO eligibility;
- an incomplete final tally candidate slashed by an omitted-vote challenge;
- a complete candidate finalized after its challenge period; and
- Result Cell plus Treasury Cell consumption producing the requested payout;
- two parallel TallyChainCells owned by distinct operators;
- Guardian veto after one Candidate has matured and rejection of its stale
  settlement transaction;
- permanent Vetoed Result enforcement; and
- exact Candidate and Active tally-bond refunds to their recorded operators.

The generated chain directory, node log, and `report.json` are retained below
`impl/target/live-e2e/` for inspection. Test funding uses distinct always-success
locks so the run focuses on the Treasury and voting contracts rather than wallet
signature assembly.
