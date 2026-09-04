#![no_std]
#![no_main]

ckb_std::entry!(program_entry);
ckb_std::default_alloc!(16384, 1258306, 64);

use ckb_std::{
    ckb_constants::Source,
    ckb_types::prelude::Entity,
    high_level::{
        QueryIter, load_cell_capacity, load_cell_data, load_cell_lock_hash, load_cell_type,
        load_cell_type_hash, load_script, load_script_hash, load_witness_args,
    },
};
use treasury_common::{
    ProposalConfig, ProposalData, ProposalOutcome, ProposalPhase, ResultData, TallyPhase,
    TallyState, blake2b_256,
};

const TREASURY_ACTION_PAYOUT: u8 = 1;

#[repr(i8)]
enum Error {
    ArgsInvalid = 1,
    InvalidCellCount,
    ConfigNotFound,
    ConfigInvalid,
    ResultInvalid,
    ProposalNotFound,
    CandidateNotFound,
    ResultMismatch,
    InvalidPayout,
    ProposalPolicyMismatch,
    GuardianMissing,
    ResultLockInvalid,
    VetoedResultImmutable,
}

pub fn program_entry() -> i8 {
    match run() {
        Ok(()) => 0,
        Err(error) => error as i8,
    }
}

fn run() -> Result<(), Error> {
    let script = load_script().map_err(|_| Error::ArgsInvalid)?;
    let config_type_hash: [u8; 32] = script
        .args()
        .raw_data()
        .as_ref()
        .try_into()
        .map_err(|_| Error::ArgsInvalid)?;
    let inputs = QueryIter::new(load_cell_type_hash, Source::GroupInput).count();
    let outputs = QueryIter::new(load_cell_type_hash, Source::GroupOutput).count();
    if inputs > 1 || outputs > 1 || inputs == outputs {
        return Err(Error::InvalidCellCount);
    }

    let (config, config_data) = load_config(config_type_hash)?;
    if load_script_hash()
        .map_err(|_| Error::ArgsInvalid)?
        .as_slice()
        != config.policy_type_hash
    {
        return Err(Error::ProposalPolicyMismatch);
    }
    if outputs == 1 {
        create_result(config_type_hash, config, &config_data)
    } else {
        consume_result(config)
    }
}

