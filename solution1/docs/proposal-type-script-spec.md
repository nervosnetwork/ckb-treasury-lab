# Proposal Type Script Specification
This is the proposal type script specification, which identifies the proposal cell, finalized proposal cell and passed proposal cell.


## Script
The script has the following structure:
```
code_hash: <code_hash to proposal type script>
hash_type: <hash_type to proposal type script>
args: <20 bytes, ckb-blake160-hash of config type script> <20 bytes, Type ID like args>
```

The first 20 bytes point to a config cell, identified by the ckb-blake160-hash of its type script. All related config fields, including vote type scripts and counting type scripts, should be read from this config cell.

The latter 20 bytes `args` are the ckb-blake160-hash of the first CellInput structure of the creating transaction, combined with the output index of the cell. This makes the script unique across the entire blockchain. This mechanism is used in [Type ID](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0022-transaction-structure/0022-transaction-structure.md#type-id).

## Cell Data
The corresponding cell data has the following structure in molecule format:
```
table ProposalCellData {
    status: byte
    description: Bytes,
    requested_amount: Uint64,
    recipient_lock_hash: Bytes20,
    total_yes: Uint64,
    origin_block_number: Uint64,
}
```

The `status` field can take the following valid values to indicate the type of cell:
- 0: proposal cell
- 1: finalized proposal cell
- 2: passed proposal cell

The `description` field is UTF-8 text that describes the proposal.

The `requested_amount` field specifies the amount of assets that can be granted if the vote passes, while `recipient_lock_hash` is the lock script ckb-blake160-hash of the target recipient.


## Witness
No witness is needed.

## Processing
The type script goes through several phases, which are described in the following sections.


### Creating
In this phase, a proposal cell is created. The `args` must follow the Type ID rule, as described in the `Script` section, and the `status` field must be 0. The `total_yes` field should be 0. The `origin_block_number` field should be 0.

The cell's `capacity` must be greater than or equal to `config.minimal_proposal_capacity`. If the proposal fails following a challenge, this capacity represents the assets to be lost.

The cell's lock script should be chosen from the initiator's pubkey, so that only the initiator can unlock the proposal cell and control the final operation.

### Updating to be finalized
In this phase, a proposal cell, together with some counting cells, is consumed to generate a finalized proposal cell. This can only happen after `config.vote_duration` blocks have elapsed since the proposal cell was created. `config.vote_duration` is a block count, so the script can validate this by checking that:
1. the `since` in the input cell is larger than `config.vote_duration`.
2. the `since` is a relative `since` with the block number metric, which makes it a block count comparable with `config.vote_duration`


The `args` should be kept the same, as the Type ID rule requires. The output lock script of the finalized proposal cell should be the `always success` lock script, so that it can be challenged by others.

The script then goes through all counting cells, which are identified by `config.counting_cell_code_hash`/`config.counting_cell_hash_type`. It checks that the `args` is the ckb-blake160-hash of the proposal cell. Then it loads the block numbers of all counting cells and verifies that they are greater than the proposal cell's block number plus `config.vote_duration`. Finally, it sums all "YES" values in the counting cells' cell data. If the sum is less than the `config.yes_threshold`, it fails. The `total_yes` field should be the sum.

The input capacity must equal the output capacity of the proposal cells. The bond serves as the challenger's incentive and must not be drained. 

Then it checks the hash ranges of all counting cells: they must not overlap. If any value v satisfies h1 <= v <= h2 and h3 <= v <= h4, then the hash ranges [h1, h2] and [h3, h4] overlap.

The `origin_block_number` should be equal to the block number in which the proposal cell is created.

Finally, the `status` field in cell data should changed from `0`("proposal") to `1`("finalized").

### Updating to be passed
After `config.challenge_time` blocks have elapsed since the proposal cell was finalized, the finalized proposal cell can be updated to a passed proposal cell. `config.challenge_time` is a block count, compared with the relative `since` of the finalized proposal input.
The `status` field in the cell data should change from `1` ("finalized") to `2` ("passed").
See [RFC](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0017-tx-valid-since/0017-tx-valid-since.md) for more information.

### Receiving Assets
The passed proposal cell, together with a treasury provider (not described in this spec), can generate a new cell that holds `request_amount` assets and is locked by the script identified by `recipient_lock_hash`. The proposal cell must be consumed entirely in this transaction. The treasury cell supplies the assets, though it is not described here. 

The treasury cell checks the proposal cell for the following:
1. `code_hash`
2. `hash_type`
3. the first 20 bytes of the config id
4. the proposal cell's `status` is "passed"


### Updating to be challenged
This process is identical to the `Updating to be finalized` phase, except for the following:
* All counting cells should have a direction of "NO"

It sums all "NO" values (called `total_no`) in the counting cells' cell data. The challenge succeeds, and the transaction is accepted, when `total_no` is greater than or equal to the `total_yes` recorded in the finalized proposal cell (`total_no >= total_yes`). It fails with `ChallengeNotMet` when fewer "NO" shannons are certified and the recycling window has not elapsed yet.

The `status` field in input cell data should be `1`("finalized").

When a challenge succeeds, the finalized proposal cell is consumed and burned, and the challenger receives all assets in the proposal cell as an incentive. The receiver's lock script must be one of the lock scripts used in the counting cells. The script verifies that no output cell carries the same proposal type script.

### Recycling the Proposal Cell
Once the sum of `config.vote_duration` and `config.challenge_time` (both block counts) has elapsed, the initiator can consume the proposal cell and recycle its assets if the proposal fails to pass.
The script can validate this by checking that:
1. the `since` in the input cell is larger than the sum of `config.vote_duration` and `config.challenge_time`.
2. the `since` is a relative `since` with the block number metric, which makes it a block count comparable with that sum.

The transaction must not include an output with a type script identical to the consumed proposal type script, so that the proposal cell is burned.

### Veto

If an input lock script's hash matches `config.veto_lock_script_hash`, a finalized proposal cell can be consumed and burned. This mechanism allows the administrator (represented by `config.veto_lock_script_hash`) to cancel a proposal.
