#![no_std]
#![no_main]

ckb_std::entry!(program_entry);
ckb_std::default_alloc!(16384, 1258306, 64);

use ckb_std::{
    ckb_constants::Source,
    ckb_types::prelude::{Entity, Unpack},
    high_level::{
        QueryIter, load_cell_capacity, load_cell_data, load_cell_lock_hash, load_cell_type,
        load_cell_type_hash, load_header, load_script, load_script_hash,
    },
    type_id::check_type_id,
};
use treasury_common::{
    ProposalConfig, ProposalData, ProposalOutcome, ProposalPhase, ResultData, TallyPhase,
    TallyState, blake2b_256,
};

#[repr(i8)]
enum Error {
    TypeIdInvalid = 1,
    InvalidCellCount,
    InvalidProposalData,
    InvalidTransition,
    VotingStillOpen,
    MissingCandidate,
    MissingResult,
    ResultMismatch,
    ConfigNotFound,
    ConfigInvalid,
    ContractIdentityMismatch,
    ChallengePeriodTooShort,
    ProposalLockInvalid,
    ProposerMissing,
    GuardianMissing,
    ResultLockInvalid,
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
    if inputs > 1 || outputs > 1 || (inputs == 0 && outputs == 0) {
        return Err(Error::InvalidCellCount);
    }

    match (inputs, outputs) {
        (0, 1) => create(),
        (1, 1) => close(),
        (1, 0) => finalize(),
        _ => Err(Error::InvalidTransition),
    }
}

fn create() -> Result<(), Error> {
    let proposal = load_proposal(0, Source::GroupOutput)?;
    if proposal.phase != ProposalPhase::Open {
        return Err(Error::InvalidTransition);
    }
    let (config, _) = load_proposal_config(proposal.proposal_config_type_hash)?;
    let script = load_script().map_err(|_| Error::ContractIdentityMismatch)?;
    if script.code_hash().as_slice() != config.proposal_code_hash
        || script.hash_type().as_slice()[0] != config.proposal_hash_type
    {
        return Err(Error::ContractIdentityMismatch);
    }
    if proposal.challenge_period < config.minimum_challenge_period {
        return Err(Error::ChallengePeriodTooShort);
    }
    if load_cell_lock_hash(0, Source::GroupOutput).map_err(|_| Error::ProposalLockInvalid)?
        != config.proposal_lock_hash
    {
        return Err(Error::ProposalLockInvalid);
    }
    require_input_lock(proposal.proposer_lock_hash, Error::ProposerMissing)?;
    Ok(())
}

