#![no_std]
#![no_main]

ckb_std::entry!(program_entry);
ckb_std::default_alloc!(16384, 1258306, 64);

use ckb_std::{
    ckb_constants::Source,
    ckb_types::prelude::{Entity, Unpack},
    high_level::{
        QueryIter, load_cell_data, load_cell_lock_hash, load_cell_type, load_cell_type_hash,
        load_header, load_script,
    },
};
use counting_common::{
    CountingCellData, CountingConfig, ProposalData, ProposalOutcome, ProposalPhase, ResultData,
    VoteData,
};

#[repr(i8)]
enum Error {
    ArgsInvalid = 1,
    CountingDataInvalid,
    ProposalNotFound,
    ProposalPhaseInvalid,
    ConfigNotFound,
    ConfigInvalid,
    ContractIdentityMismatch,
    VoteProposalMismatch,
    VoteDataInvalid,
    VoteDirectionMismatch,
    VoteOutsideRange,
    VoterLocksNotUnique,
    AmountOverflow,
    AmountMismatch,
    VoteCountMismatch,
    TooManyVotes,
    VoteHeaderMissing,
    VoteOutsideWindow,
    CreatorMissing,
    CountingLockInvalid,
    ProposalTransitionInvalid,
    ResultInvalid,
    InvalidTransition,
}

pub fn program_entry() -> i8 {
    match run() {
        Ok(()) => 0,
        Err(error) => error as i8,
    }
}

fn run() -> Result<(), Error> {
    let inputs = QueryIter::new(load_cell_type_hash, Source::GroupInput).count();
    let outputs = QueryIter::new(load_cell_type_hash, Source::GroupOutput).count();
    match (inputs, outputs) {
        (0, 1) => create(),
        (inputs, 0) if inputs > 0 => consume(),
        _ => Err(Error::InvalidTransition),
    }
}

fn create() -> Result<(), Error> {
    let script = load_script().map_err(|_| Error::ArgsInvalid)?;
    let proposal_id: [u8; 32] = script
        .args()
        .raw_data()
        .as_ref()
        .try_into()
        .map_err(|_| Error::ArgsInvalid)?;
    let (proposal, config) = load_proposal_and_config(proposal_id)?;
    if script.code_hash().as_slice() != config.counting_code_hash
        || script.hash_type().as_slice()[0] != config.counting_hash_type
    {
        return Err(Error::ContractIdentityMismatch);
    }

    let data = load_cell_data(0, Source::GroupOutput).map_err(|_| Error::CountingDataInvalid)?;
    let counting = CountingCellData::decode(&data).map_err(|_| Error::CountingDataInvalid)?;
    let expected_phase = if counting.direction == 1 {
        ProposalPhase::Closed
    } else {
        ProposalPhase::Finalized
    };
    if proposal.phase != expected_phase {
        return Err(Error::ProposalPhaseInvalid);
    }
    let counting_lock =
        load_cell_lock_hash(0, Source::GroupOutput).map_err(|_| Error::CountingLockInvalid)?;
    if counting.direction == 1 {
        if counting_lock != proposal.proposer_lock_hash {
            return Err(Error::CountingLockInvalid);
        }
        require_input_lock(proposal.proposer_lock_hash)?;
    } else {
        require_input_lock(counting_lock)?;
    }
    if counting.vote_count > config.max_votes_per_counting_cell {
        return Err(Error::TooManyVotes);
    }

    let mut previous_lock_hash = None;
    let mut amount = 0u128;
    let mut vote_count = 0u32;
    for (index, type_script) in QueryIter::new(load_cell_type, Source::CellDep).enumerate() {
        let Some(type_script) = type_script else {
            continue;
        };
        if type_script.code_hash().as_slice() != config.vote_code_hash
            || type_script.hash_type().as_slice()[0] != config.vote_hash_type
        {
            continue;
        }
        if type_script.args().raw_data().as_ref() != proposal_id {
            return Err(Error::VoteProposalMismatch);
        }
        let lock_hash =
            load_cell_lock_hash(index, Source::CellDep).map_err(|_| Error::VoteDataInvalid)?;
        if !counting.contains_lock_hash(&lock_hash) {
            return Err(Error::VoteOutsideRange);
        }
        if previous_lock_hash.is_some_and(|previous| previous >= lock_hash) {
            return Err(Error::VoterLocksNotUnique);
        }
        previous_lock_hash = Some(lock_hash);
        let vote_block: u64 = load_header(index, Source::CellDep)
            .map_err(|_| Error::VoteHeaderMissing)?
            .raw()
            .number()
            .unpack();
        if vote_block < proposal.start_block || vote_block > proposal.end_block {
            return Err(Error::VoteOutsideWindow);
        }

        let vote_data =
            load_cell_data(index, Source::CellDep).map_err(|_| Error::VoteDataInvalid)?;
        let vote = VoteData::decode(&vote_data).map_err(|_| Error::VoteDataInvalid)?;
        if vote.direction != counting.direction {
            return Err(Error::VoteDirectionMismatch);
        }
        amount = amount
            .checked_add(vote.amount as u128)
            .ok_or(Error::AmountOverflow)?;
        vote_count = vote_count.checked_add(1).ok_or(Error::AmountOverflow)?;
    }

    if amount != counting.amount {
        return Err(Error::AmountMismatch);
    }
    if vote_count != counting.vote_count {
        return Err(Error::VoteCountMismatch);
    }
    Ok(())
}

