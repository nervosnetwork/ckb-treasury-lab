//! Error codes shared by the voting scripts.
//!
//! Each script returns `0` on success and the numeric value of one of these
//! variants on failure. Codes are grouped per script so that a failing
//! transaction can be diagnosed from the exit code alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i8)]
pub enum Error {
    // ---------------------------------------------------------------- generic
    /// The `args` of the script is not a 20 byte id.
    ArgsInvalid = 1,
    /// A syscall failed unexpectedly.
    SyscallError = 2,
    /// A molecule structure could not be decoded.
    EncodingInvalid = 3,
    /// A counted value overflowed.
    AmountOverflow = 4,
    /// A `since` value is not a relative, block number based value.
    SinceInvalid = 5,
    /// No config cell was found in `cell_deps`.
    ConfigCellNotFound = 6,
    /// The config cell data is not a well formed `VotingConfig`.
    ConfigCellInvalid = 7,
    /// More than one config cell was found in `cell_deps`.
    ConfigCellAmbiguous = 8,
    /// `config.emergent_halt` is set: the whole system is halted.
    EmergentHalt = 9,
    /// The cell's block header is not available in `header_deps`.
    HeaderMissing = 10,
    /// A Type ID check failed.
    TypeIdInvalid = 11,

    // ------------------------------------------------------ config type script
    /// The config cell may not be burned.
    ConfigCellBurned = 20,
    /// Only minting (`0 in / 1 out`) and updating (`1 in / 1 out`) are allowed.
    ConfigCellTransitionInvalid = 21,

    // -------------------------------------------------- proposal type script
    /// The proposal cell data is malformed or inconsistent.
    ProposalDataInvalid = 40,
    /// The requested status transition is not allowed.
    ProposalStatusInvalid = 41,
    /// The proposal bond is below `config.minimal_proposal_capacity`.
    ProposalCapacityTooSmall = 42,
    /// The bond capacity must survive the finalized / passed transitions.
    ProposalCapacityChanged = 43,
    /// `description`, `requested_amount` and `recipient_lock_hash` are
    /// immutable once created.
    ProposalFieldsChanged = 44,
    /// `config.vote_duration` has not elapsed yet.
    VoteDurationNotElapsed = 45,
    /// `config.challenge_time` has not elapsed yet.
    ChallengeTimeNotElapsed = 46,
    /// `vote_duration + challenge_time` has not elapsed yet.
    RecycleTooEarly = 47,
    /// The finalized cell must be locked by the always success lock.
    AlwaysSuccessLockRequired = 48,
    /// The sum of the "YES" counting cells is below `config.yes_threshold`.
    YesThresholdNotMet = 49,
    /// The "NO" counting cells do not outnumber the "YES" votes.
    ChallengeNotMet = 50,
    /// A referenced counting cell is invalid for this proposal.
    CountingCellInvalid = 51,
    /// The hash ranges of the counting cells overlap.
    CountingRangeOverlap = 52,
    /// No counting cell was referenced.
    CountingCellMissing = 53,
    /// A passed proposal must create an output for `recipient_lock_hash`.
    RecipientOutputMissing = 54,
    /// The recipient output holds less than `requested_amount`.
    RecipientAmountTooSmall = 55,
    /// A successful challenge does not pay a lock script used by its counting
    /// cells, so the challenger would not receive the bond.
    ChallengeRewardMissing = 56,
    /// A counting cell was created before `config.vote_duration` elapsed.
    CountingCellTooEarly = 57,

    // ------------------------------------------------------ vote type script
    /// The vote cell data is not a well formed `Vote`.
    VoteDataInvalid = 60,
    /// The proposal cell referenced by `args` was not found in `cell_deps`.
    ProposalCellNotFound = 61,
    /// The proposal cell is not in the `proposal` status any more.
    ProposalNotOpen = 62,
    /// The vote cell lock script is not unlocked by any input.
    VoterLockNotUnlocked = 63,
    /// No DAO deposit of the voter was referenced.
    DaoDepositMissing = 64,
    /// A DAO deposit is not older than the proposal cell.
    DaoDepositTooNew = 65,
    /// The declared amount does not match the referenced DAO deposits.
    VoteAmountMismatch = 66,
    /// More than one vote cell for the same proposal is created in one transaction.
    MultipleVoteCells = 67,
    /// The declared `vote_amount` is below `config.minimal_vote_amount`.
    VoteAmountTooSmall = 68,

    // -------------------------------------------------- counting type script
    /// The counting cell data is not a well formed `Counting`.
    CountingDataInvalid = 80,
    /// A hash range does not satisfy `start_hash <= end_hash`. Returned by the
    /// counting script for its own range, and by the proposal script when it
    /// aggregates the ranges of the referenced counting cells.
    CountingRangeInvalid = 81,
    /// A counting cell is immutable: it may only be created or consumed.
    CountingCellTransitionInvalid = 82,
    /// The proposal cell is not in a status that allows this counting cell.
    ProposalStatusInvalidForCounting = 83,
    /// A vote cell does not belong to the referenced proposal.
    VoteCellProposalMismatch = 84,
    /// A vote cell direction differs from the counting cell direction.
    VoteDirectionMismatch = 85,
    /// A voter lock hash falls outside `[start_hash, end_hash]`.
    VoteLockOutOfRange = 86,
    /// Two vote cells share the same lock script.
    VoteLockNotUnique = 87,
    /// A vote cell was cast outside `config.vote_window`.
    VoteOutsideWindow = 88,
    /// No vote cell was referenced.
    VoteCellMissing = 89,
    /// A referenced vote cell is invalid.
    VoteCellInvalid = 90,
}

/// Shorthand for the results returned by the voting scripts.
pub type Result<T> = core::result::Result<T, Error>;
