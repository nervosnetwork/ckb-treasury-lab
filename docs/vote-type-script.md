# Vote Type Script Specification
The vote type script is used together with the [proposal type script](./proposal-type-script.md). The `vote_cell_code_hash` and `vote_cell_hash_type` fields in that spec identify this vote type script.

## Script

```
code_hash: <vote type script code hash>
hash_type: <vote type script hash type>
args: <20 bytes blake160 hash of proposal type script>
```
The `args` field holds a blake160 hash of the proposal type script that this vote is cast for.
Since a proposal type script is unique across the entire chain, there is no ambiguity.


## Cell Data

The cell data uses the following molecule structure:
```
table Vote {
    vote: Byte,
    amount: Uint64,
    dao_index: Uint16Vec,
}
```
The `vote` field is a single byte: `0` for "NO" and `1` for "YES".
The `amount` is the total CKB the owner holds in DAO deposits, in shannon. The `dao_index` contains the indices into `cell_deps` that point to those DAO deposit cells.


## Witness
This script doesn't read witness.

## Unlocking process

1. When a vote cell is created, the action is treated as casting a vote. The script first checks that a proposal cell — identified by the blake160 hash stored in `args` — exists in `cell_deps`. Full validation of the proposal cell is not required; it only needs to confirm its presence.

2. The script then looks for a lock script on the input cells that matches the corresponding output lock script. This lock script represents ownership of the DAO.

3. Ensure that exactly one cell in the output contains this type script. If more than one such cell is present, the script must fail.

4. The script traverses all `cell_deps` to find cells that satisfy all of the following conditions:

- Its lock script matches the DAO owner.
- Its type script is the Nervos DAO, as described [here](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0024-ckb-genesis-script-list/0024-ckb-genesis-script-list.md#nervos-dao).
- Its index appears in `dao_index` from the cell data. For simplicity, `dep_group` is not supported here. There should be at least one element in `dao_index`.
- The total capacity in shannon across all matching DAO deposits equals the `amount` in the cell data.

If no such `cell_dep` exists, the script fails.


When a vote cell is consumed, there is no special meaning — it simply recycles the occupied CKB. The cell can be consumed at any time and does not need to wait until voting ends.

## Design Notes

* A "NO" vote is generally unnecessary — users can simply do nothing. However, it can be used to retract a previous "YES" vote: later votes from the same voter overwrite earlier ones, so casting a "NO" after a "YES" effectively cancels that prior vote. This is enforced by the proposal type script's vote tally, not by this script.

* Once a proposal cell is consumed, it can no longer be referenced in `cell_deps`, which prevents new votes from being cast after the proposal closes.

* the vote cells and referenced DAO deposits are not required to still be alive at settlement time.
  Vote cells may be consumed immediately after voting, while the DAO deposit must remain alive for `duration` blocks.

## Examples

### Example 1: Creating a Vote Cell (Casting a YES Vote)

```yaml
Cell Deps:
    Proposal_Cell                                       # the proposal this vote is cast for
        Data: <proposal cell data>
        Type:
            code_hash: <proposal type script code hash>
            hash_type: <proposal type script hash type>
            args:
                <20-byte blake160 Type ID>
        Lock:
            <always-success lock script>

    DAO_Deposit_Cell                                    # voter's DAO deposit, referenced to prove vote weight
        Data: <DAO deposit data>
        Type:
            code_hash: <Nervos DAO code hash>           # genesis DAO type script
            hash_type: 0x01                             # type
            args: <empty>
        Lock:
            <voter's lock script>                       # must match the lock on Vote_Cell output

Inputs:
    Funding_Cell
        Data: <empty>
        Type: <none>
        Lock:
            <voter's lock script>                       # same lock as DAO_Deposit_Cell and Vote_Cell output

Outputs:
    Vote_Cell
        Data: <molecule-encoded Vote>
            vote: 0x01                                  # 1 = YES
            amount: <voter's total DAO balance in shannon>
            dao_index: [1]                              # index 1 in cell_deps (DAO_Deposit_Cell)
        Type:
            code_hash: <vote type script code hash>
            hash_type: <vote type script hash type>
            args:
                <20-byte blake160 of proposal type script>  # identifies which proposal this vote is for
        Lock:
            <voter's lock script>

    <any> Change_Cell
        Data: <empty>
        Type: <none>
        Lock:
            <voter's lock script>

Witnesses:
    WitnessArgs structure (at index matching Funding_Cell input):
        Lock: <voter's signature>
        Input Type: <none>
        Output Type: <none>                             # vote type script does not read witness
```

---

### Example 2: Consuming a Vote Cell (Recycling CKB)

```yaml
Inputs:
    Vote_Cell                                           # the previously cast vote
        Data: <molecule-encoded Vote>
            vote: 0x01                                  # 1 = YES (original vote content)
            amount: <voter's total DAO balance in shannon>
            dao_index: [1]
        Type:
            code_hash: <vote type script code hash>
            hash_type: <vote type script hash type>
            args:
                <20-byte blake160 of proposal type script>
        Lock:
            <voter's lock script>

Outputs:
    <any> Change_Cell
        Data: <empty>
        Type: <none>
        Lock:
            <voter's lock script>                       # voter reclaims the occupied CKB

Witnesses:
    WitnessArgs structure (at index matching Vote_Cell input):
        Lock: <voter's signature>
        Input Type: <none>
        Output Type: <none>                             # vote type script does not read witness
```
