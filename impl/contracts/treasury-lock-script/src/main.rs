#![no_std]
#![no_main]

ckb_std::entry!(program_entry);
ckb_std::default_alloc!(16384, 1258306, 64);

use ckb_std::{
    ckb_constants::Source,
    ckb_types::prelude::Entity,
    high_level::{
        QueryIter, load_cell_capacity, load_cell_data, load_cell_lock, load_cell_lock_hash,
        load_cell_type, load_cell_type_hash, load_input_since, load_script, load_witness_args,
    },
    since::{LockValue, Since},
};
use treasury_common::{ResultData, TreasuryConfig};

const ACTION_BURN: u8 = 0;
const ACTION_PAYOUT: u8 = 1;

#[repr(i8)]
enum Error {
    ArgsInvalid = 1,
    WitnessInvalid,
    ConfigNotFound,
    ConfigInvalid,
    CapacityOverflow,
    BurnNotMature,
    BurnOutputInvalid,
    ResultNotFound,
    ResultInvalid,
    ReceiverOutputInvalid,
    PayoutChangeCountInvalid,
    PayoutChangeCapacityInvalid,
    PayoutChangeDataInvalid,
    PayoutChangeTypeInvalid,
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
    let config = load_config(config_type_hash)?;
    let witness = load_witness_args(0, Source::GroupInput).map_err(|_| Error::WitnessInvalid)?;
    let lock = witness
        .lock()
        .to_opt()
        .ok_or(Error::WitnessInvalid)?
        .raw_data();
    let action = *lock.first().ok_or(Error::WitnessInvalid)?;
    match action {
        ACTION_BURN if lock.len() == 33 => {
            let recipient_lock_hash = lock[1..].try_into().map_err(|_| Error::WitnessInvalid)?;
            burn(config, recipient_lock_hash)
        }
        ACTION_PAYOUT if lock.len() == 1 => payout(config),
        _ => Err(Error::WitnessInvalid),
    }
}

fn burn(config: TreasuryConfig, recipient_lock_hash: [u8; 32]) -> Result<(), Error> {
    if !treasury_output_indices()?.is_empty() {
        return Err(Error::BurnOutputInvalid);
    }
    if QueryIter::new(load_cell_type_hash, Source::Input)
        .any(|type_hash| type_hash == Some(config.result_type_hash))
    {
        return Err(Error::ResultInvalid);
    }
    let relative_blocks = QueryIter::new(load_input_since, Source::GroupInput)
        .map(|raw_since| {
            let since = Since::new(raw_since);
            if !since.flags_is_valid() || !since.is_relative() {
                return None;
            }
            match since.extract_lock_value() {
                Some(LockValue::BlockNumber(blocks)) => Some(blocks),
                _ => None,
            }
        })
        .collect::<Option<alloc::vec::Vec<_>>>()
        .ok_or(Error::BurnNotMature)?
        .into_iter()
        .min()
        .ok_or(Error::BurnNotMature)?;
    let incentive = config
        .burn_incentive(relative_blocks)
        .ok_or(Error::BurnNotMature)?;
    let treasury_capacity = group_input_capacity()?;
    let burned_capacity = treasury_capacity
        .checked_sub(incentive)
        .ok_or(Error::BurnOutputInvalid)?;

    let zero_output = find_unique_output(config.zero_lock_hash, Error::BurnOutputInvalid)?;
    let incentive_output = find_unique_output(recipient_lock_hash, Error::BurnOutputInvalid)?;
    if zero_output == incentive_output
        || load_cell_capacity(zero_output, Source::Output).map_err(|_| Error::BurnOutputInvalid)?
            != burned_capacity
        || load_cell_capacity(incentive_output, Source::Output)
            .map_err(|_| Error::BurnOutputInvalid)?
            != incentive
        || load_cell_type(zero_output, Source::Output)
            .map_err(|_| Error::BurnOutputInvalid)?
            .is_some()
        || !load_cell_data(zero_output, Source::Output)
            .map_err(|_| Error::BurnOutputInvalid)?
            .is_empty()
    {
        return Err(Error::BurnOutputInvalid);
    }
    Ok(())
}

