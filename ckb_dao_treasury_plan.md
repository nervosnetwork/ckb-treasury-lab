# CKB DAO Treasury Implementation Plan

Last updated: 2026-09-03

Sizes in this document use decimal KB (`1 KB = 1,000 bytes`).

## Hash-Range Counting Cells (2026-09-03)

### Objective

Implement the alternative on-chain voting design from xjd's Treasury brainstorm
as a separate protocol path under `impl/counting-contracts/`. Vote Cells remain
the authenticated source of voting weight. Independent transactions create
range-bounded Counting Cells by loading Vote Cells as CellDeps, checking
proposal and direction identity,
requiring unique voter locks, and recomputing the declared subtotal. A proposal
candidate aggregates non-overlapping YES ranges; during its challenge period,
non-overlapping NO ranges may prove that the configured passing rule fails.

The existing `impl/contracts/` SMT tally suite remains untouched and available
for direct comparison. The Counting Cell workspace owns its common wire models,
Proposal, Vote, Counting, and Policy contracts. No CKB node or consensus change
is required for this path.

### Steps

- [x] Create branch `xjd-counting-cell-voting` without discarding the existing
  uncommitted PoC changes.
- [x] Re-fetch the latest gist and verify the existing 39-test baseline.
- [x] Define canonical Counting Cell and finalized-proposal encodings in the
  independent `counting-common` crate.
- [x] Implement the Rust `counting-type-script` contract.
- [x] Integrate YES candidate creation, NO challenge, and mature settlement with
  Proposal and Policy Type Scripts.
- [x] Add an off-chain Counting Cell builder.
- [x] Add codec, CKB-VM, adversarial, and proposal-rule tests.
- [x] Add a real-node E2E lifecycle covering proposal, votes, counting,
  challenge, and settlement.
- [x] Update the design document and compare contract size and transaction cost
  with the SMT path.
- [x] Run formatting, Clippy, all tests, and the live E2E from a freshly started
  node.

### Security Invariants

- A Counting Cell amount is exactly the checked sum of its referenced Vote
  Cells, not a claim supplied by the operator.
- YES Counting Cells and the `Closed -> Finalized` transition require the
  Proposal creator's lock authorization. NO Counting Cells remain
  challenger-created so the Proposal creator cannot suppress counter-evidence.
- The Proposal Cell's complete capacity is its bond and must be at least the
  configured `minimum_proposal_bond`. A successful challenge transfers that
  exact capacity to the unique lock shared by all consumed NO Counting Cells;
  fees cannot be deducted from it or the reward redirected by an unrelated
  input.
- Approval uses a strict checked ratio comparison. Equality with the configured
  threshold fails, so enough verified NO weight is a sufficient challenge
  certificate even without proving the complete NO tally.
- Vote Cells in one Counting Cell have strictly increasing full lock hashes, so
  one voter lock cannot be counted twice in that Cell.
- Counting ranges in one candidate or challenge are pairwise non-overlapping,
  preventing the same lock-hash bucket from being accepted through two batches.
- Counting Cell creation proves only the included subtotal. The candidate and
  NO-challenge flow is an optimistic certificate, not a claim that every Vote
  Cell in a range was included.
- A candidate can be created only when its verified YES subtotal passes with
  zero NO votes. A challenge succeeds only when its verified NO subtotal makes
  the configured proposal rule fail.
- The proposal's relative `since` enforces the challenge period before a passed
  result can settle.

### Validation

- Five independent release CKB-VM contracts build between 56.192 KB and 76.360
  KB. The Counting Type Script is 60.240 KB.
- Codec, builder, and CKB-VM suites pass 12 non-ignored tests. The 1,000-vote
  benchmark verifies in 14,331,162 cycles with a 37.463 KB transaction.
- A fresh real node accepted DAO deposits, YES and NO Vote Cells, a
  proposer-authorized YES Counting Cell, a proposer-authorized Finalized
  Proposal, a challenger-authored NO Counting Cell, and a successful rejection
  challenge. The Finalized Proposal and NO Counting Cell became dead while the
  Rejected Result was then consumed by the challenger to claim the complete
  1,500 CKB Proposal bond. Report:
  `impl/counting-contracts/target/live-e2e/1788440203-52932/report.json`.

## Pre-Settlement Guardian Veto (V7, 2026-08-27)

### Objective

