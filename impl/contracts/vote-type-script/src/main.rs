#![no_std]
#![no_main]

ckb_std::entry!(program_entry);
ckb_std::default_alloc!(16384, 1258306, 64);

use ckb_gen_types::packed::Vote;
use ckb_hash::new_blake2b;
use ckb_std::{
    ckb_constants::Source,
    high_level::{QueryIter, load_cell_capacity, load_cell_lock, load_cell_type, load_script},
};
use molecule::prelude::{Entity, Reader};

// Nervos DAO genesis type script (RFC 0024).
const DAO_CODE_HASH: [u8; 32] = [
    0x82, 0xd7, 0x6d, 0x1b, 0x75, 0xfe, 0x2f, 0xd9, 0xa2, 0x7d, 0xfb, 0xaa, 0x65, 0xa0, 0x39, 0x22,
    0x1a, 0x38, 0x0d, 0x76, 0xc9, 0x26, 0xf3, 0x78, 0xd3, 0xf8, 0x1c, 0xf3, 0xe7, 0xe1, 0x3f, 0x2e,
];
const DAO_HASH_TYPE: u8 = 0x01;

#[repr(i8)]
enum Error {
    ArgsInvalid = 1,
    ProposalNotFound,
    VoterLockNotFound,
    VoteDataInvalid,
    DaoDepInvalid,
    AmountMismatch,
    MultipleVoteOutputs,
}

pub fn program_entry() -> i8 {
    match run() {
        Ok(()) => 0,
        Err(e) => e as i8,
    }
}

fn blake160(data: &[u8]) -> [u8; 20] {
    let mut hash = [0u8; 32];
    let mut b = new_blake2b();
    b.update(data);
    b.finalize(&mut hash);
    let mut result = [0u8; 20];
    result.copy_from_slice(&hash[..20]);
    result
}

fn run() -> Result<(), Error> {
    let script = load_script().map_err(|_| Error::ArgsInvalid)?;
    let args = script.args().raw_data().to_vec();
    if args.len() != 20 {
        return Err(Error::ArgsInvalid);
    }
    let expected_blake160: [u8; 20] = args[..20].try_into().unwrap();

    // Determine action: zero group outputs = consumption (recycle CKB, always allow);
    // exactly one = creation (cast a vote, validate below); more than one = invalid.
    let group_output_count = QueryIter::new(load_cell_lock, Source::GroupOutput).count();
    if group_output_count == 0 {
        return Ok(());
    }
    // 3. Ensure that exactly one cell in the output contains this type script.
    if group_output_count > 1 {
        return Err(Error::MultipleVoteOutputs);
    }

    // 1. Find the proposal cell in cell_deps. Its type script's blake160 must match args[0..20].
    let proposal_found = QueryIter::new(load_cell_type, Source::CellDep).any(|maybe_type_script| {
        maybe_type_script
            .as_ref()
            .map(|type_script| blake160(type_script.as_slice()) == expected_blake160)
            .unwrap_or(false)
    });
    if !proposal_found {
        return Err(Error::ProposalNotFound);
    }

    let vote_lock = load_cell_lock(0, Source::GroupOutput).map_err(|_| Error::ArgsInvalid)?;

    let vote_data = ckb_std::high_level::load_cell_data(0, Source::GroupOutput)
        .map_err(|_| Error::VoteDataInvalid)?;

    let vote = Vote::from_slice(&vote_data).map_err(|_| Error::VoteDataInvalid)?;
    let vote_value = vote.as_reader().vote().as_slice()[0];
    if vote_value != 0 && vote_value != 1 {
        return Err(Error::VoteDataInvalid);
    }

    // 2. Find a lock on an input that matches the vote output lock; this proves DAO ownership.
    let voter_lock_found = QueryIter::new(load_cell_lock, Source::Input)
        .any(|input_lock| input_lock.as_slice() == vote_lock.as_slice());
    if !voter_lock_found {
        return Err(Error::VoterLockNotFound);
    }

    // 4. Each cell_dep index listed in dao_index must be a DAO deposit owned by the voter.
    //    Sum their capacities and verify the total equals the amount field in the vote data.
    let expected_amount = u64::from_le_bytes(
        vote.as_reader()
            .amount()
            .as_slice()
            .try_into()
            .map_err(|_| Error::VoteDataInvalid)?,
    );
    let mut dao_dep_count: usize = 0;
    let mut total_capacity: u64 = 0;

    for dao_idx_reader in vote.as_reader().dao_index().iter() {
        dao_dep_count += 1;
        let dep_idx = u16::from_le_bytes(dao_idx_reader.as_slice().try_into().unwrap()) as usize;

        let dep_lock =
            load_cell_lock(dep_idx, Source::CellDep).map_err(|_| Error::DaoDepInvalid)?;
        let dep_type = load_cell_type(dep_idx, Source::CellDep)
            .map_err(|_| Error::DaoDepInvalid)?
            .ok_or(Error::DaoDepInvalid)?;

        if dep_lock.as_slice() != vote_lock.as_slice() {
            return Err(Error::DaoDepInvalid);
        }
        if dep_type.code_hash().as_slice() != DAO_CODE_HASH {
            return Err(Error::DaoDepInvalid);
        }
        if dep_type.hash_type().as_slice()[0] != DAO_HASH_TYPE {
            return Err(Error::DaoDepInvalid);
        }
        if !dep_type.args().raw_data().is_empty() {
            return Err(Error::DaoDepInvalid);
        }

        let cap = load_cell_capacity(dep_idx, Source::CellDep).map_err(|_| Error::DaoDepInvalid)?;
        total_capacity = total_capacity.saturating_add(cap);
    }
    if dao_dep_count == 0 || total_capacity == 0 {
        return Err(Error::DaoDepInvalid);
    }

    if total_capacity != expected_amount {
        return Err(Error::AmountMismatch);
    }

    Ok(())
}
