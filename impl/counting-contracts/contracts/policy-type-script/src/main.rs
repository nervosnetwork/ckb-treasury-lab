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
use counting_common::{
    CountingConfig, Hash, ProposalData, ProposalOutcome, ProposalPhase, ResultData, blake2b_256,
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
    let config_type_hash: Hash = script
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
    if load_script_hash().map_err(|_| Error::ArgsInvalid)? != config.policy_type_hash {
        return Err(Error::ProposalPolicyMismatch);
    }
    if outputs == 1 {
        create_result(config_type_hash, &config, &config_data)
    } else {
        consume_result(&config)
    }
}

fn create_result(
    config_type_hash: Hash,
    config: &CountingConfig,
    config_data: &[u8],
) -> Result<(), Error> {
    let data = load_cell_data(0, Source::GroupOutput).map_err(|_| Error::ResultInvalid)?;
    let result = ResultData::decode(&data).map_err(|_| Error::ResultInvalid)?;
    if result.proposal_config_data_hash != blake2b_256(config_data) {
        return Err(Error::ResultMismatch);
    }
    let (proposal_index, proposal, proposal_data) = load_proposal_input(&result, config)?;
    if proposal.config_type_hash != config_type_hash
        || result.requested_amount != proposal.requested_amount
        || result.receiver_lock_hash != proposal.receiver_lock_hash
        || load_cell_capacity(0, Source::GroupOutput).map_err(|_| Error::ResultLockInvalid)?
            != load_cell_capacity(proposal_index, Source::Input)
                .map_err(|_| Error::ResultLockInvalid)?
    {
        return Err(Error::ResultMismatch);
    }
    if result.outcome == ProposalOutcome::Vetoed {
        return validate_veto(config);
    }
    if proposal.phase != ProposalPhase::Finalized
        || result.yes != proposal.yes_amount
        || result.final_state_hash != blake2b_256(&proposal_data)
        || result.veto_reason_hash != [0; 32]
    {
        return Err(Error::ResultMismatch);
    }
    let passes = config.passes(result.yes, result.no, proposal.requested_amount);
    if (result.outcome == ProposalOutcome::Passed) != passes {
        return Err(Error::ResultMismatch);
    }
    if result.outcome == ProposalOutcome::Passed
        && load_cell_lock_hash(0, Source::GroupOutput).map_err(|_| Error::ResultLockInvalid)?
            != proposal.proposer_lock_hash
    {
        return Err(Error::ResultLockInvalid);
    }
    Ok(())
}

fn load_proposal_input(
    result: &ResultData,
    config: &CountingConfig,
) -> Result<(usize, ProposalData, alloc::vec::Vec<u8>), Error> {
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
        if type_script.code_hash().as_slice() != config.proposal_code_hash
            || type_script.hash_type().as_slice()[0] != config.proposal_hash_type
            || proposal.replace((index, parsed, data)).is_some()
        {
            return Err(Error::ProposalPolicyMismatch);
        }
    }
    proposal.ok_or(Error::ProposalNotFound)
}

fn validate_veto(config: &CountingConfig) -> Result<(), Error> {
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

fn consume_result(config: &CountingConfig) -> Result<(), Error> {
    let data = load_cell_data(0, Source::GroupInput).map_err(|_| Error::ResultInvalid)?;
    let result = ResultData::decode(&data).map_err(|_| Error::ResultInvalid)?;
    if result.outcome == ProposalOutcome::Vetoed {
        return Err(Error::VetoedResultImmutable);
    }
    if result.outcome != ProposalOutcome::Passed {
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
    if action.as_ref() == [TREASURY_ACTION_PAYOUT] {
        Ok(())
    } else {
        Err(Error::InvalidPayout)
    }
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