Allow the configured Guardian to veto an Open or Closed Proposal at any time
before final settlement. Veto consumes the singleton Proposal Cell, creates an
auditable Vetoed Result Cell under the configured burn lock, and therefore
prevents every competing TallyChain from settling. The Proposal bond is burned
in full. Existing Active or Candidate TallyChainCells may be cleaned up, but
each complete bond must be returned to its recorded tally operator. Active
cleanup still satisfies the operator lock; Candidate cleanup uses the configured
permissionless Candidate lock.

### Design

- Proposal Cells use the canonical permissionless lock committed by Proposal
  Config. Proposal data commits the initiating proposer's lock hash.
- Normal settlement returns the complete Proposal bond to that proposer lock.
- Veto requires an independent input under the configured Guardian lock and
  sends the complete Proposal bond to the configured burn lock as a typed
  Vetoed Result Cell.
- Normal settlement and veto consume the same singleton Proposal Cell, so only
  one can commit.
- A Vetoed Result Cell authorizes cleanup of any TallyChainCell for the same
  Proposal. Cleanup cannot redirect or reduce the bond. The Cell is itself
  immutable, so no third party can remove this cleanup credential.

### Steps

- [x] Audit the current Proposal, Result, Tally bond, Treasury burn, and live E2E
  paths.
- [x] Version the shared wire model with explicit Passed, RejectedByVote, and
  Vetoed outcomes plus Guardian, proposal-lock, proposer, and burn commitments.
- [x] Implement Proposal and Policy veto validation and normal Proposal-bond
  return validation.
- [x] Implement Active/Candidate TallyChain cleanup with exact operator-bond
  refund.
- [x] Add positive and adversarial common/contract tests.
- [x] Extend the real-node E2E with veto, Proposal-bond burn, parallel
  TallyChain cleanup, and exact operator refunds.
- [x] Rebuild contracts and run formatting, Clippy, all tests, and the live E2E.
- [x] Update protocol documentation and record final contract sizes and reports.

### Implementation Notes

- The shared fixed model is V2 and the tally witness is V7. Result outcomes are
  `Passed`, `RejectedByVote`, or `Vetoed`.
- A Vetoed Result is a permanently unspendable typed Cell, even if its burn lock
  would otherwise authorize a spend. This both burns the Proposal bond and keeps
  the tally-cleanup credential live.
- Normal settlement and veto both preserve the Proposal Cell's complete capacity
  in the Result output. The former locks it to the proposer; the latter locks it
  to the configured burn lock.
- Active tally cleanup requires the operator lock's normal authorization.
  Candidate cleanup is permissionless. The Tally Type Script always enforces a
  full-capacity, plain output to the recorded operator lock hash.

### Validation

- All seven release CKB-VM contracts rebuilt cleanly. Modified stripped sizes:
  Proposal Type Script 71.096 KB, Policy Type Script 67.872 KB, Tally Type
  Script 220.976 KB, and Treasury Lock Script 57.312 KB.
- Formatting and Clippy completed without warnings. The common, builder, and
  CKB-VM suites passed 39 non-ignored tests; three benchmark tests remain
  intentionally ignored by the normal test target.
- A fresh-node V7 E2E passed both the existing vote/challenge/payout lifecycle
  and the Guardian path: two distinct operators created parallel TallyChains,
  one reached a mature Candidate, veto consumed the Proposal Cell, stale
  settlement and Vetoed-Result consumption were rejected, and 5,200/5,300 CKB
  bonds were refunded exactly. Report:
  `impl/target/live-e2e/1787826997-21466/report.json`.

## Vote-Time DAO Eligibility (2026-08-27)

### Objective

Count a vote when its immutable VoteEventCell is created from live, eligible DAO
CellDeps. Spending those DAO Cells after voting does not revoke the vote. Remove
DAO-spend events, the DAO-outpoint SMT namespace, and omitted-spend challenges
from tally settlement.

### Steps

- [x] Inspect the V5 wire format, reducer, scanner, contract tests, live E2E, and
  benchmark harness.
- [x] Upgrade the tally witness and reducer to the vote-time eligibility model.
- [x] Replace spend-liveness tests and update protocol documentation.
- [x] Rebuild and deploy the contracts; run unit, contract, and live-chain E2E
  tests.
- [x] Measure the default-limit vote capacity and SMT proof sizes.

### Implementation Notes

- V6 has one `TallyState.state_root` and two SMT namespaces: vote and event.
- VoteRecord no longer stores DAO outpoints. VoteData still commits them so the
  Vote Type Script can validate live DAO deposits at vote creation.