fn payout(config: TreasuryConfig) -> Result<(), Error> {
    let mut result = None;
    for (index, type_hash) in QueryIter::new(load_cell_type_hash, Source::Input).enumerate() {
        if type_hash == Some(config.result_type_hash) {
            let data = load_cell_data(index, Source::Input).map_err(|_| Error::ResultInvalid)?;
            let parsed = ResultData::decode(&data).map_err(|_| Error::ResultInvalid)?;
            if !parsed.is_passed() || result.replace(parsed).is_some() {
                return Err(Error::ResultInvalid);
            }
        }
    }
    let result = result.ok_or(Error::ResultNotFound)?;
    let treasury_capacity = group_input_capacity()?;
    let change = treasury_capacity
        .checked_sub(result.requested_amount)
        .ok_or(Error::PayoutChangeCapacityInvalid)?;

    let receiver_output =
        find_unique_output(result.receiver_lock_hash, Error::ReceiverOutputInvalid)?;
    if load_cell_capacity(receiver_output, Source::Output)
        .map_err(|_| Error::ReceiverOutputInvalid)?
        != result.requested_amount
        || load_cell_type(receiver_output, Source::Output)
            .map_err(|_| Error::ReceiverOutputInvalid)?
            .is_some()
        || !load_cell_data(receiver_output, Source::Output)
            .map_err(|_| Error::ReceiverOutputInvalid)?
            .is_empty()
    {
        return Err(Error::ReceiverOutputInvalid);
    }

    let treasury_outputs = treasury_output_indices()?;
    if change == 0 {
        if !treasury_outputs.is_empty() {
            return Err(Error::PayoutChangeCountInvalid);
        }
    } else {
        if treasury_outputs.len() != 1 {
            return Err(Error::PayoutChangeCountInvalid);
        }
        let change_index = treasury_outputs[0];
        if change_index == receiver_output
            || load_cell_capacity(change_index, Source::Output)
                .map_err(|_| Error::PayoutChangeCapacityInvalid)?
                != change
        {
            return Err(Error::PayoutChangeCapacityInvalid);
        }
        if !load_cell_data(change_index, Source::Output)
            .map_err(|_| Error::PayoutChangeDataInvalid)?
            .is_empty()
        {
            return Err(Error::PayoutChangeDataInvalid);
        }
        if load_cell_type(change_index, Source::Output)
            .map_err(|_| Error::PayoutChangeTypeInvalid)?
            .is_some()
        {
            return Err(Error::PayoutChangeTypeInvalid);
        }
    }
    Ok(())
}

fn treasury_output_indices() -> Result<alloc::vec::Vec<usize>, Error> {
    let current_script = load_script().map_err(|_| Error::ArgsInvalid)?;
    Ok(QueryIter::new(load_cell_lock, Source::Output)
        .enumerate()
        .filter_map(|(index, lock)| (lock.as_slice() == current_script.as_slice()).then_some(index))
        .collect())
}

fn group_input_capacity() -> Result<u64, Error> {
    QueryIter::new(load_cell_capacity, Source::GroupInput).try_fold(0u64, |sum, capacity| {
        sum.checked_add(capacity).ok_or(Error::CapacityOverflow)
    })
}

fn find_unique_output(expected_lock_hash: [u8; 32], error: Error) -> Result<usize, Error> {
    let mut found = None;
    for (index, lock_hash) in QueryIter::new(load_cell_lock_hash, Source::Output).enumerate() {
        if lock_hash == expected_lock_hash && found.replace(index).is_some() {
            return Err(error);
        }
    }
    found.ok_or(error)
}

fn load_config(config_type_hash: [u8; 32]) -> Result<TreasuryConfig, Error> {
    for (index, type_hash) in QueryIter::new(load_cell_type_hash, Source::CellDep).enumerate() {
        if type_hash == Some(config_type_hash) {
            let data = load_cell_data(index, Source::CellDep).map_err(|_| Error::ConfigInvalid)?;
            return TreasuryConfig::decode(&data).map_err(|_| Error::ConfigInvalid);
        }
    }
    Err(Error::ConfigNotFound)
}
