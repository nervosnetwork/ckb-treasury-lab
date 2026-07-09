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

1. **No block data on-chain.** The `duration + 1` blocks never need to be carried
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
cells. When consumed, it appears in the input cells. It is not allowed to appear
on both sides at once, to prevent updating an existing proposal cell.

## Witness

This script reads **no witness**, on either creation or consumption.

## Cell Data

The cell data is a molecule structure:

```
table Proposal {
    duration: Uint32,
    vote_cell_code_hash: Byte32,
    vote_cell_hash_type: byte,
    description: Bytes,
    receiver: Script,
    amount: Uint64,
    minimal_requirement: Uint64,
}
```

1. `duration` (N) in blocks: the start block (the block where the proposal cell is
   created) is reserved for the proposal itself; votes are valid only if cast within
   the N consecutive blocks that follow. The full scanned range is therefore
   `duration + 1` blocks: the start block plus the N voting blocks. Votes outside
   the voting range are not counted.
2. `vote_cell_code_hash` / `vote_cell_hash_type`: specifies the script a vote cell
   must use. Cells using a different script are not counted as valid votes.
3. `description`: a plain-text UTF-8 description of the proposal.
4. `receiver`: the address that will receive the CKBytes when the proposal
   passes.
5. `amount`: the amount of CKBytes to be received.
6. `minimal_requirement`: minimum required CKBytes involved in voting.

Since proposal cells can be created by anyone, the fields `duration`,
`vote_cell_code_hash` / `vote_cell_hash_type`, `amount`, and
`minimal_requirement` must be constrained by the proposal type script. These
parameters will be published once the voting system is finalized.

## Unlocking Process

### Creation

When a proposal cell is created (the type script is on the output side), the
script verifies the following:

1. The 20-byte blake160 Type ID in `args` matches the standard Type ID
   construction.
2. The following fields are validated against the published parameters:
   - `vote_cell_code_hash` / `vote_cell_hash_type` in cell data
   - `duration` in cell data
   - `amount` in cell data
   - `minimal_requirement` in cell data
3. There is exactly one such type script in the transaction.

The vote cell `code_hash` / `hash_type` is fixed once the vote type script is
deployed. The remaining constrained fields are under discussion.

Since anyone can initialize a proposal on-chain, the system is vulnerable to spam. One approach is to require locking more capacity in the proposal cell, such as 1000 CKBytes. Spam proposals cannot be unlocked, so the locked capacity is lost forever — this is the cost of spamming.

### Consuming

When the proposal cell is consumed (the type script is on the input side), the
node determines the outcome by scanning the chain natively. No witness is
required, but the transaction must supply two `header_deps`.

1. **Reference the start and end blocks via `header_deps`.** The transaction
   provides `header_deps[0]` and `header_deps[1]`, corresponding to the start
   block hash and end block hash respectively. These hashes are referenced via
   `header_deps`; if either is invalid, the reference fails and the transaction
   cannot be constructed. The start block is the block that created the proposal
   cell being consumed — i.e. where voting began — and the node verifies that
   `header_deps[0]` matches the input's creating block (resolved from the input's
   `previous_output`).
2. **Determine the block range.** The node verifies that `header_deps[1].number`
   equals `header_deps[0].number + duration`, then reads the `duration + 1`
   consecutive blocks from `header_deps[0]` through `header_deps[1]` inclusive
   from its own storage. Because `header_deps[1]` must be referenced, the end
   block is guaranteed to already exist on-chain; this enforces that a proposal
   can only be settled **after the voting window has closed**.
