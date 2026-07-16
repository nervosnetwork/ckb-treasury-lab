# ckb-treasury-lab

**NOTE** This is not a production project.

This project aims to discuss, experiment with, and design a treasury system on CKB.

The treasury consists of two systems: a creating system and a voting system.
The creating system mints CKB out of thin air, similar to a coinbase transaction.
The voting system votes on and allocates the treasury created in the previous step.
These two systems are independent and can be designed and implemented separately.

Specification as following:

- [proposal type script](docs/proposal-type-script.md)
- [vote type script](docs/vote-type-script.md)
- [treasury lock script](docs/treasury-lock-spec.md)

Implementation as following:
- [proposal type script](https://github.com/XuJiandong/ckb/tree/proposal-type-script)
- [vote type script](impl/contracts/vote-type-script/)
- treasury lock script: TODO

## How the scripts relate

These three scripts work together to form the voting system that allocates the treasury.

- The **proposal type script** guards a proposal cell. Anyone can create one to request a CKB amount. When it is consumed, it tallies votes over the proposal's block range and unlocks only if the proposal passes.
- The **vote type script** guards a vote cell. The proposal type script counts exactly these vote cells during its tally.
- The **treasury lock script** guards treasury cells (minted by consensus). Its reward method releases funds only when the transaction consumes a matching proposal cell.

In short: votes (vote type script) decide a proposal (proposal type script), and a passed proposal is what authorizes spending treasury funds (treasury lock script). The proposal and vote scripts are generic and reusable beyond the treasury.

