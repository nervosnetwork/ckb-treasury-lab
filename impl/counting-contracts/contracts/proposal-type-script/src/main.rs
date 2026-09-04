#![no_std]
#![no_main]

ckb_std::entry!(program_entry);
ckb_std::default_alloc!(16384, 1258306, 64);

use ckb_std::{
    ckb_constants::Source,
    ckb_types::prelude::{Entity, Unpack},
    high_level::{
        QueryIter, load_cell_capacity, load_cell_data, load_cell_lock_hash, load_cell_type,
        load_cell_type_hash, load_header, load_input_since, load_script, load_script_hash,
    },
    since::{LockValue, Since},
    type_id::check_type_id,
};
use counting_common::{
    CountingCellData, CountingConfig, Hash, ProposalData, ProposalOutcome, ProposalPhase,
    ResultData, blake2b_256,
};

#[repr(i8)]
enum Error {
    TypeIdInvalid = 1,
    InvalidCellCount,
    InvalidProposalData,
    InvalidTransition,
    VotingStillOpen,
    ConfigNotFound,
    ConfigInvalid,
    ContractIdentityMismatch,
    ChallengePeriodTooShort,
    RequestedAmountTooLarge,
    ProposalBondTooSmall,
    ProposalLockInvalid,
    ProposerMissing,
    CountingCellInvalid,
    CountingRangesInvalid,
    CountingOwnersInvalid,
    CountingAmountOverflow,
    PassingRuleNotMet,
    ChallengeRuleNotMet,
    ChallengePeriodOpen,
    ResultMissing,
    ResultMismatch,
    ResultLockInvalid,
    GuardianMissing,
    ProposalNotExpired,
}

pub fn program_entry() -> i8 {
    match run() {
        Ok(()) => 0,
        Err(error) => error as i8,
    }
}

fn run() -> Result<(), Error> {
    check_type_id(0, 32).map_err(|_| Error::TypeIdInvalid)?;
    let inputs = QueryIter::new(load_cell_type_hash, Source::GroupInput).count();
    let outputs = QueryIter::new(load_cell_type_hash, Source::GroupOutput).count();
    match (inputs, outputs) {
        (0, 1) => create(),
        (1, 1) => update(),
        (1, 0) => terminate(),
        _ => Err(Error::InvalidCellCount),
    }
}

fn create() -> Result<(), Error> {
    let proposal = load_proposal(0, Source::GroupOutput)?;
    if proposal.phase != ProposalPhase::Open {
        return Err(Error::InvalidTransition);
    }
    let (config, _) = load_config(proposal.config_type_hash)?;
    ensure_proposal_identity(&config)?;
    if proposal.challenge_period < config.minimum_challenge_period {
        return Err(Error::ChallengePeriodTooShort);
    }
    if proposal.requested_amount > config.maximum_proposal_amount {
        return Err(Error::RequestedAmountTooLarge);
    }
    if load_cell_capacity(0, Source::GroupOutput).map_err(|_| Error::ProposalBondTooSmall)?
        < config.minimum_proposal_bond
    {
        return Err(Error::ProposalBondTooSmall);
    }
    if load_cell_lock_hash(0, Source::GroupOutput).map_err(|_| Error::ProposalLockInvalid)?
        != config.proposal_lock_hash
    {
        return Err(Error::ProposalLockInvalid);
    }
    require_input_lock(proposal.proposer_lock_hash, Error::ProposerMissing)
}

fn update() -> Result<(), Error> {
    let input = load_proposal(0, Source::GroupInput)?;
    let output = load_proposal(0, Source::GroupOutput)?;
    let (config, _) = load_config(input.config_type_hash)?;
    ensure_proposal_identity(&config)?;
    ensure_proposal_cell_preserved(&input, &output)?;
    match (input.phase, output.phase) {
        (ProposalPhase::Open, ProposalPhase::Closed) => close(&input, &output),
        (ProposalPhase::Closed, ProposalPhase::Finalized) => finalize_yes(&input, &output, &config),
        _ => Err(Error::InvalidTransition),
    }
}