- DAO-spend transactions are not scanned, encoded, or challengeable. The only
  completeness challenge is an omitted immutable VoteEventCell.

### Validation

- All release CKB-VM contracts rebuilt cleanly. The stripped Tally Type Script
  is 217.576 KB; the Vote Type Script is 72.544 KB.
- Formatting, full Clippy with warnings denied, and 37 non-ignored common,
  builder, and CKB-VM contract tests passed.
- Fresh-chain V6 E2E passed proposal creation, DAO age rejection, two votes, a
  real DAO phase-1 spend after voting, omitted-vote challenge, complete tally,
  challenge maturity, Result creation, and Treasury payout. The spent deposit's
  vote remained counted. Report:
  `impl/target/live-e2e/1787822684-11839/report.json`.
- The ideal empty-state 100-vote batch used 208,647,822 cycles, a 32.869 KB
  witness, a 37.262 KB transaction, and a 0.895 KB SMT proof.
- Under default 3.5B-cycle and 597 KB block limits, 1,365 ideal independent
  votes passed at 2,839,164,998 cycles and 498.948 KB. At 1,366 votes the
  contract hit its deterministic CKB-VM heap-allocation boundary first.
- For 200 absent target leaves (100 votes), synthetic compiled SMT proofs were
  0.889, 13.925, 34.875, and 56.983 KB against 0, 300, 3,000, and 30,000
  existing nonzero leaves respectively.

## DAO Deposit Age Validation (2026-08-27)

### Objective

Require every DAO deposit referenced by a VoteEventCell to have been created in
a block strictly earlier than the block that created the Proposal Cell. The
Vote transaction supplies both creation headers as HeaderDeps; the Vote Type
Script loads the headers through the resolved CellDeps and compares their block
numbers. No raw creation transaction or transaction-position proof is needed.

### Steps

- [x] Verify that CKB exposes a CellDep's creation header when its block hash is
  present in the transaction's HeaderDeps.
- [x] Add the strict DAO creation block check to the Vote Type Script.
- [x] Add contract tests for older, same-block, newer, and missing-header DAO
  deposits.
- [x] Update the live-chain E2E transaction builder and add a rejected late-DAO
  vote before the successful voting and settlement flow.
- [x] Rebuild all contract binaries and run formatting, Clippy, and the full
  Rust/contract test suite.
- [x] Deploy the rebuilt binaries to a fresh local CKB chain and rerun the full
  live-chain E2E.

### Validation

- The clean release build, formatting check, full Clippy run, 39 standard
  Rust/CKB-VM tests, and the ignored cycle benchmark passed.
- The rebuilt Vote Type Script is 72.544 KB and has CKB data hash
  `0x15ab76603782b78b5600b9874754c000a081dc9c9e6d9178ffaf1a946b545020`.
- A fresh local CKB chain deployed the rebuilt binaries as genesis system cells.
  DAO deposits from blocks 20 and 24 could vote on the Proposal from block 28;
  a DAO deposit from block 32 was rejected with Vote script error code 20.
- The remaining omitted-vote challenge, complete tally, finalization, and
  Treasury payout flow passed. Report:
  `impl/target/live-e2e/1787804454-33060/report.json`.

## Historical V5 Implementation (Superseded)

### Objective

Replace raw historical vote transactions in tally witnesses with live,
canonical VoteEventCells authenticated by transaction-position proofs. Keep
DAO-spend raw transactions as tally events so a deposit spent during the
voting window still revokes the related vote. Authorization remains delegated
to each DAO deposit's lock script.

## Steps

- [x] Define the V5 hybrid wire model for compact VoteEventCell proofs, raw DAO
  spend events, and the current immutable VoteEventCell lifecycle rule.
- [x] Update the Vote Type Script to validate lock-agnostic authorization,
  eligible DAO deposits, canonical event commitments, and event-cell lifetime.
- [x] Update the Tally Type Script to load VoteEventCells from direct CellDeps,
  authenticate their creation positions against HeaderDeps, while retaining
  raw DAO-spend processing and ChallengeSpend.
- [x] Update the off-chain scanner, batch builder, candidate replay, RPC data
  sources, and live-chain transaction assembly.
- [x] Add positive and adversarial tests, update protocol documentation and
  benchmarks, then run formatting, Clippy, contract tests, cycle benchmarks,
  and the live CKB E2E. Compare V4 and V5 witness/transaction bytes, contract
  binary sizes, and cycles for the same vote counts.

## Implementation Notes