fn consume() -> Result<(), Error> {
    let script = load_script().map_err(|_| Error::ArgsInvalid)?;
    let proposal_id: [u8; 32] = script
        .args()
        .raw_data()
        .as_ref()
        .try_into()
        .map_err(|_| Error::ArgsInvalid)?;
    let counting_data =
        load_cell_data(0, Source::GroupInput).map_err(|_| Error::CountingDataInvalid)?;
    let counting =
        CountingCellData::decode(&counting_data).map_err(|_| Error::CountingDataInvalid)?;
    let (proposal, proposal_type) = load_proposal(proposal_id, Source::Input)?;
    let config = load_config(proposal.config_type_hash)?;
    if script.code_hash().as_slice() != config.counting_code_hash
        || script.hash_type().as_slice()[0] != config.counting_hash_type
        || proposal_type.code_hash().as_slice() != config.proposal_code_hash
        || proposal_type.hash_type().as_slice()[0] != config.proposal_hash_type
    {
        return Err(Error::ContractIdentityMismatch);
    }

    if counting.direction == 1 {
        let (output, _) = load_proposal(proposal_id, Source::Output)?;
        if proposal.phase != ProposalPhase::Closed || output.phase != ProposalPhase::Finalized {
            return Err(Error::ProposalTransitionInvalid);
        }
        return Ok(());
    }

    if proposal.phase != ProposalPhase::Finalized
        || QueryIter::new(load_cell_type_hash, Source::Output)
            .any(|type_hash| type_hash == Some(proposal_id))
    {
        return Err(Error::ProposalTransitionInvalid);
    }
    let result = load_result(&config)?;
    if result.proposal_id != proposal_id || result.outcome != ProposalOutcome::RejectedByVote {
        return Err(Error::ResultInvalid);
    }
    Ok(())
}

fn load_proposal_and_config(
    proposal_id: [u8; 32],
) -> Result<(ProposalData, CountingConfig), Error> {
    let (proposal, proposal_type) = load_proposal(proposal_id, Source::CellDep)?;
    let config = load_config(proposal.config_type_hash)?;
    if proposal_type.code_hash().as_slice() != config.proposal_code_hash
        || proposal_type.hash_type().as_slice()[0] != config.proposal_hash_type
    {
        return Err(Error::ContractIdentityMismatch);
    }
    Ok((proposal, config))
}

fn load_proposal(
    proposal_id: [u8; 32],
    source: Source,
) -> Result<(ProposalData, ckb_std::ckb_types::packed::Script), Error> {
    let mut proposal = None;
    for (index, type_hash) in QueryIter::new(load_cell_type_hash, source).enumerate() {
        if type_hash != Some(proposal_id) {
            continue;
        }
        let type_script = load_cell_type(index, source)
            .map_err(|_| Error::ProposalNotFound)?
            .ok_or(Error::ProposalNotFound)?;
        let data = load_cell_data(index, source).map_err(|_| Error::ProposalNotFound)?;
        let parsed = ProposalData::decode(&data).map_err(|_| Error::ProposalNotFound)?;
        if proposal.replace((parsed, type_script)).is_some() {
            return Err(Error::ProposalNotFound);
        }
    }
    proposal.ok_or(Error::ProposalNotFound)
}

fn load_result(config: &CountingConfig) -> Result<ResultData, Error> {
    let mut result = None;
    for (index, type_hash) in QueryIter::new(load_cell_type_hash, Source::Output).enumerate() {
        if type_hash != Some(config.policy_type_hash) {
            continue;
        }
        let data = load_cell_data(index, Source::Output).map_err(|_| Error::ResultInvalid)?;
        let parsed = ResultData::decode(&data).map_err(|_| Error::ResultInvalid)?;
        if result.replace(parsed).is_some() {
            return Err(Error::ResultInvalid);
        }
    }
    result.ok_or(Error::ResultInvalid)
}

fn require_input_lock(lock_hash: [u8; 32]) -> Result<(), Error> {
    if QueryIter::new(load_cell_lock_hash, Source::Input).any(|input| input == lock_hash) {
        Ok(())
    } else {
        Err(Error::CreatorMissing)
    }
}

fn load_config(config_type_hash: [u8; 32]) -> Result<CountingConfig, Error> {
    for (index, type_hash) in QueryIter::new(load_cell_type_hash, Source::CellDep).enumerate() {
        if type_hash == Some(config_type_hash) {
            let data = load_cell_data(index, Source::CellDep).map_err(|_| Error::ConfigInvalid)?;
            return CountingConfig::decode(&data).map_err(|_| Error::ConfigInvalid);
        }
    }
    Err(Error::ConfigNotFound)
}