fn close(input: &ProposalData, output: &ProposalData) -> Result<(), Error> {
    if input.certified_yes_amount != 0
        || input.certified_yes_vote_count != 0
        || output.certified_yes_amount != 0
        || output.certified_yes_vote_count != 0
    {
        return Err(Error::InvalidTransition);
    }
    let latest_header = QueryIter::new(load_header, Source::HeaderDep)
        .map(|header| -> u64 { header.raw().number().unpack() })
        .max()
        .ok_or(Error::VotingStillOpen)?;
    if latest_header < input.end_block {
        return Err(Error::VotingStillOpen);
    }
    Ok(())
}

fn finalize_yes(
    input: &ProposalData,
    output: &ProposalData,
    config: &CountingConfig,
) -> Result<(), Error> {
    require_input_lock(input.proposer_lock_hash, Error::ProposerMissing)?;
    let proposal_id = load_script_hash().map_err(|_| Error::InvalidTransition)?;
    let (yes, vote_count, owner_lock) = aggregate_counting_inputs(config, proposal_id, 1)?;
    if owner_lock != input.proposer_lock_hash {
        return Err(Error::CountingOwnersInvalid);
    }
    if output.certified_yes_amount != yes || output.certified_yes_vote_count != vote_count {
        return Err(Error::ResultMismatch);
    }
    if !config.passes(yes, 0, input.requested_amount) {
        return Err(Error::PassingRuleNotMet);
    }
    Ok(())
}

fn terminate() -> Result<(), Error> {
    let proposal = load_proposal(0, Source::GroupInput)?;
    let (config, config_data) = load_config(proposal.config_type_hash)?;
    ensure_proposal_identity(&config)?;
    let proposal_id = load_script_hash().map_err(|_| Error::InvalidTransition)?;
    let (result_index, result) = load_result(&config)?;
    if result.proposal_id != proposal_id
        || result.requested_amount != proposal.requested_amount
        || result.receiver_lock_hash != proposal.receiver_lock_hash
        || result.proposal_config_data_hash != blake2b_256(&config_data)
        || load_cell_capacity(result_index, Source::Output).map_err(|_| Error::ResultMismatch)?
            != load_cell_capacity(0, Source::GroupInput).map_err(|_| Error::ResultMismatch)?
    {
        return Err(Error::ResultMismatch);
    }

    if result.outcome == ProposalOutcome::Vetoed {
        return veto(&config, result_index);
    }
    if result.outcome == ProposalOutcome::Expired {
        return expire(&proposal, &config, result_index, &result);
    }
    if proposal.phase != ProposalPhase::Finalized
        || result.certified_yes_amount != proposal.certified_yes_amount
        || result.final_state_hash
            != blake2b_256(&proposal.encode().map_err(|_| Error::InvalidProposalData)?)
        || result.veto_reason_hash != [0; 32]
    {
        return Err(Error::ResultMismatch);
    }

    match result.outcome {
        ProposalOutcome::Passed => settle_passed(
            &proposal,
            &config,
            result_index,
            result.challenging_no_amount,
        ),
        ProposalOutcome::RejectedByVote => settle_challenge(
            &proposal,
            &config,
            proposal_id,
            result_index,
            result.challenging_no_amount,
        ),
        ProposalOutcome::Vetoed | ProposalOutcome::Expired => unreachable!(),
        ProposalOutcome::Paid | ProposalOutcome::RejectionClaimed => Err(Error::ResultMismatch),
    }
}