fn close() -> Result<(), Error> {
    let input = load_proposal(0, Source::GroupInput)?;
    let output = load_proposal(0, Source::GroupOutput)?;
    if input.phase != ProposalPhase::Open
        || output.phase != ProposalPhase::Closed
        || !input.immutable_fields_equal(&output)
        || load_cell_capacity(0, Source::GroupInput).map_err(|_| Error::InvalidTransition)?
            != load_cell_capacity(0, Source::GroupOutput).map_err(|_| Error::InvalidTransition)?
        || load_cell_lock_hash(0, Source::GroupInput).map_err(|_| Error::InvalidTransition)?
            != load_cell_lock_hash(0, Source::GroupOutput).map_err(|_| Error::InvalidTransition)?
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

fn finalize() -> Result<(), Error> {
    let proposal = load_proposal(0, Source::GroupInput)?;
    let (config, config_data) = load_proposal_config(proposal.proposal_config_type_hash)?;
    let proposal_id = load_script_hash().map_err(|_| Error::InvalidTransition)?;
    if load_cell_lock_hash(0, Source::GroupInput).map_err(|_| Error::ProposalLockInvalid)?
        != config.proposal_lock_hash
    {
        return Err(Error::ProposalLockInvalid);
    }

    let proposal_capacity =
        load_cell_capacity(0, Source::GroupInput).map_err(|_| Error::ResultMismatch)?;
    let (result_index, result) = load_result_output(&config)?;
    if result.proposal_id != proposal_id
        || result.requested_amount != proposal.requested_amount
        || result.receiver_lock_hash != proposal.receiver_lock_hash
        || result.proposal_config_data_hash != blake2b_256(&config_data)
        || load_cell_capacity(result_index, Source::Output).map_err(|_| Error::ResultMismatch)?
            != proposal_capacity
    {
        return Err(Error::ResultMismatch);
    }

    if result.is_vetoed() {
        return veto(&config, result_index);
    }
    settle(&proposal, &config, &result, result_index, proposal_id)
}

fn veto(config: &ProposalConfig, result_index: usize) -> Result<(), Error> {
    require_input_lock(config.guardian_lock_hash, Error::GuardianMissing)?;
    if load_cell_lock_hash(result_index, Source::Output).map_err(|_| Error::ResultLockInvalid)?
        != config.proposal_bond_burn_lock_hash
    {
        return Err(Error::ResultLockInvalid);
    }
    Ok(())
}

fn settle(
    proposal: &ProposalData,
    config: &ProposalConfig,
    result: &ResultData,
    result_index: usize,
    proposal_id: [u8; 32],
) -> Result<(), Error> {
    if proposal.phase != ProposalPhase::Closed {
        return Err(Error::InvalidTransition);
    }

    let mut candidate = None;
    for (index, type_script) in QueryIter::new(load_cell_type, Source::Input).enumerate() {
        if is_tally_script(&type_script, config) {
            let data = load_cell_data(index, Source::Input).map_err(|_| Error::MissingCandidate)?;
            let tally = TallyState::decode(&data).map_err(|_| Error::MissingCandidate)?;
            if tally.phase == TallyPhase::Candidate && tally.proposal_id == proposal_id {
                if candidate.is_some() {
                    return Err(Error::MissingCandidate);
                }
                candidate = Some((tally, data));
            }
        }
    }
    let (candidate, candidate_data) = candidate.ok_or(Error::MissingCandidate)?;

    let expected_outcome = if config.passes(candidate.yes, candidate.no, proposal.requested_amount)
    {
        ProposalOutcome::Passed
    } else {
        ProposalOutcome::RejectedByVote
    };
    if result.outcome != expected_outcome
        || result.yes != candidate.yes
        || result.no != candidate.no
        || result.final_state_hash != blake2b_256(&candidate_data)
        || load_cell_lock_hash(result_index, Source::Output)
            .map_err(|_| Error::ResultLockInvalid)?
            != proposal.proposer_lock_hash
    {
        return Err(Error::ResultMismatch);
    }
    Ok(())
}

fn load_result_output(config: &ProposalConfig) -> Result<(usize, ResultData), Error> {
    let mut result = None;
    for (index, type_hash) in QueryIter::new(load_cell_type_hash, Source::Output).enumerate() {
        if type_hash == Some(config.policy_type_hash) {
            let data = load_cell_data(index, Source::Output).map_err(|_| Error::MissingResult)?;
            let parsed = ResultData::decode(&data).map_err(|_| Error::MissingResult)?;
            if result.replace((index, parsed)).is_some() {
                return Err(Error::MissingResult);
            }
        }
    }
    result.ok_or(Error::MissingResult)
}

fn require_input_lock(lock_hash: [u8; 32], error: Error) -> Result<(), Error> {
    if QueryIter::new(load_cell_lock_hash, Source::Input).any(|input| input == lock_hash) {
        Ok(())
    } else {
        Err(error)
    }
}

fn is_tally_script(
    script: &Option<ckb_std::ckb_types::packed::Script>,
    config: &ProposalConfig,
) -> bool {
    script.as_ref().is_some_and(|script| {
        script.code_hash().as_slice() == config.tally_code_hash
            && script.hash_type().as_slice()[0] == config.tally_hash_type
    })
}

fn load_proposal(index: usize, source: Source) -> Result<ProposalData, Error> {
    let data = load_cell_data(index, source).map_err(|_| Error::InvalidProposalData)?;
    ProposalData::decode(&data).map_err(|_| Error::InvalidProposalData)
}

fn load_proposal_config(
    config_type_hash: [u8; 32],
) -> Result<(ProposalConfig, alloc::vec::Vec<u8>), Error> {
    for (index, type_hash) in QueryIter::new(load_cell_type_hash, Source::CellDep).enumerate() {
        if type_hash == Some(config_type_hash) {
            let data = load_cell_data(index, Source::CellDep).map_err(|_| Error::ConfigInvalid)?;
            let config = ProposalConfig::decode(&data).map_err(|_| Error::ConfigInvalid)?;
            return Ok((config, data));
        }
    }
    Err(Error::ConfigNotFound)
}
