# Vote Type Script Specification
This type script identifies a vote cell, through which any user can cast a vote.

## Script

```
code_hash: <code_hash of vote type script>
hash_type: <hash_type of vote type script>
args: <20 bytes, ckb-blake160-hash of proposal type script>
```
The `args` is the ckb-blake160-hash of the proposal type script for which the vote is cast.


## Witness
No Witness is required.


## Cell Data
The cell data has the following structure in molecule format:
```
table Vote {
    vote_amount: Uint64,
    direction: byte,
}
```
The `vote_amount` is the sum of all related DAO deposit amounts, as described later.
The `direction` is `0` for "NO" or `1` for "YES". A "NO" vote can challenge the final results.

## Processing
The vote cell's lock script should represent the voter's identity and must be unlocked in the input cells. The other cells in this transaction can't be vote cell.

The script iterates over all cell_deps to find the proposal type script whose hash matches `args`; if none is found, the script fails. The referenced cell must be a proposal cell, i.e. the `status` in its cell data must be 0 ("proposal"), so a vote can only be cast before the proposal cell is finalized. 


Iterating from 0 up to `end_of_dao_deposit` (exclusive), as described below, the script collects all DAO deposit cells whose lock script matches the voter's lock script. A cell is counted as a DAO deposit only if its type script is the Nervos DAO type script, checked via `code_hash` and `hash_type`; cells with any other type script are ignored. The sum of all these DAO deposits must equal the `vote_amount` in the cell data. The `vote_amount` must be greater than or equal to `config.minimal_vote_amount`. All DAO deposit cells should be older than the proposal cell, as determined by comparing the `number` in the `header` (block number).

Since items in `cell_deps` can be duplicated after `dep_groups` are expanded, the script must ensure that no OutPoint appears more than once in `cell_deps`. The algorithm is as follows:
1. Load the transaction using the syscall.
2. Parse the transaction in molecule and iterate over all `cell_deps`.
3. Upon reaching a `cell_dep` whose type is `dep_group`, or the end of the list, mark the current index as `end_of_dao_deposit`.



## Others
Users must keep vote cells alive throughout the voting process. Any user can withdraw an existing vote by consuming the vote cell. A user may also cast a vote using only part of a DAO deposit: the script always counts a referenced deposit cell in full, so the voter uses a part of a larger deposit by splitting it first and referencing only the DAO deposit cell that holds that part.