fn create_result(
    config_type_hash: [u8; 32],
    config: ProposalConfig,
    config_data: &[u8],
) -> Result<(), Error> {
    let result_data = load_cell_data(0, Source::GroupOutput).map_err(|_| Error::ResultInvalid)?;
    let result = ResultData::decode(&result_data).map_err(|_| Error::ResultInvalid)?;
    if result.proposal_config_data_hash != blake2b_256(config_data) {
        return Err(Error::ResultMismatch);
    }

    let mut proposal = None;
    for (index, type_script) in QueryIter::new(load_cell_type, Source::Input).enumerate() {
        let Some(type_script) = type_script else {
            continue;
        };
        if type_script.calc_script_hash().as_slice() != result.proposal_id {
            continue;
        }
        let data = load_cell_data(index, Source::Input).map_err(|_| Error::ProposalNotFound)?;
        let parsed = ProposalData::decode(&data).map_err(|_| Error::ProposalNotFound)?;
        if proposal.replace((index, parsed, type_script)).is_some() {
            return Err(Error::ProposalNotFound);
        }
    }
    let (proposal_index, proposal, proposal_script) = proposal.ok_or(Error::ProposalNotFound)?;
    if proposal.proposal_config_type_hash != config_type_hash
        || proposal_script.code_hash().as_slice() != config.proposal_code_hash
        || proposal_script.hash_type().as_slice()[0] != config.proposal_hash_type
    {
        return Err(Error::ProposalPolicyMismatch);
    }

    if result.proposal_id != proposal_script.calc_script_hash().as_slice()
        || result.requested_amount != proposal.requested_amount
        || result.receiver_lock_hash != proposal.receiver_lock_hash
        || load_cell_capacity(0, Source::GroupOutput).map_err(|_| Error::ResultLockInvalid)?
            != load_cell_capacity(proposal_index, Source::Input)
                .map_err(|_| Error::ResultLockInvalid)?
        || load_cell_lock_hash(proposal_index, Source::Input)
            .map_err(|_| Error::ResultLockInvalid)?
            != config.proposal_lock_hash
    {
        return Err(Error::ResultMismatch);
    }

    if result.is_vetoed() {
        return validate_veto(&config);
    }
    if proposal.phase != ProposalPhase::Closed
        || load_cell_lock_hash(0, Source::GroupOutput).map_err(|_| Error::ResultLockInvalid)?
            != proposal.proposer_lock_hash
    {
        return Err(Error::ResultLockInvalid);
    }

    let mut candidate = None;
    for (index, type_script) in QueryIter::new(load_cell_type, Source::Input).enumerate() {
        if type_script.as_ref().is_some_and(|script| {
            script.code_hash().as_slice() == config.tally_code_hash
                && script.hash_type().as_slice()[0] == config.tally_hash_type
        }) {
            let data =
                load_cell_data(index, Source::Input).map_err(|_| Error::CandidateNotFound)?;
            let parsed = TallyState::decode(&data).map_err(|_| Error::CandidateNotFound)?;
            if parsed.phase == TallyPhase::Candidate
                && parsed.proposal_id == result.proposal_id
                && candidate.replace((parsed, data)).is_some()
            {
                return Err(Error::CandidateNotFound);
            }
        }
    }
    let (candidate, candidate_data) = candidate.ok_or(Error::CandidateNotFound)?;
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
    {
        return Err(Error::ResultMismatch);
    }
    Ok(())
}

fn validate_veto(config: &ProposalConfig) -> Result<(), Error> {
    if load_cell_lock_hash(0, Source::GroupOutput).map_err(|_| Error::ResultLockInvalid)?
        != config.proposal_bond_burn_lock_hash
    {
        return Err(Error::ResultLockInvalid);
    }
    if QueryIter::new(load_cell_lock_hash, Source::Input)
        .any(|lock_hash| lock_hash == config.guardian_lock_hash)
    {
        Ok(())
    } else {
        Err(Error::GuardianMissing)
    }
}

fn consume_result(config: ProposalConfig) -> Result<(), Error> {
    let result_data = load_cell_data(0, Source::GroupInput).map_err(|_| Error::ResultInvalid)?;
    let result = ResultData::decode(&result_data).map_err(|_| Error::ResultInvalid)?;
    if result.is_vetoed() {
        return Err(Error::VetoedResultImmutable);
    }
    if !result.is_passed() {
        return Ok(());
    }
    let treasury_index = QueryIter::new(load_cell_lock_hash, Source::Input)
        .enumerate()
        .find_map(|(index, lock_hash)| (lock_hash == config.treasury_lock_hash).then_some(index))
        .ok_or(Error::InvalidPayout)?;
    let witness =
        load_witness_args(treasury_index, Source::Input).map_err(|_| Error::InvalidPayout)?;
    let action = witness
        .lock()
        .to_opt()
        .ok_or(Error::InvalidPayout)?
        .raw_data();
    if action.as_ref() != [TREASURY_ACTION_PAYOUT] {
        return Err(Error::InvalidPayout);
    }
    Ok(())
}

fn load_config(config_type_hash: [u8; 32]) -> Result<(ProposalConfig, alloc::vec::Vec<u8>), Error> {
    for (index, type_hash) in QueryIter::new(load_cell_type_hash, Source::CellDep).enumerate() {
        if type_hash == Some(config_type_hash) {
            let data = load_cell_data(index, Source::CellDep).map_err(|_| Error::ConfigInvalid)?;
            let config = ProposalConfig::decode(&data).map_err(|_| Error::ConfigInvalid)?;
            return Ok((config, data));
        }
    }
    Err(Error::ConfigNotFound)
}
