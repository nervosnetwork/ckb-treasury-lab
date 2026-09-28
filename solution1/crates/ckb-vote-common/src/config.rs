//! Loading of the voting system config cell.
//!
//! Every script of the voting system carries `blake160(config type script)` in
//! the leading bytes of its `args`, and finds the matching cell in the
//! transaction's cell dependencies. Because the pointer is a hash, a
//! transaction cannot substitute a config cell of its own.

use ckb_std::{
    ckb_constants::Source,
    ckb_types::{
        packed::{Byte, Byte32, Script, Uint64},
        prelude::Entity,
    },
    high_level::{QueryIter, load_cell_data, load_cell_type},
};
use ckb_vote_types::molecules::types::VotingConfig;

use crate::{constants, error::Error, hash};

/// The decoded content of the voting system config cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// When set to `1`, every voting script fails.
    pub emergent_halt: u8,
    /// Code hash of the vote type script.
    pub vote_code_hash: [u8; 32],
    /// Hash type of the vote type script.
    pub vote_hash_type: u8,
    /// Code hash of the counting type script.
    pub counting_code_hash: [u8; 32],
    /// Hash type of the counting type script.
    pub counting_hash_type: u8,
    /// Code hash of the `always success` lock script.
    pub always_success_code_hash: [u8; 32],
    /// Hash type of the `always success` lock script.
    pub always_success_hash_type: u8,
    /// The minimum amount of "YES" votes a proposal needs to be finalized.
    pub yes_threshold: u64,
    /// The minimum capacity (in shannons) of the proposal cell bond.
    pub minimal_proposal_capacity: u64,
    /// The number of blocks a proposal has to wait after its creation before it
    /// can be finalized. A block count, compared with the relative `since` of
    /// the proposal input.
    pub vote_duration: u64,
    /// The number of blocks after the proposal cell in which a vote is valid.
    /// A block count, compared with the block numbers that created the cells.
    pub vote_window: u64,
    /// The number of blocks a finalized proposal has to wait before it can be
    /// passed. A block count, compared with the relative `since` of the
    /// proposal input.
    pub challenge_time: u64,
    /// The lock script hash allowed to veto a finalized proposal.
    pub veto_lock_script_hash: [u8; 32],
    /// The minimum `vote_amount` (in shannons) a vote cell may declare.
    pub minimal_vote_amount: u64,
}

impl Config {
    /// Decodes a config cell payload.
    pub fn from_data(data: &[u8]) -> Result<Config, Error> {
        let packed = VotingConfig::from_slice(data).map_err(|_| Error::ConfigCellInvalid)?;
        Ok(Config {
            emergent_halt: byte_of(packed.emergent_halt()),
            vote_code_hash: byte32_of(packed.vote_code_hash()),
            vote_hash_type: byte_of(packed.vote_hash_type()),
            counting_code_hash: byte32_of(packed.counting_code_hash()),
            counting_hash_type: byte_of(packed.counting_hash_type()),
            always_success_code_hash: byte32_of(packed.always_success_code_hash()),
            always_success_hash_type: byte_of(packed.always_success_hash_type()),
            yes_threshold: u64_of(packed.yes_threshold()),
            minimal_proposal_capacity: u64_of(packed.minimal_proposal_capacity()),
            vote_duration: u64_of(packed.vote_duration()),
            vote_window: u64_of(packed.vote_window()),
            challenge_time: u64_of(packed.challenge_time()),
            veto_lock_script_hash: byte32_of(packed.veto_lock_script_hash()),
            minimal_vote_amount: u64_of(packed.minimal_vote_amount()),
        })
    }

    /// Loads the config cell whose type script id is `config_id`.
    ///
    /// The system is halted as soon as `emergent_halt` is set, so a halted
    /// config is reported as an error and never handed out to a script.
    pub fn load(config_id: &[u8; constants::CONFIG_ID_LEN]) -> Result<Config, Error> {
        let mut found: Option<Config> = None;
        for (index, type_script) in QueryIter::new(load_cell_type, Source::CellDep).enumerate() {
            let Some(type_script) = type_script else {
                continue;
            };
            if &hash::script_id(&type_script) != config_id {
                continue;
            }
            if found.is_some() {
                return Err(Error::ConfigCellAmbiguous);
            }
            let data =
                load_cell_data(index, Source::CellDep).map_err(|_| Error::ConfigCellNotFound)?;
            found = Some(Config::from_data(&data)?);
        }

        let config = found.ok_or(Error::ConfigCellNotFound)?;
        config.validate()?;
        if config.emergent_halt != 0 {
            return Err(Error::EmergentHalt);
        }
        Ok(config)
    }

    /// Rejects values that can never be meaningful.
    pub fn validate(&self) -> Result<(), Error> {
        if self.emergent_halt > 1
            || !constants::is_valid_script_hash_type(self.vote_hash_type)
            || !constants::is_valid_script_hash_type(self.counting_hash_type)
            || !constants::is_valid_script_hash_type(self.always_success_hash_type)
        {
            return Err(Error::ConfigCellInvalid);
        }
        Ok(())
    }

    /// Tells whether `type_script` is the vote type script recorded in the config.
    pub fn is_vote_script(&self, type_script: &Script) -> bool {
        script_matches(type_script, &self.vote_code_hash, self.vote_hash_type)
    }

    /// Tells whether `type_script` is the counting type script recorded in the config.
    pub fn is_counting_script(&self, type_script: &Script) -> bool {
        script_matches(
            type_script,
            &self.counting_code_hash,
            self.counting_hash_type,
        )
    }

    /// Tells whether `lock_script` is the `always success` lock script recorded
    /// in the config. A finalized proposal cell has to be spendable by anyone,
    /// otherwise it could not be challenged.
    pub fn is_always_success_lock(&self, lock_script: &Script) -> bool {
        script_matches(
            lock_script,
            &self.always_success_code_hash,
            self.always_success_hash_type,
        )
    }
}

/// Compares the identity of a script against a `(code_hash, hash_type)` pair.
pub fn script_matches(type_script: &Script, code_hash: &[u8; 32], hash_type: u8) -> bool {
    type_script.code_hash().as_slice() == code_hash
        && type_script.hash_type().as_slice()[0] == hash_type
}

/// Loads the config cell and fails when the system has been halted.
///
/// This is the check every script has to perform, including on the paths that
/// do not read any other config field.
pub fn ensure_running(config_id: &[u8; constants::CONFIG_ID_LEN]) -> Result<(), Error> {
    Config::load(config_id).map(|_| ())
}

fn byte_of(value: Byte) -> u8 {
    value.as_slice()[0]
}

fn byte32_of(value: Byte32) -> [u8; 32] {
    let mut buffer = [0u8; 32];
    buffer.copy_from_slice(value.as_slice());
    buffer
}

/// Reads a little endian `Uint64` field.
pub fn u64_of(value: Uint64) -> u64 {
    let mut buffer = [0u8; 8];
    buffer.copy_from_slice(value.as_slice());
    u64::from_le_bytes(buffer)
}
