# Proposal Type Script Specification

This document specifies a proposal type script implemented as a **native embedded
script in the CKB node**, in the same spirit as the built-in
[Type ID](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0022-transaction-structure/0022-transaction-structure.md#type-id)
script. It does **not** run inside CKB-VM.

The script solves the problem of counting stake-weighted votes over a block range
and unlocking a proposal cell when the proposal passes.

A proposal cell represents a proposal, and once it appears on-chain, voting
begins. Users cast votes in response to the proposal.


## Why an Embedded Script

The tally logic runs natively inside the CKB node. Because the script is part of
the node, it is not confined to the transaction's own data window (inputs,
outputs, `cell_deps`, `header_deps`, `witnesses`). It reads the required blocks
**directly from the node's own block storage**, which is already chain-validated.

This has three important consequences:

1. **No block data on-chain.** The remaining voting blocks never need to be carried
   in the witness (which would be infeasible for realistic durations). The node
   already has them.
2. **No integrity checks needed.** Blocks fetched from the node's store are
   already validated, so there is no need to verify the `parent_hash` chain or
   recompute each block's `transactions_root`. Only the vote-tally traversal
   remains.
3. **Cycle charging matters.** Because the work runs natively (not metered by
   CKB-VM instruction counting), the node must charge cycles explicitly and
   conservatively, proportional to the work performed, to prevent denial-of-service
   attacks. See [Cycle Charging](#cycle-charging).

## Script

```text
code_hash: <reserved embedded proposal type script code_hash>
hash_type: type
args:      <blake160 Type ID, 20 bytes>
```

Like Type ID, this script is matched by the node using a **reserved
`code_hash`** together with `hash_type: type`; when the node encounters a script
group with this identity, it runs the embedded implementation instead of locating
and executing RISC-V code in CKB-VM. Reserving this `code_hash` and shipping the
implementation is a **consensus change** (node fork / new genesis script).

The `args` is a single field:

- **`<blake160 Type ID>` (20 bytes)** ensures the cell is a singleton via the
  Type ID mechanism (see
  [Type ID implementation](https://github.com/nervosnetwork/ckb-std/blob/0a16c0ed8a6b4d8194d64420dbe309a0c23fc1b2/src/type_id.rs#L79-L85)).
  `blake160` is blake2b-256 with `ckb-default-hash` personalization, truncated to
  the first 20 bytes. The Type ID follows the standard construction:
  `blake160(first_input_out_point || output_index)`.

The corresponding lock script of the proposal cell should be an always-success
lock script. All access control is delegated to this type script. Once a proposal
passes, the cell can be consumed by anyone.

When a proposal cell is created, the proposal type script appears in the output
cells. When consumed, it appears in the input cells. During an optional updating
phase, it appears on both sides (exactly one input and one output) so the
checkpointed `TallyState` can advance without settling the proposal.

## Witness

It uses the following structure:

```
array Byte20 [byte; 20];
vector Byte20Vec <Byte20>;

struct VoteMapEntry {
  lock_hash_index: Uint16,
  direction: byte,
  shannon: Uint64,
}

vector VoteMapEntryVec <VoteMapEntry>;

struct DaoVoterEntry {
  out_point: OutPoint,
  lock_hash_index: Uint16,
}

vector DaoVoterEntryVec <DaoVoterEntry>;

table TallyState {
  all_lock_hash: Byte20Vec,
  vote_map: VoteMapEntryVec,
  dao_outpoint_to_voter: DaoVoterEntryVec,
}
```

During updating and consuming, the TallyState is placed in `input_type` of the type script witness in WitnessArgs.

The `Byte20`, `VoteMapEntry`, and `DaoVoterEntry` entries within each vector must be sorted. The final TallyState must be deterministic.


## Cell Data

The cell data is a molecule structure:

```
table Proposal {
    tally_state_hash: Byte20,
    duration: Uint32,
    vote_cell_code_hash: Byte32,
    vote_cell_hash_type: byte,
    description: Bytes,
    receiver: Script,
    amount: Uint64,
    minimal_requirement: Uint64,
}
```
1. `tally_state_hash`: blake160 of TallyState. This hash is all zero when created
   (no prior checkpoint). After an updating phase it is the blake160 of the
   checkpointed `TallyState` produced by that phase.
2. `duration` (N) in blocks: remaining blocks in which votes are still to be
   counted. Votes outside the voting range are not counted. During an updating
   phase the duration is reduced by the number of blocks just scanned.
3. `vote_cell_code_hash` / `vote_cell_hash_type`: specifies the script a vote cell
   must use. Cells using a different script are not counted as valid votes.
4. `description`: a plain-text UTF-8 description of the proposal.
5. `receiver`: the address that will receive the CKBytes when the proposal
   passes.
6. `amount`: the amount of CKBytes to be received.
7. `minimal_requirement`: minimum required CKBytes involved in voting.

Since proposal cells can be created by anyone, the fields `duration`,
`vote_cell_code_hash` / `vote_cell_hash_type`, `amount`, and
`minimal_requirement` must be constrained by the proposal type script. These
parameters will be published once the voting system is finalized.

## Unlocking Process
There are three phases: creating, updating, and consuming. The updating phase is optional.
It exists for performance reasons. When vote throughput is very high, a single consuming phase
would take too long, making it impossible to finish within one block.
Therefore, the whole process is split into several updating phases.

### Creating

When a proposal cell is created (the type script is on the output side), the
script verifies the following:

1. The 20-byte blake160 Type ID in `args` matches the standard Type ID
   construction.
2. The following fields are validated against the published parameters:
   - `vote_cell_code_hash` / `vote_cell_hash_type` in cell data
   - `duration` in cell data
   - `amount` in cell data
   - `minimal_requirement` in cell data
3. `tally_state_hash` are all zero.
4. There is exactly one such type script in the transaction.

The vote cell `code_hash` / `hash_type` is fixed once the vote type script is
deployed. The remaining constrained fields are under discussion.

Since anyone can initialize a proposal on-chain, the system is vulnerable to spam. One approach is to require locking more capacity in the proposal cell, such as 1000 CKBytes. Spam proposals cannot be unlocked, so the locked capacity is lost forever — this is the cost of spamming.

### Updating
Any proposal cell can be updated during the vote process. The script verifies the following:
1. There must be exactly one input proposal cell and one output proposal cell, with the same `args`.
2. Only `tally_state_hash` and `duration` may change in the cell data; all other
   fields must be identical.
3. If the input's `tally_state_hash` is not all zero, a `TallyState` must be included in
   the corresponding `WitnessArgs.input_type`, and its blake160 must match the input's
   `tally_state_hash`. If the input's `tally_state_hash` is all zero, start from an empty
   `TallyState` (no prior checkpoint).
4. The script then performs `count_vote` (described later) over all blocks from the
   input proposal cell's block (exclusive) to the output proposal cell's block (inclusive).
   It computes blake160 of the final `TallyState`; the result must match the
   `tally_state_hash` in the output proposal cell data.
5. The delta of `duration` (the old value minus the new value) must exactly match the
   block count above. The output `duration` cannot be zero.
   The delta should be large enough to prevent DoS attacks (e.g. > 450, ~1 hour).

The `count_vote` algorithm described as follow:
   - A cell is counted as a vote when its type script `code_hash` /
     `hash_type` equals `vote_cell_code_hash` / `vote_cell_hash_type` from the
     proposal cell data **and** its type script `args` equals
     `blake160(proposal_type_script)`.
   - Its `Vote.amount` is recorded in a map keyed by the voter's lock script
     blake160 hash. Duplicate keys overwrite, so a later vote from the same voter replaces
     the earlier one (this is what enables vote retraction / changing a vote).
   - Each `Vote` carries a `dao_index`; the referenced DAO deposit out points are
     recorded in a second map keyed by out point, valued by the voter's lock blake160 hash.
     If any transaction in the range spends an out point already in that map, the
     associated voter is removed from both maps, preventing the same DAO deposit
     from being counted twice (double-vote resistance).

### Consuming

When the proposal cell is consumed (the type script is on the input side), the
node determines the outcome by scanning the chain natively.

The script verifies the following:

1. The transaction provides `header_deps[0]` as the end block hash.
   The start block is the block that produced the proposal cell being consumed
   (the original creating block if never updated, otherwise the block of the latest
   updating transaction). The node verifies that `header_deps[0].number`
   equals the start block number plus the remaining `duration`. Since
   `header_deps[0]` must be referenced, the end block is guaranteed to already
   exist on-chain; this ensures a proposal can only be settled after the voting
   window has closed.
2. Read the `TallyState` from `WitnessArgs.input_type` if `tally_state_hash` is not
   zero, and verify that its blake160 hash matches. If `tally_state_hash` is all
   zero, start from an empty `TallyState`.
3. Run the `count_vote` algorithm over every transaction in the voting blocks, from
   the start block (exclusive) to the end block (inclusive) denoted by
   `header_deps[0].number`. The start block was already counted in a previous
   updating phase, or is the creating block and is excluded from vote counting.

4. After the final block is processed, aggregate `yes_vote` and
   `no_vote` from the remaining entries. The proposal passes if and only if:

   ```
   yes_vote > no_vote
     && yes_vote + no_vote > minimal_requirement * 100_000_000
   ```

   (`Vote.amount` and the tallies are in shannon; `minimal_requirement` is in
   CKBytes, hence the `100_000_000` factor.)
5. If the proposal passes, the type script succeeds and the cell may
   be spent. Otherwise it fails and the cell remains unspendable.

## Cycle Charging

Because the tally runs natively rather than in CKB-VM, its cost is **not** metered
by counting RISC-V instructions. The node must charge cycles explicitly. As with
the built-in Type ID script (a flat `TYPE_ID_CYCLES = 1_000_000`), the charge
exists for correctness/anti-DoS accounting, not to offer a discount over an
equivalent CKB-VM implementation.

Unlike Type ID, the work here is **variable** — it depends on how much data the
scan touches — so a flat charge is inadequate. There is **no closed-form
formula** for the cost. Instead, cycles are accumulated by an **algorithm that is
bound to the implementation**: as the tally runs, it charges cycles for each
non-constant-time operation it performs. The total is applied **whether the
proposal passes or fails**.

The charge accumulates from operations such as:

1. **Bytes read** — every byte of block/transaction/cell data loaded from the
   node's storage and traversed during the scan.
2. **Bytes hashed** — every byte fed into a hash function (e.g. computing voter
   lock-script hashes, the proposal `blake160`, out-point keys). Hashing cost is
   proportional to input length.
3. **Map operations** — insertions, lookups, and removals in the vote map (voter
   lock hash → vote) and the DAO-deposit map (out point → voter). Each operation's
   cost reflects its actual (non-constant-time) complexity, including key
   comparisons.
4. **Other non-constant-time operations** — any additional work whose cost scales
   with input size (e.g. molecule parsing/validation, variable-length copies,
   iteration over transactions, inputs, outputs, and `dao_index` entries).

Constant-time operations (fixed-size integer comparisons, arithmetic on the
tallies, etc.) do not need to be individually accounted for.

Properties this accounting must satisfy:

- **Deterministic across nodes.** Every full node scans the identical canonical
  block range and performs the identical operations, so the accumulated cycle
  count is identical everywhere. It is consensus-relevant, so all node
  implementations must accumulate cycles in exactly the same way.
- **Conservative.** The per-operation cycle costs must over-estimate real node
  cost, so a transaction can never force more work than it pays for.
- **Charged before completion.** As with `TYPE_ID`, if the accumulated cycles
  exceed the transaction's limit mid-scan, verification fails; work already done
  is bounded by the limit.
- Because `transactions_root` recomputation and `parent_hash` chaining are **not**
  performed (the blocks are already validated by the node), the cost is limited to
  the traversal, hashing, and map bookkeeping described above.

Another issue is that we will add a separate cycle limit for the proposal script. Consider a scenario
where a block contains only one proposal script. The current cycle limit for a single block is quite high
(3.5 billion), and such a limit might take down the node if a proposal script consumes all the cycles.
We will set the separate cycle limit to something like 50M (TODO). This value is sufficient for normal use (e.g. 7-day voting).

### DoS considerations

- `duration` directly scales the amount of native work. A very large `duration`
  makes settlement expensive; the cycle charge (and therefore the fee) grows
  proportionally, so an attacker cannot force unpaid work.
- Consider constraining `duration` at creating time (see
  [creating](#creating)) to bound the worst-case per-transaction scan.

## Design Notes

- **Self-containment trade-off.** This design intentionally breaks CKB's usual
  property that a transaction is self-contained and determined solely by its
  explicit inputs, outputs, and deps. The embedded script reads chain state
  (blocks) beyond what the transaction references. This is acceptable only because
  the code runs inside the node against the canonical chain, where the data is
  authoritative and identical on every node.
- **Determinism and reorgs.** At consuming time the end block is pinned by
  `header_deps[0]`, and the start block is the canonical block that produced the
  consumed proposal cell; the remaining range is the cell's `duration` blocks
  forward on the canonical chain, so the result is identical across nodes.
  Updating phases checkpoint intermediate `TallyState` into `tally_state_hash`.
  A chain reorg that changes any block in the scanned range (or a checkpoint
  cell's position) changes the result; this is inherent to reading chain state
  and is the same class of concern that `header_deps` addresses for ordinary scripts.
- **Penalty on failure.** If a proposal fails, no one can recycle the cell. This
  is a deliberate penalty to discourage flooding the system with proposals.
  Field changes outside the updating rules (e.g. rewriting `amount` or
  `description`) are disallowed; abandon the proposal and create a new one.
- **Reusable by third parties.** The proposal and vote scripts are not
  treasury-specific and can be integrated into third-party systems, which can
  reference this proposal type script.
- **Vote-time eligibility.** A DAO deposit created during the voting window can be
  used to vote, encouraging broader DAO participation.

## Examples

### Example 1: Creating a Proposal Cell

```yaml
Inputs:
    <any> Funding_Cell
        Data: <empty>
        Type: <none>
        Lock:
            <proposer's lock script>

Outputs:
    Proposal_Cell
        Data:
            Proposal (molecule):
                tally_state_hash: 0x0000...00           # 20 zero bytes; no checkpoint yet
                duration: 8640                          # ~1 day (8640 blocks x ~10s)
                vote_cell_code_hash: <32-byte hash of vote type script>
                vote_cell_hash_type: 0x01               # type
                description: "Fund infrastructure work Q3 2026"
                receiver:
                    code_hash: <secp256k1 code hash>
                    hash_type: 0x01                     # type
                    args: <20-byte blake160 of receiver pubkey>
                amount: 1000                            # 1000 CKBytes
                minimal_requirement: 5000               # 5000 CKBytes total vote weight
        Type:
            code_hash: <reserved embedded proposal type script code_hash>
            hash_type: type
            args:
                <20-byte blake160 Type ID>              # blake160(first_input_out_point || output_index)
        Lock:
            code_hash: <always-success lock code_hash>
            hash_type: <always-success lock hash_type>
            args: <empty>

    <any> Change_Cell
        Data: <empty>
        Type: <none>
        Lock:
            <proposer's lock script>

Witnesses:
    WitnessArgs structure:
        Lock: <proposer's signature>
        input_type: <none>
        output_type: <none>                             # no witness needed on creating
```

---

### Example 2: Updating a Proposal Cell (Optional Checkpoint)

Advances the checkpoint after scanning 1000 voting blocks. Input
`tally_state_hash` is all zero, so no prior `TallyState` is required in the
witness. Output `duration` is reduced by the scanned block count and must stay
non-zero. Output `tally_state_hash` is blake160 of the post-scan `TallyState`.

```yaml
Inputs:
    Proposal_Cell                                       # produced at block S (creating or prior update)
        Data:
            Proposal (molecule):
                tally_state_hash: 0x0000...00           # or blake160 of prior TallyState
                duration: 8640
                vote_cell_code_hash: <32-byte hash of vote type script>
                vote_cell_hash_type: 0x01
                description: "Fund infrastructure work Q3 2026"
                receiver:
                    code_hash: <secp256k1 code hash>
                    hash_type: 0x01
                    args: <20-byte blake160 of receiver pubkey>
                amount: 1000
                minimal_requirement: 5000
        Type:
            code_hash: <reserved embedded proposal type script code_hash>
            hash_type: type
            args:
                <20-byte blake160 Type ID>              # same args as output
        Lock:
            code_hash: <always-success lock code_hash>
            hash_type: <always-success lock hash_type>
            args: <empty>

Outputs:
    Proposal_Cell                                       # this tx is included in block S+1000
        Data:
            Proposal (molecule):
                tally_state_hash: <blake160(TallyState)> # after count_vote over (S, S+1000]
                duration: 7640                          # 8640 - 1000; must be > 0
                vote_cell_code_hash: <32-byte hash of vote type script>
                vote_cell_hash_type: 0x01
                description: "Fund infrastructure work Q3 2026"
                receiver:
                    code_hash: <secp256k1 code hash>
                    hash_type: 0x01
                    args: <20-byte blake160 of receiver pubkey>
                amount: 1000
                minimal_requirement: 5000
        Type:
            code_hash: <reserved embedded proposal type script code_hash>
            hash_type: type
            args:
                <20-byte blake160 Type ID>              # same args as input
        Lock:
            code_hash: <always-success lock code_hash>
            hash_type: <always-success lock hash_type>
            args: <empty>

Witnesses:
    WitnessArgs structure (proposal type script group):
        Lock: <none>                                    # always-success lock
        input_type: <TallyState or none>                # required only if input.tally_state_hash != 0
        output_type: <none>
```

---

### Example 3: Consuming a Proposal Cell (Proposal Passed)

The transaction supplies `header_deps[0]` as the end block. The start block is
the block that produced the consumed `Proposal_Cell` (creating block, or the
latest updating block). If `tally_state_hash` is non-zero, `WitnessArgs.input_type`
carries the matching `TallyState` checkpoint. The node scans the remaining
`duration` voting blocks, tallies votes, and unlocks the cell because the
proposal passed.

```yaml
Inputs:
    Proposal_Cell                                       # start block = block that produced this cell
        Data:
            Proposal (molecule):
                tally_state_hash: <blake160(TallyState)> # 0x00..00 if never updated
                duration: 7640                          # remaining blocks after updates (or full N)
                vote_cell_code_hash: <32-byte hash of vote type script>
                vote_cell_hash_type: 0x01
                description: "Fund infrastructure work Q3 2026"
                receiver:
                    code_hash: <secp256k1 code hash>
                    hash_type: 0x01
                    args: <20-byte blake160 of receiver pubkey>
                amount: 1000
                minimal_requirement: 5000
        Type:
            code_hash: <reserved embedded proposal type script code_hash>
            hash_type: type
            args:
                <20-byte blake160 Type ID>
        Lock:
            code_hash: <always-success lock code_hash>
            hash_type: <always-success lock hash_type>
            args: <empty>

    <vec> Treasury_Cell

Outputs:
    Receiver_Cell
        Data: <empty>
        Type: <none>
        Lock:
            code_hash: <secp256k1 code hash>
            hash_type: 0x01
            args: <20-byte blake160 of receiver pubkey> # must match Proposal.receiver
        Capacity: <Proposal.amount>

    Change_Cell
        Data: <empty>
        Type: <none>
        Lock:
            code_hash: <treasury lock script code_hash>
            hash_type: <treasury lock script hash_type>
            args: <empty>

Header Deps:
    header_deps[0]: <end block hash>                    # start_block.number + remaining duration

Witnesses:
    WitnessArgs structure (proposal type script group):
        Lock: <none>                                    # always-success lock
        input_type: <TallyState or none>                # required if tally_state_hash != 0
        output_type: <none>

    WitnessArgs structure (for the Treasury_Cell / funding inputs, as needed):
        Lock: <signature(s) required by those inputs' lock scripts>
        input_type: <none>
        output_type: <none>
```
