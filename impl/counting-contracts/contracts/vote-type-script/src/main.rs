#![no_std]
#![no_main]

ckb_std::entry!(program_entry);
ckb_std::default_alloc!(16384, 1258306, 64);

use ckb_std::{
    ckb_constants::Source,
    ckb_types::prelude::{Entity, Unpack},
    high_level::{
        QueryIter, load_cell_capacity, load_cell_data, load_cell_lock_hash, load_cell_type,
        load_cell_type_hash, load_header, load_input_out_point, load_script, load_transaction,
    },
};
use counting_common::{CountingConfig, OutPoint, ProposalData, ProposalPhase, VoteData};

#[repr(i8)]
enum Error {
    ArgsInvalid = 1,
    ProposalNotFound,
    ProposalNotOpen,
    VoterLockNotFound,
    VoteDataInvalid,
    DaoDepInvalid,
    DuplicateDaoDep,
    AmountMismatch,
    AmountBelowMinimum,
    CapacityOverflow,
    MultipleVoteOutputs,
    DaoSpentInVote,
    ConfigNotFound,
    ConfigInvalid,
    ContractIdentityMismatch,
    EventImmutable,
    DepGroupUnsupported,
    ProposalCreationHeaderMissing,
    DaoCreationHeaderMissing,
    DaoDepositTooNew,
}

pub fn program_entry() -> i8 {
    match run() {
        Ok(()) => 0,
        Err(error) => error as i8,
    }
}

fn run() -> Result<(), Error> {
    let script = load_script().map_err(|_| Error::ArgsInvalid)?;
    let args = script.args().raw_data();
    let proposal_type_hash: [u8; 32] = args.as_ref().try_into().map_err(|_| Error::ArgsInvalid)?;

    let input_count = QueryIter::new(load_cell_type_hash, Source::GroupInput).count();
    let output_count = QueryIter::new(load_cell_type_hash, Source::GroupOutput).count();
    if input_count != 0 {
        return Err(Error::EventImmutable);
    }
    if output_count != 1 {
        return Err(Error::MultipleVoteOutputs);
    }

    let (proposal_dep_index, proposal, config) = find_open_proposal(proposal_type_hash)?;
    if script.code_hash().as_slice() != config.vote_code_hash
        || script.hash_type().as_slice()[0] != config.vote_hash_type
    {
        return Err(Error::ContractIdentityMismatch);
    }
    let proposal_creation_block: u64 = load_header(proposal_dep_index, Source::CellDep)
        .map_err(|_| Error::ProposalCreationHeaderMissing)?
        .raw()
        .number()
        .unpack();
    let vote_lock_hash =
        load_cell_lock_hash(0, Source::GroupOutput).map_err(|_| Error::VoteDataInvalid)?;
    if !QueryIter::new(load_cell_lock_hash, Source::Input)
        .any(|input_lock_hash| input_lock_hash == vote_lock_hash)
    {
        return Err(Error::VoterLockNotFound);
    }

    let vote_data = load_cell_data(0, Source::GroupOutput).map_err(|_| Error::VoteDataInvalid)?;
    let vote = VoteData::decode(&vote_data).map_err(|_| Error::VoteDataInvalid)?;
    let transaction = load_transaction().map_err(|_| Error::VoteDataInvalid)?;
    let cell_deps = transaction.raw().cell_deps();
    let spent_out_points =
        QueryIter::new(load_input_out_point, Source::Input).collect::<alloc::vec::Vec<_>>();
    let mut previous_out_point = None;
    let mut total_capacity = 0u64;
    for dao_out_point in vote.dao_out_points {
        if previous_out_point.is_some_and(|previous| dao_out_point <= previous) {
            return Err(Error::DuplicateDaoDep);
        }
        previous_out_point = Some(dao_out_point);
        let dep_index = cell_deps
            .clone()
            .into_iter()
            .position(|cell_dep| unpack_out_point(&cell_dep.out_point()) == dao_out_point)
            .ok_or(Error::DaoDepInvalid)?;
        if cell_deps
            .clone()
            .into_iter()
            .take(dep_index + 1)
            .any(|cell_dep| cell_dep.dep_type().as_slice()[0] != 0)
        {
            return Err(Error::DepGroupUnsupported);
        }
        let dep_out_point = cell_deps.get(dep_index).unwrap().out_point();
        if spent_out_points.contains(&dep_out_point) {
            return Err(Error::DaoSpentInVote);
        }

        let dep_lock_hash =
            load_cell_lock_hash(dep_index, Source::CellDep).map_err(|_| Error::DaoDepInvalid)?;
        if dep_lock_hash != vote_lock_hash {
            return Err(Error::DaoDepInvalid);
        }
        let dep_type = load_cell_type(dep_index, Source::CellDep)
            .map_err(|_| Error::DaoDepInvalid)?
            .ok_or(Error::DaoDepInvalid)?;
        if dep_type.code_hash().as_slice() != config.dao_code_hash
            || dep_type.hash_type().as_slice()[0] != config.dao_hash_type
            || !dep_type.args().raw_data().is_empty()
        {
            return Err(Error::DaoDepInvalid);
        }
        let dep_data =
            load_cell_data(dep_index, Source::CellDep).map_err(|_| Error::DaoDepInvalid)?;
        if dep_data.as_slice() != [0u8; 8] {
            return Err(Error::DaoDepInvalid);
        }
        let dao_creation_block: u64 = load_header(dep_index, Source::CellDep)
            .map_err(|_| Error::DaoCreationHeaderMissing)?
            .raw()
            .number()
            .unpack();
        if dao_creation_block >= proposal_creation_block {
            return Err(Error::DaoDepositTooNew);
        }

        total_capacity = total_capacity
            .checked_add(
                load_cell_capacity(dep_index, Source::CellDep).map_err(|_| Error::DaoDepInvalid)?,
            )
            .ok_or(Error::CapacityOverflow)?;
    }

    if total_capacity != vote.amount {
        return Err(Error::AmountMismatch);
    }
    if total_capacity < proposal.minimum_vote_capacity {
        return Err(Error::AmountBelowMinimum);
    }
    Ok(())
}

fn unpack_out_point(out_point: &ckb_std::ckb_types::packed::OutPoint) -> OutPoint {
    OutPoint {
        tx_hash: out_point.tx_hash().as_slice().try_into().unwrap(),
        index: out_point.index().unpack(),
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

fn find_open_proposal(
    proposal_type_hash: [u8; 32],
) -> Result<(usize, ProposalData, CountingConfig), Error> {
    for (index, type_hash) in QueryIter::new(load_cell_type_hash, Source::CellDep).enumerate() {
        if type_hash == Some(proposal_type_hash) {
            let type_script = load_cell_type(index, Source::CellDep)
                .map_err(|_| Error::ProposalNotOpen)?
                .ok_or(Error::ProposalNotOpen)?;
            let data =
                load_cell_data(index, Source::CellDep).map_err(|_| Error::ProposalNotOpen)?;
            let proposal = ProposalData::decode(&data).map_err(|_| Error::ProposalNotOpen)?;
            if proposal.phase != ProposalPhase::Open {
                return Err(Error::ProposalNotOpen);
            }
            let config = load_config(proposal.config_type_hash)?;
            if type_script.code_hash().as_slice() != config.proposal_code_hash
                || type_script.hash_type().as_slice()[0] != config.proposal_hash_type
            {
                return Err(Error::ContractIdentityMismatch);
            }
            return Ok((index, proposal, config));
        }
    }
    Err(Error::ProposalNotFound)
}