- This is a prototype wire-format upgrade. Tally witnesses move from V4 to V5;
  old witness bytes are rejected instead of being interpreted under new rules.
- A transaction has no single OutPoint. Each VoteEventCell is identified by
  `(vote_tx_hash, output_index)`, while `tx_index` separately identifies the
  vote transaction's position inside a block.
- VoteEventCells are referenced as direct CellDeps so parallel TallyChains do
  not consume or contend on them.
- VoteEventCells are immutable in V5. Safe reclaim remains unresolved because a
  fixed timeout can expire before a delayed tally/challenge completes.
- The tally contract must never trust a builder-supplied event. It checks the
  live CellDep, its configured Vote Type Script, canonical event data, voting
  window, and CBMT inclusion under the referenced block header.
- Vote creation no longer places the whole vote transaction in a tally witness.
  DAO-spend events still carry raw transactions because the reducer must inspect
  their inputs and revoke any VoteRecord backed by a spent DAO deposit.
- This removes the oversized-vote-transaction witness attack. An oversized
  DAO-spend transaction remains a separate residual risk and must be bounded or
  handled without weakening deposit-liveness semantics.

## Progress Log

- 2026-08-21: Inspected the current V4 raw-transaction witness path, Vote Cell
  creation, DAO-spend removal logic, candidate replay, and live E2E layout.
- 2026-08-21: Corrected the design boundary: DAO-spend raw transactions and
  ChallengeSpend are retained; snapshot voting is explicitly out of scope.
- 2026-08-21: Captured the V4 comparison baseline. For 100 votes: a 59.119 KB
  witness, a 59.876 KB transaction, and 314,614,791 cycles. For 500 votes: a
  295.249 KB witness, a 296.006 KB transaction, and 1,566,465,014 cycles.
  Baseline stripped ELF sizes: tally-type-script 226.040 KB and vote-type-script
  67.896 KB.
- 2026-08-21: Implemented V5 compact VoteEvent proofs with direct CellDep
  indices and retained raw DAO-spend events. The builder test asserts compact
  votes carry no raw transaction while tracked spends do.
- 2026-08-21: Final V5 measurement at 100 votes: a 43.325 KB witness, a 47.782
  KB transaction, and 311,820,088 cycles. Compared with V4, this is 26.7%
  smaller witness, 20.2% smaller transaction, and 0.9% fewer cycles. The final
  stripped tally ELF is 229.904 KB (+3.864 KB, 1.7%); the Vote ELF is 71.304 KB
  (+3.408 KB, 5.0%).
- 2026-08-21: The real local CKB E2E passed proposal creation, two votes, an
  intentionally omitted vote, successful challenge, complete re-tally,
  challenge-period maturity, final settlement, and Treasury payout. Report:
  `impl/target/live-e2e/1787274775-93275/report.json`.

## Counting Cell Batch Limit Increase

### Objective

Increase the prototype's per-Counting-Cell Vote Cell limit from 1,000 to 2,000
and measure how many Counting Cells a proposal finalization can consume.

### Steps

- [x] Update the shared test config and live-chain E2E config to 2,000 votes per
  Counting Cell.
- [x] Extend the Counting Cell creation benchmark to 2,000 Vote Cells.
- [x] Add a proposal-finalization benchmark over increasing Counting Cell input
  counts.
- [x] Rebuild the contracts and run the standard Rust and CKB-VM tests.
- [x] Redeploy the rebuilt contracts to a fresh local CKB chain and rerun the
  challenge and successful Treasury payout paths.

### Validation

- A 2,000-vote Counting Cell transaction verifies in 27,504,245 cycles and is
  74.557 KB.
- The idealized finalization benchmark reaches 69,987,211 cycles with 3,837
  Counting Cells and 70,005,379 cycles with 3,838 Counting Cells. The default
  70M tx-pool verification policy therefore becomes the first relay boundary,
  before transaction bytes.
- With 2,000 Vote Cells represented by each Counting Cell, the benchmark's
  idealized default-relay boundary represents 7,674,000 Vote Cells. This is a
  certificate capacity, not a protocol-wide cap on votes cast for a Proposal.
- All 16 standard Rust and CKB-VM tests passed; two cycle benchmarks remain
  ignored by the normal suite and passed when run explicitly.
- The fresh local-chain E2E passed both rejection-by-challenge and successful
  finalization/payout paths. Report:
  `impl/counting-contracts/target/live-e2e/1788491853-24710/report.json`.