3. **Tally votes.** Run the `count_vote` algorithm (identical to the shared logic
   in [crates/verification/src/lib.rs](https://github.com/XuJiandong/ckb-vote-poc/blob/main/crates/verification/src/lib.rs)) over
   every transaction in the N voting blocks (`header_deps[0].number + 1` through
   `header_deps[1].number` inclusive). The start block (`header_deps[0]`) is the
   proposal creation block and is excluded from vote counting.
   - A cell is counted as a vote when its type script `code_hash` /
     `hash_type` equals `vote_cell_code_hash` / `vote_cell_hash_type` from the
     proposal cell data **and** its type script `args` equals
     `blake160(proposal_type_script)`.
   - Its `Vote.amount` is recorded in a map keyed by the voter's lock script
     hash. Duplicate keys overwrite, so a later vote from the same voter replaces
     the earlier one (this is what enables vote retraction / changing a vote).
   - Each `Vote` carries a `dao_index`; the referenced DAO deposit out points are
     recorded in a second map keyed by out point, valued by the voter's lock hash.
     If any transaction in the range spends an out point already in that map, the
     associated voter is removed from both maps, preventing the same DAO deposit
     from being counted twice (double-vote resistance).
4. **Decide.** After the final block is processed, aggregate `yes_vote` and
   `no_vote` from the remaining entries. The proposal passes iff:

   ```
   yes_vote > no_vote
     && yes_vote + no_vote > minimal_requirement * 100_000_000
   ```

   (`Vote.amount` and the tallies are in shannon; `minimal_requirement` is in
   CKBytes, hence the `100_000_000` factor.)
5. **Unlock.** If the proposal passes, the type script succeeds and the cell may
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

### DoS considerations

- `duration` directly scales the amount of native work. A very large `duration`
  makes settlement expensive; the cycle charge (and therefore the fee) grows
  proportionally, so an attacker cannot force unpaid work.
- Consider constraining `duration` at creation time (see
  [Creation](#creation)) to bound the worst-case per-transaction scan.

## Design Notes

- **Self-containment trade-off.** This design intentionally breaks CKB's usual
  property that a transaction is self-contained and determined solely by its
  explicit inputs, outputs, and deps. The embedded script reads chain state
  (blocks) beyond what the transaction references. This is acceptable only because
  the code runs inside the node against the canonical chain, where the data is
  authoritative and identical on every node.
- **Determinism and reorgs.** The start block is pinned by `header_deps[0]` (and
  verified against the consumed input's canonical creating block), the end block
  by `header_deps[1]`, and the range is `duration + 1` blocks forward on the
  canonical chain, so the result is identical across nodes. A chain reorg that
  changes any block in the range (or the start block's position) changes the
  result; this is inherent to reading chain state and is the same class of
  concern that `header_deps` addresses for ordinary scripts.
- **Penalty on failure.** If a proposal fails, no one can recycle the cell. This
  is a deliberate penalty to discourage flooding the system with proposals.
  Updating an existing proposal is likewise disallowed; the recommended approach
  is to abandon it and create a new one.
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
        output_type: <none>                             # no witness needed on creation
```

---

### Example 2: Consuming a Proposal Cell (Proposal Passed)

No type-script witness is required. The
transaction supplies `header_deps[0]` (start block) and `header_deps[1]` (end
block); the node verifies `header_deps[0]` is `Proposal_Cell`'s creating block
and `header_deps[1].number == header_deps[0].number + duration`, scans the
`duration + 1` blocks from its own storage, tallies the votes, and unlocks the
cell because the proposal passed.

```yaml
Inputs:
    Proposal_Cell                                       # start block = the block that created this cell
        Data:
            Proposal (molecule):
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
    header_deps[0]: <start block hash>                  # block containing the proposal cell
    header_deps[1]: <end block hash>                    # start block + duration blocks later

# No type-script witness. The node performs the scan:
#   start_block   = header_deps[0]                       (must equal Proposal_Cell's creating block; reserved for proposal, not counted for votes)
#   end_block     = header_deps[1]                       (must exist; number == start_block.number + duration)
#   scan blocks [start_block .. end_block] inclusive (1 proposal block + duration voting blocks = duration + 1 total)
#   tally votes from voting blocks only [start_block+1 .. end_block]
#   tally votes -> yes_vote > no_vote && yes_vote + no_vote > minimal_requirement * 1e8
#   -> passed -> unlock

Witnesses:
    WitnessArgs structure (for the Treasury_Cell / funding inputs, as needed):
        Lock: <signature(s) required by those inputs' lock scripts>
        input_type: <none>
        output_type: <none>
```
