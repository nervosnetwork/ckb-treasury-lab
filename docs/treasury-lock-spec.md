# Treasury Lock Script Specification

This spec describes a lock script used in a treasury system.


## Introduction

A cell with a treasury lock script is called a treasury cell. Treasury cells can only be created by consensus, similar to a [cell base](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0022-transaction-structure/0022-transaction-structure.md#exceptions). A treasury cell can be consumed via two methods: the burning method or the reward method.

## Script

The lock script has the following structure:

```
code_hash: <code hash to treasury lock script binary>
hash_type: Type
args: <8-byte block number>
```

The 8-byte block number is parsed as a 64-bit unsigned little-endian integer. It represents the block number at the time the cell was created, set by the CKB node.

## Witness

The corresponding witness must be in WitnessArgs molecule format. The full data structure is as follows:

```
WitnessArgs:
    lock: <1 byte consuming_method>
    input_type: <>
    output_type: <>
```

The 1-byte `consuming_method` can be `0` (burning method) or `1` (reward method).


## Unlocking Process

Depending on the `consuming_method`, the treasury lock script can be unlocked via one of two methods: burning or reward.

### Burning Method

When this method is chosen, the script iterates over all headers using `ckb_load_header` (with `source = 4, header deps`) to fetch all block numbers. It parses each header to extract the block number, and the largest block number found is treated as the `current block number`. The transaction creator should put the latest block number in header_deps.

A duration block count is then calculated by subtracting the block number in `args` from the `current block number`.

This duration must be greater than a predefined value called `burn_expiry_blocks`, which is stored in the config cell described later.

To encourage anyone to burn expired treasury cells, an incentive mechanism is included. The incentive amount is calculated as follows:

```
incentive amount = (duration block count - burn_expiry_blocks) * burn_incentive_rate + base_burn_incentive
```

The config parameters `burn_incentive_rate` and `base_burn_incentive` are also stored in the config cell. The incentive amount is denominated in shannons, not CKB.

This design ensures the incentive amount grows over time if no one burns the cell, addressing the case where transaction fees exceed the incentive amount.

The lock script takes the capacity of the current cell, subtracts the `incentive amount`, and finds a corresponding output cell with exactly that capacity. This output cell must have zero-length `script args`, `cell data`, and `type script`, with `code_hash` and `hash_type` both set to zero, meaning it is permanently unspendable (a zero lock).

The remaining capacity can be split between transaction fee and changes; it is the transaction creator's responsibility to distribute these.

### Reward Method

The reward method reads `proposal_code_hash` and `proposal_hash_type` which from the config cell. If a type script in input cells matches all of the following:

- `code_hash`
- `hash_type`

then the cell is unlocked.

The following rules must be followed:

1. Sort input treasury cells by capacity in decreasing order. Let N be the total cell count. The sum of capacity for the leading N - 1 cells must be less than the required CKBytes of the proposal. This prevents the inclusion of arbitrary treasury cells.
2. There must be exactly one change cell, locked by a zero lock, with empty script args and cell data. Its amount equals the total capacity of the N treasury cells minus the required CKBytes of the proposal.
3. The transaction fee is not covered by treasury cells. It may be provided by the proposal creator.

The reason treasury cells do not cover the transaction fee is that the fee is dynamic and cannot be determined in advance, so it is left to the proposal creator. In practice, the fee is typically very small.

At most one treasury cell may be burned per transaction. Since a new treasury cell is generated every block, the burned amount is small and acceptable.


## Config Cell

The config cell is deployed on-chain and locked by a different key than the treasury lock script, allowing config updates to be delegated to a separate team. Its type script must follow Type ID rules. The treasury lock script references the config cell by its type script hash, which is hard-coded.

The data in the config cell is encoded in molecule format as follows:

```
table {
    burn_expiry_blocks: Uint32,
    burn_incentive_rate: Uint64,
    base_burn_incentive: Uint64,
    proposal_code_hash: Byte32,
    proposal_hash_type: byte
}
```
`proposal_code_hash` and `proposal_hash_type` identify the proposal type script.

