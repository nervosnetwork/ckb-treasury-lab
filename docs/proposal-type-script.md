# Proposal and Settlement Scripts V2

The canonical system design is in
[On-chain Voting with Optimistic Batch Settlement](./on-chain-voting-with-batch-settlement.md).

The Proposal Type Script is an ordinary Rust CKB contract, not an embedded or
node-native script. It enforces Type ID uniqueness, `Open -> Closed` transition,
immutable proposal fields, proposal-bond capacity preservation, and two
mutually exclusive terminal transitions: normal settlement or Guardian veto.

Creation also requires the immutable Proposal Config Cell identified by
`proposal_config_type_hash`. Proposal data contains no DAO, Vote, Tally, or
Policy identity of its own. The current Proposal script must match the version
authorized by Proposal Config; all later Vote, Tally, Result, and Treasury paths
resolve their protocol identities from that same immutable cell. Proposal
Config is therefore the single protocol-identity source rather than a second
copy checked against proposer-supplied fields.

Proposal data contains proposal-specific limits, amount, receiver, voting and
challenge windows, the Proposal Config reference, a proposer lock hash, and a
metadata commitment. The Proposal Config commits a canonical permissionless
Proposal lock, Guardian lock, and Proposal-bond burn lock. Only the lifecycle
phase may change when an Open Proposal becomes Closed.

TallyChainCells are independent Type-ID cells. Each batch verifies historical
transaction CBMT proofs, one unified SMT transition, and the ordered reducer
inside CKB-VM. Every TallyChain transaction includes the referenced Proposal
Config Cell and verifies both the Proposal and Tally code identities against it.
The final candidate has one challenge period. A valid omitted-vote proof
consumes the candidate and transfers its bond to the
lock of the first non-Candidate input in the challenge transaction. A mature
candidate can create one Result Cell under the Policy Type Script authorized by
Proposal Config.

Normal settlement returns the full Proposal Cell capacity in a Result Cell
locked to the committed proposer. The Policy output records either `Passed` or
`RejectedByVote` from the configured tally rule. A Guardian veto may instead
consume either an Open or Closed Proposal before settlement. It requires a
Guardian-lock input and creates an immutable `Vetoed` Result under the configured
burn lock with the complete Proposal bond. Because both paths consume the same
Proposal Type-ID singleton, only one can commit.

The immutable Vetoed Result remains available as a CellDep. It permits cleanup
of every Active or Candidate TallyChainCell for the same Proposal only when each
complete bond is returned to that tally state's recorded operator lock hash.
Active cleanup still requires the operator lock; Candidate cleanup is
permissionless under the configured Candidate lock. A rejected-by-vote Result
remains consumable so the proposer can reclaim the normal Proposal bond; a
Vetoed Result does not.