fn expire(
    proposal: &ProposalData,
    config: &CountingConfig,
    result_index: usize,
    result: &ResultData,
) -> Result<(), Error> {
    if proposal.phase != ProposalPhase::Closed
        || result.certified_yes_amount != 0
        || result.challenging_no_amount != 0
        || result.final_state_hash
            != blake2b_256(&proposal.encode().map_err(|_| Error::InvalidProposalData)?)
        || result.veto_reason_hash != [0; 32]
        || load_cell_lock_hash(result_index, Source::Output)
            .map_err(|_| Error::ResultLockInvalid)?
            != config.proposal_bond_burn_lock_hash
    {
        return Err(Error::ResultMismatch);
    }
    let since =
        Since::new(load_input_since(0, Source::GroupInput).map_err(|_| Error::ProposalNotExpired)?);
    if !since.flags_is_valid()
        || !since.is_relative()
        || !matches!(
            since.extract_lock_value(),
            Some(LockValue::BlockNumber(blocks)) if blocks >= proposal.challenge_period
        )
    {
        return Err(Error::ProposalNotExpired);
    }
    Ok(())
}

fn settle_passed(
    proposal: &ProposalData,
    config: &CountingConfig,
    result_index: usize,
    challenging_no_amount: u128,
) -> Result<(), Error> {
    if challenging_no_amount != 0 || has_counting_inputs(config) {
        return Err(Error::ResultMismatch);
    }
    let since = Since::new(
        load_input_since(0, Source::GroupInput).map_err(|_| Error::ChallengePeriodOpen)?,
    );
    if !since.flags_is_valid()
        || !since.is_relative()
        || !matches!(
            since.extract_lock_value(),
            Some(LockValue::BlockNumber(blocks)) if blocks >= proposal.challenge_period
        )
    {
        return Err(Error::ChallengePeriodOpen);
    }
    if load_cell_lock_hash(result_index, Source::Output).map_err(|_| Error::ResultLockInvalid)?
        != proposal.proposer_lock_hash
    {
        return Err(Error::ResultLockInvalid);
    }
    Ok(())
}

fn settle_challenge(
    proposal: &ProposalData,
    config: &CountingConfig,
    proposal_id: Hash,
    result_index: usize,
    claimed_no: u128,
) -> Result<(), Error> {
    let (no, _, challenger_lock) = aggregate_counting_inputs(config, proposal_id, 0)?;
    if no != claimed_no {
        return Err(Error::ResultMismatch);
    }
    if config.passes(proposal.certified_yes_amount, no, proposal.requested_amount) {
        return Err(Error::ChallengeRuleNotMet);
    }
    if load_cell_lock_hash(result_index, Source::Output).map_err(|_| Error::ResultLockInvalid)?
        != challenger_lock
    {
        return Err(Error::ResultLockInvalid);
    }
    Ok(())
}

fn veto(config: &CountingConfig, result_index: usize) -> Result<(), Error> {
    require_input_lock(config.guardian_lock_hash, Error::GuardianMissing)?;
    if load_cell_lock_hash(result_index, Source::Output).map_err(|_| Error::ResultLockInvalid)?
        != config.proposal_bond_burn_lock_hash
    {
        return Err(Error::ResultLockInvalid);
    }
    Ok(())
}

fn aggregate_counting_inputs(
    config: &CountingConfig,
    proposal_id: Hash,
    direction: u8,
) -> Result<(u128, u64, Hash), Error> {
    let mut found = false;
    let mut previous_end = None;
    let mut amount = 0u128;
    let mut vote_count = 0u64;
    let mut owner_lock = None;
    for (index, type_script) in QueryIter::new(load_cell_type, Source::Input).enumerate() {
        let Some(type_script) = type_script else {
            continue;
        };
        if type_script.code_hash().as_slice() != config.counting_code_hash
            || type_script.hash_type().as_slice()[0] != config.counting_hash_type
        {
            continue;
        }
        if type_script.args().raw_data().as_ref() != proposal_id {
            return Err(Error::CountingCellInvalid);
        }
        let data = load_cell_data(index, Source::Input).map_err(|_| Error::CountingCellInvalid)?;
        let counting = CountingCellData::decode(&data).map_err(|_| Error::CountingCellInvalid)?;
        if counting.direction != direction {
            return Err(Error::CountingCellInvalid);
        }
        let current_owner =
            load_cell_lock_hash(index, Source::Input).map_err(|_| Error::CountingCellInvalid)?;
        if owner_lock.is_some_and(|owner| owner != current_owner) {
            return Err(Error::CountingOwnersInvalid);
        }
        owner_lock = Some(current_owner);
        if previous_end.is_some_and(|end| counting.range_start <= end) {
            return Err(Error::CountingRangesInvalid);
        }
        previous_end = Some(counting.range_end);
        amount = amount
            .checked_add(counting.amount)
            .ok_or(Error::CountingAmountOverflow)?;
        vote_count = vote_count
            .checked_add(counting.vote_count as u64)
            .ok_or(Error::CountingAmountOverflow)?;
        found = true;
    }
    if !found {
        return Err(Error::CountingCellInvalid);
    }
    Ok((
        amount,
        vote_count,
        owner_lock.ok_or(Error::CountingCellInvalid)?,
    ))
}

