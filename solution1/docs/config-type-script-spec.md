# Config Type Script Specification
In this voting system, many configurations need to be set. We collect them into a dedicated config cell, whose type script is the config type script described here.

Any field can be referred to as `config.<field>` across design and specification documents.

## Script

```
code_hash: <code_hash to config type script>
hash_type: <hash_type to config type script>
args: <20 bytes, Type ID like args>
```

The `args` is the ckb-blake160-hash of the first CellInput structure of the creating transaction, combined with the output index of the cell. This makes the script unique across the entire blockchain. This mechanics is used in [Type ID](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0022-transaction-structure/0022-transaction-structure.md#type-id).

It performs [Type ID checking](https://github.com/nervosnetwork/ckb-std/blob/master/src/type_id.rs#L139) and verifies that the output cell exists.
This prevents the config cell from being burned by mistake.

## Cell Data
It has following structures in molecule format:

```
table VotingConfig {
    emergent_halt: byte,
    vote_code_hash: Byte32,
    vote_hash_type: byte,
    counting_code_hash: Byte32,
    counting_hash_type: byte,
    always_success_code_hash: Byte32,
    always_success_hash_type: byte,
    yes_threshold: Uint64,
    minimal_proposal_capacity: Uint64,
    vote_duration: Uint64,
    vote_window: Uint64,
    challenge_time: Uint64,
    veto_lock_script_hash: Byte32,
    minimal_vote_amount: Uint64,
}
```

The `vote_duration`, `vote_window` and `challenge_time` fields are plain block counts (not `since` values): a script compares them with the relative `since` of the proposal input, or with the block numbers that created the cells.

The `emergent_halt` is read by all type scripts (the proposal type script, the vote type script, and the count type script). When set to `1`, all scripts fail. Updating this field halts the voting system.


## Security
This cell is very important and should be locked by a very safe lock script, as compromising it can break the whole voting system. It is suggested to use multisig to lock this cell.

