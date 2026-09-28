Your task is to write unit tests for the proposal type script, counting type script, vote type script, and config type script.
The unit tests all live in the `tests` folder.

First, generate the success test cases. Based on these success test cases, write the failure test cases.

To simplify writing test cases, use `always-success`(from builtin::ALWAYS_SUCCESS in ckb-testtool) scripts as lock scripts where needed, for example, as a voter's lock script or a proposal's lock script.

Read [ckb-testtool](https://github.com/nervosnetwork/ckb-testtool) first. You will need it to mock the necessary data.

Don't include all tests in one file. split them into different files.

The `enable_log` feature is on by default; use it for troubleshooting. Run `cargo test -- --nocapture` to collect logs.

When a bug is found in the implementation, consider updating the specification under the `docs/` folder with concise wording. This is not mandatory.

## Scope

- Test the four on-chain type scripts only: `config-type-script`, `proposal-type-script`, `vote-type-script`, `counting-type-script`.
- Do not add tests for `crates/ckb-vote-common` here. Its pure logic (`range.rs` and friends) is covered
  by in-crate `#[cfg(test)]` unit tests and stays there.
- `always-success` keeps a single smoke test (it proves the toolchain and the VM harness work); it gets no
  rule coverage.
- The `tests` crate is a native (`std`) crate. Never move test-only code into the `no_std` contracts or
  into `crates/ckb-vote-common`.

## File layout

```
tests/src/lib.rs            # declares the modules below, keeps Loader / TestEnv / MODE handling
tests/src/tests.rs          # module root + the always-success smoke test only
tests/src/helpers.rs        # shared fixtures and assertions, pub(crate) items
tests/src/config_tests.rs   # config type script
tests/src/proposal_tests.rs # proposal type script
tests/src/vote_tests.rs     # vote type script
tests/src/counting_tests.rs # counting type script
```

- Declare every module in `tests/src/lib.rs` under `#[cfg(test)]`, like the existing `mod tests;`.
- Every test case that used to live in `tests/src/tests.rs` must be migrated into the file of the script it
  exercises. `tests.rs` keeps only the always-success smoke test and no script rules.
- A test belongs to exactly one file: the file of the script whose rule it checks. Tests that span two
  scripts (for example "a counting cell aggregates vote cells") live in the file of the script that is
  being consumed/produced by the transaction, and say so in the doc comment.
- Do not create a second `Context`-bootstrap copy per file; everything shared goes into `helpers.rs`.

## Workflow

1. Write the success cases for a script first, exactly as the transaction is described in the `docs/`
   specification.
2. Derive the failure cases from those success cases: take the working transaction and change exactly one
   thing, so the test isolates the rule that is broken.
3. Every test carries a doc comment naming the rule it covers, e.g. `/// Spec: proposal, "Creating" - the
   capacity must be at least config.minimal_proposal_capacity.` A test with no traceable rule is either
   noise or a missing spec entry - fix the wording instead of keeping the test.
4. Run `make build && make test` when the file is written; do not wait until the very end.

## Coverage target (moderate)

One success test and one failure test per documented rule of each script. Do not aim for one test per
error variant; do aim for every rule, and for the failure test of a rule to assert the exact error the rule
maps to (see "Assertions").

- **config**: Type ID minting; update (`1 in / 1 out`) keeps args; burn is rejected; other transitions are
  rejected; malformed args length; cell data that does not decode as `VotingConfig`; `emergent_halt` set;
  a hash_type field larger than `MAX_SCRIPT_HASH_TYPE`.
- **proposal**: creation (`status = 0`, `total_yes = 0`, capacity at least the minimum, Type ID tail);
  creation rejected on wrong status / non-zero `total_yes` / bond below the minimum / wrong args length;
  fields immutable across an update; `open -> finalized` (`since` elapsed, always-success output lock,
  yes threshold met, `total_yes` written, counting ranges disjoint); rejections for each of those
  conditions; `finalized -> passed` with `challenge_time` elapsed and its rejection; receiving the grant
  and its rejections (missing recipient output, output too small); challenge (`NO >= total_yes`), a
  challenge below the bar, veto, recycle and "recycle too early"; invalid status transitions.
- **vote**: casting a vote against DAO deposits; every documented rejection (args length, direction,
  zero amount, voter lock not unlocked, proposal missing/not open, amount mismatch, amount below
  `config.minimal_vote_amount`, no deposit, deposit created after the proposal); withdrawal (`0` outputs).
- **counting**: creation aggregating vote cells; every documented rejection (malformed data, bad
  direction, `start_hash > end_hash`, proposal status, vote cell of another proposal, direction mismatch,
  lock hash outside the range, vote outside `vote_window`, amount mismatch, duplicate voter lock, no vote
  cell); consuming a counting cell; rewriting a counting cell is rejected.
- Include the boundary that the rule names (equal to the minimum, exactly `vote_duration` /
  `challenge_time` blocks, a lock hash exactly on `start_hash` / `end_hash`).

## Assertions

- Success: `context.verify_tx(&tx, MAX_CYCLES).expect("...")` with a message that names the rule.
- Failure: assert the exact exit code, never bare `is_err()`. Add one shared helper in `helpers.rs`:

  ```rust
  pub fn assert_script_error(context: &Context, tx: &TransactionView, expected: Error);
  ```

  It calls `verify_tx`, panics with the raw error when the transaction unexpectedly succeeds, extracts the
  script exit code from the `ckb_error::Error` (`downcast_ref::<TransactionScriptError>()`, then
  `script_error()`, then `ScriptError::ValidationFailure(_, code)`), and asserts it equals `expected as i8`.
  When the error is not a script validation failure, panic with the full error so the mismatch is obvious.
- A failure test must fail for the reason it claims. If the assertion cannot pin the code (for example a
  consensus-level failure that happens before the script runs), say so in the doc comment and assert the
  concrete error kind instead of `is_err()`.
- Never relax an assertion, delete a case, or mark a test `#[ignore]` to get a green run.
- Do not use `verify_and_dump_failed_tx` (see `AGENTS.md`); `verify_tx` is enough.
- `MAX_CYCLES` and `ONE_CKB` stay shared constants in `helpers.rs`.

## Fixtures and helpers

- Move the existing helpers out of `tests.rs` into `helpers.rs` (`always_success_lock`, `funding_cell`,
  `type_id`, `proposal_args`, `voting_config_data`, `proposal_data`, ...) and grow them there.
- Fixture builders take parameters and return everything a test needs (out points, scripts, scripts'
  hashes, the assembled transaction). A failure test then rebuilds the same fixture with one parameter
  changed, instead of hand-assembling a transaction.
- Proposed helpers: `deploy_contracts`, `config_cell`, `proposal_cell`, `vote_cell`, `counting_cell`,
  `dao_deposit_cell`, `link_cell_at_block`, `assert_script_error`.
- Keep `Loader`, `TestEnv` and the `MODE` handling in `lib.rs` as they are; tests load the binaries with
  `deploy_cell_by_name("<contract-name>")`.
- Build molecule payloads with the `ckb-vote-types` builders. Hand-written byte payloads are only for the
  malformed-encoding tests, and must be commented as such.
- Each test creates its own `Context`. No test may depend on the state or the order of another test.

## Mocking rules

- Deploy the real contract binaries with `deploy_cell_by_name` and derive the config's
  `vote_code_hash` / `counting_code_hash` / `always_success_code_hash` (plus their hash types) from those
  deployed scripts. Do not hard-code synthetic 32-byte patterns: the config must point at scripts that
  the tests actually deploy, which is what production does.
- DAO deposits use the real `constants::DAO_TYPE_SCRIPT_CODE_HASH` and
  `constants::DAO_TYPE_SCRIPT_HASH_TYPE`. The deposit is referenced through `cell_deps`; never as an
  input - `Context::complete_tx` resolves the scripts of inputs and would panic on a type-hash script
  with no code cell.
- Lock scripts of voters, initiators and the config cell are `always-success`. When a test needs two
  distinct identities that both work without signatures, use different `args` so the lock hashes differ.
- The finalized proposal output lock, the recipient lock and the veto lock are all built from the
  `always-success` binary, with the config carrying the matching hash.
- `complete_tx` preserves the cell deps of a transaction and adds the ones it can resolve; build the
  transaction's own deps (config cell, proposal cell, vote cells, DAO deposits) explicitly with
  `.cell_dep(...)`.

## Block numbers, headers and `since`

- The proposal and counting scripts read block numbers through `load_header(..., Source::CellDep)`. Any
  cell whose block number is read must have a header inserted into the context and linked to the cell
  (`insert_header` + `link_cell_with_block`), and the header hash must be listed in the transaction's
  `header_deps`.
- A cell that is an input of a transaction that uses a relative `since` must be linked to its creating
  block as well.
- Build relative block-number `since` values with a helper (`0x4000_0000_0000_0000 | n`, RFC 0017). Never
  use absolute `since` in a success case.
- Cover the `since` rules as failures too: an absolute `since`, a non-block-number metric, and a
  `cell_dep` whose header is not in `header_deps`.

## Logging and running

- Build the contracts before running tests; the binaries are loaded from `build/<MODE>`:
  - `make build && make test`
  - `cargo test -- --nocapture` after a build, when the logs are needed
- Keep `enable_log` enabled: it is the main troubleshooting tool and it is a default feature. Do not turn
  it off to make binaries smaller or tests faster.
- Logs are not part of an assertion: never assert on log text. `helpers.rs` may print the captured debug
  messages when a transaction is rejected unexpectedly.


## Definition of done

- All four scripts have their own test file, `tests.rs` holds only the smoke test, and `helpers.rs` holds
  the shared fixtures.
- `make build && make test` passes; the suite is green and no test is ignored.
- The final report contains: the divergences noted in `docs/`, the bugs fixed (if any), and the `make build && make test` result.