fn has_counting_inputs(config: &CountingConfig) -> bool {
    QueryIter::new(load_cell_type, Source::Input).any(|type_script| {
        type_script.is_some_and(|type_script| {
            type_script.code_hash().as_slice() == config.counting_code_hash
                && type_script.hash_type().as_slice()[0] == config.counting_hash_type
        })
    })
}

fn ensure_proposal_cell_preserved(
    input: &ProposalData,
    output: &ProposalData,
) -> Result<(), Error> {
    if !input.immutable_fields_equal(output)
        || load_cell_capacity(0, Source::GroupInput).map_err(|_| Error::InvalidTransition)?
            != load_cell_capacity(0, Source::GroupOutput).map_err(|_| Error::InvalidTransition)?
        || load_cell_lock_hash(0, Source::GroupInput).map_err(|_| Error::InvalidTransition)?
            != load_cell_lock_hash(0, Source::GroupOutput).map_err(|_| Error::InvalidTransition)?
    {
        return Err(Error::InvalidTransition);
    }
    Ok(())
}

fn ensure_proposal_identity(config: &CountingConfig) -> Result<(), Error> {
    let script = load_script().map_err(|_| Error::ContractIdentityMismatch)?;
    if script.code_hash().as_slice() == config.proposal_code_hash
        && script.hash_type().as_slice()[0] == config.proposal_hash_type
    {
        Ok(())
    } else {
        Err(Error::ContractIdentityMismatch)
    }
}

fn load_result(config: &CountingConfig) -> Result<(usize, ResultData), Error> {
    let mut result = None;
    for (index, type_hash) in QueryIter::new(load_cell_type_hash, Source::Output).enumerate() {
        if type_hash == Some(config.policy_type_hash) {
            let data = load_cell_data(index, Source::Output).map_err(|_| Error::ResultMissing)?;
            let parsed = ResultData::decode(&data).map_err(|_| Error::ResultMissing)?;
            if result.replace((index, parsed)).is_some() {
                return Err(Error::ResultMissing);
            }
        }
    }
    result.ok_or(Error::ResultMissing)
}

fn require_input_lock(lock_hash: Hash, error: Error) -> Result<(), Error> {
    if QueryIter::new(load_cell_lock_hash, Source::Input).any(|input| input == lock_hash) {
        Ok(())
    } else {
        Err(error)
    }
}

fn load_proposal(index: usize, source: Source) -> Result<ProposalData, Error> {
    let data = load_cell_data(index, source).map_err(|_| Error::InvalidProposalData)?;
    ProposalData::decode(&data).map_err(|_| Error::InvalidProposalData)
}

fn load_config(config_type_hash: Hash) -> Result<(CountingConfig, alloc::vec::Vec<u8>), Error> {
    for (index, type_hash) in QueryIter::new(load_cell_type_hash, Source::CellDep).enumerate() {
        if type_hash == Some(config_type_hash) {
            let data = load_cell_data(index, Source::CellDep).map_err(|_| Error::ConfigInvalid)?;
            let config = CountingConfig::decode(&data).map_err(|_| Error::ConfigInvalid)?;
            return Ok((config, data));
        }
    }
    Err(Error::ConfigNotFound)
}
