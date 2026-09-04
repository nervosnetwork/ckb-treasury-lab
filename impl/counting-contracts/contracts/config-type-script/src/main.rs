#![no_std]
#![no_main]

ckb_std::entry!(program_entry);
ckb_std::default_alloc!(16384, 1258306, 64);

use ckb_std::{
    ckb_constants::Source,
    high_level::{
        QueryIter, load_cell_capacity, load_cell_data, load_cell_lock_hash, load_cell_type_hash,
    },
    type_id::check_type_id,
};

#[repr(i8)]
enum Error {
    TypeIdInvalid = 1,
    InvalidCellCount,
    MutationForbidden,
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
        (0, 1) => Ok(()),
        (1, 1) => immutable_transfer(),
        (0..=1, 0..=1) => Err(Error::MutationForbidden),
        _ => Err(Error::InvalidCellCount),
    }
}

fn immutable_transfer() -> Result<(), Error> {
    if load_cell_data(0, Source::GroupInput).map_err(|_| Error::MutationForbidden)?
        != load_cell_data(0, Source::GroupOutput).map_err(|_| Error::MutationForbidden)?
        || load_cell_capacity(0, Source::GroupInput).map_err(|_| Error::MutationForbidden)?
            != load_cell_capacity(0, Source::GroupOutput).map_err(|_| Error::MutationForbidden)?
        || load_cell_lock_hash(0, Source::GroupInput).map_err(|_| Error::MutationForbidden)?
            != load_cell_lock_hash(0, Source::GroupOutput).map_err(|_| Error::MutationForbidden)?
    {
        return Err(Error::MutationForbidden);
    }
    Ok(())
}
