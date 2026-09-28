//! Shared fixtures, builders and assertions for the voting system type script
//! tests.
//!
//! Every test bootstraps its own [`Fixture`]: the bootstrap deploys the five
//! contract binaries - including the `always success` lock - and derives every
//! script identity from those deployed cells. Nothing here hard-codes a
//! synthetic code hash: the config cell points at the scripts the tests
//! actually deploy, exactly like production deployment does.
//!
//! A [`Fixture`] also plays the role of a tiny chain: it hands out block
//! headers, links cells to the block that created them and can assemble the
//! `header_deps` a transaction needs for the [`load_header`](ckb_std)
//! syscalls. See the module level comments on [`Fixture::at_block`].

use ckb_testtool::{
    ckb_hash::new_blake2b,
    ckb_script::{ScriptError, TransactionScriptError},
    ckb_types::{
        bytes::Bytes,
        core::{HeaderBuilder, HeaderView, TransactionView},
        packed::*,
        prelude::*,
    },
    context::Context,
};
use ckb_vote_common::{constants, error::Error, hash, status};
use ckb_vote_types::molecules::types::{Counting, ProposalCellData, Vote, VotingConfig};
use std::collections::HashMap;

/// Maximum number of cycles a test transaction may consume.
pub const MAX_CYCLES: u64 = 10_000_000;
/// 1 CKB, in shannons.
pub const ONE_CKB: u64 = 100_000_000;

/// `config.yes_threshold` of the default fixture.
pub const YES_THRESHOLD: u64 = 500;
/// `config.minimal_proposal_capacity` of the default fixture.
pub const MINIMAL_PROPOSAL_CAPACITY: u64 = 100 * ONE_CKB;
/// `config.vote_duration` of the default fixture.
pub const VOTE_DURATION: u64 = 10;
/// `config.vote_window` of the default fixture.
pub const VOTE_WINDOW: u64 = 10;
/// `config.challenge_time` of the default fixture.
pub const CHALLENGE_TIME: u64 = 10;

/// Capacity of the default proposal cell: `config.minimal_proposal_capacity`
/// plus a margin.
pub const PROPOSAL_CAPACITY: u64 = 150 * ONE_CKB;
/// Capacity of the default config cell.
pub const CONFIG_CAPACITY: u64 = 500 * ONE_CKB;
/// Capacity of a plain funding input.
pub const FUNDING_CAPACITY: u64 = 1000 * ONE_CKB;
/// Capacity of the default vote / counting cell.
pub const CERTIFICATE_CAPACITY: u64 = 200 * ONE_CKB;
/// The default `vote_amount` of a vote cell and of the DAO deposit backing it.
pub const VOTE_AMOUNT: u64 = 100 * ONE_CKB;
/// `config.minimal_vote_amount` of the default fixture.
pub const MINIMAL_VOTE_AMOUNT: u64 = VOTE_AMOUNT;
/// The block that creates a proposal in the default fixture.
pub const PROPOSAL_BLOCK: u64 = 100;
/// The Type ID tail of the proposal script of the default fixture.
pub const PROPOSAL_TYPE_ID: [u8; constants::TYPE_ID_LEN] = [0x33u8; constants::TYPE_ID_LEN];

// --------------------------------------------------------------------------
// `since` values (RFC 0017)
// --------------------------------------------------------------------------

/// A relative `since` with the block number metric.
///
/// `ckb-std` treats a `since` as relative when the highest bit is set
/// (`Since::is_absolute`); the metric flags (bits 61 and 62) stay clear for the
/// block number metric. The value is then a plain block count, which is what
/// `config.vote_duration` and friends are compared with.
pub fn relative_since(blocks: u64) -> u64 {
    0x8000_0000_0000_0000 | blocks
}

/// A relative `since` with the timestamp metric: the value is a number of
/// seconds, not of blocks.
pub fn relative_timestamp_since(seconds: u64) -> u64 {
    0x8000_0000_0000_0000 | 0x4000_0000_0000_0000 | seconds
}

/// An absolute `since`: the highest bit is clear.
pub fn absolute_since(blocks: u64) -> u64 {
    blocks
}

// --------------------------------------------------------------------------
// Scripts, hashes and molecule payloads
// --------------------------------------------------------------------------

/// The Type ID rule: `blake160(first input of the transaction || output index)`.
pub fn type_id(input: &CellInput, output_index: u64) -> [u8; constants::TYPE_ID_LEN] {
    let mut hasher = new_blake2b();
    hasher.update(input.as_slice());
    hasher.update(&output_index.to_le_bytes());
    let mut digest = [0u8; 32];
    hasher.finalize(&mut digest);
    let mut id = [0u8; constants::TYPE_ID_LEN];
    id.copy_from_slice(&digest[..constants::TYPE_ID_LEN]);
    id
}

/// Proposal type script args: `blake160(config type script) || Type ID`.
pub fn proposal_args(config_type_script: &Script, input: &CellInput, output_index: u64) -> Bytes {
    let mut args = hash::blake160(config_type_script.as_slice()).to_vec();
    args.extend_from_slice(&type_id(input, output_index));
    args.into()
}

/// Turns a deployed contract into the `always success` lock script with `args`.
pub fn always_success_lock(context: &mut Context, out_point: &OutPoint, args: Bytes) -> Script {
    context
        .build_script(out_point, args)
        .expect("always success lock")
}

/// The 2 byte prefix of a lock script hash, read as a big endian `u16`.
///
/// Counting cells slice the voter set with exactly this value.
pub fn lock_prefix(lock: &Script) -> u16 {
    let hash = hash::script_hash(lock);
    u16::from_be_bytes([hash[0], hash[1]])
}

/// A `code` cell dep on `out_point`.
pub fn dep(out_point: &OutPoint) -> CellDep {
    CellDep::new_builder().out_point(out_point.clone()).build()
}

/// The real Nervos DAO type script (RFC 0024): a vote is backed by its deposits.
pub fn dao_type_script() -> Script {
    Script::new_builder()
        .code_hash(constants::DAO_TYPE_SCRIPT_CODE_HASH.pack())
        .hash_type(constants::DAO_TYPE_SCRIPT_HASH_TYPE)
        .args(Bytes::new().pack())
        .build()
}

/// A well formed `ProposalCellData` payload.
pub fn proposal_data(
    status: u8,
    requested_amount: u64,
    recipient_lock_hash: [u8; 20],
    total_yes: u64,
    origin_block_number: u64,
) -> Bytes {
    ProposalCellData::new_builder()
        .status(status)
        .description(b"a proposal under test".to_vec())
        .requested_amount(requested_amount.to_le_bytes())
        .recipient_lock_hash(recipient_lock_hash)
        .total_yes(total_yes.to_le_bytes())
        .origin_block_number(origin_block_number.to_le_bytes())
        .build()
        .as_slice()
        .to_vec()
        .into()
}

/// A well formed `Vote` payload.
pub fn vote_data(vote_amount: u64, direction: u8) -> Bytes {
    Vote::new_builder()
        .vote_amount(vote_amount.to_le_bytes())
        .direction(direction)
        .build()
        .as_slice()
        .to_vec()
        .into()
}

/// A well formed `Counting` payload, with `[start_hash, end_hash]` read as big
/// endian `u16` bounds.
pub fn counting_data(start_hash: u16, end_hash: u16, direction: u8, vote_amount: u64) -> Bytes {
    Counting::new_builder()
        .start_hash(start_hash.to_be_bytes())
        .end_hash(end_hash.to_be_bytes())
        .direction(direction)
        .vote_amount(vote_amount.to_le_bytes())
        .build()
        .as_slice()
        .to_vec()
        .into()
}

/// A `Byte32` as a plain array.
fn digest_of(value: Byte32) -> [u8; 32] {
    value.as_slice().try_into().expect("32 bytes")
}

// --------------------------------------------------------------------------
// Specs
// --------------------------------------------------------------------------

/// The fields of `VotingConfig` a test may want to change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigValues {
    pub emergent_halt: u8,
    pub yes_threshold: u64,
    pub minimal_proposal_capacity: u64,
    pub vote_duration: u64,
    pub vote_window: u64,
    pub challenge_time: u64,
    /// The full 32 byte ckb hash of the lock allowed to veto a proposal.
    pub veto_lock_script_hash: [u8; 32],
    /// The minimum `vote_amount` a vote cell may declare.
    pub minimal_vote_amount: u64,
}

impl Default for ConfigValues {
    fn default() -> Self {
        ConfigValues {
            emergent_halt: 0,
            yes_threshold: YES_THRESHOLD,
            minimal_proposal_capacity: MINIMAL_PROPOSAL_CAPACITY,
            vote_duration: VOTE_DURATION,
            vote_window: VOTE_WINDOW,
            challenge_time: CHALLENGE_TIME,
            veto_lock_script_hash: [0u8; 32],
            minimal_vote_amount: MINIMAL_VOTE_AMOUNT,
        }
    }
}

/// Everything needed to create a proposal cell in a [`Fixture`].
#[derive(Debug, Clone)]
pub struct ProposalSpec {
    /// The Type ID tail of the proposal type script.
    pub type_id: [u8; constants::TYPE_ID_LEN],
    pub status: u8,
    pub total_yes: u64,
    /// The block that created the original proposal cell: `0` for an open
    /// proposal, the original block for a finalized or passed one.
    pub origin_block_number: u64,
    pub requested_amount: u64,
    pub recipient_lock_hash: [u8; 20],
    pub capacity: u64,
    pub lock: Script,
    /// The block that creates the cell.
    pub block: u64,
}

/// Everything needed to create a vote cell in a [`Fixture`].
#[derive(Debug, Clone)]
pub struct VoteSpec {
    /// `blake160(proposal type script)`.
    pub proposal_id: [u8; constants::PROPOSAL_ID_LEN],
    pub direction: u8,
    pub vote_amount: u64,
    pub capacity: u64,
    pub lock: Script,
    /// When set, the cell is linked to that block so that a counting cell can
    /// read its block number.
    pub block: Option<u64>,
}

/// Everything needed to create a counting cell in a [`Fixture`].
#[derive(Debug, Clone)]
pub struct CountingSpec {
    /// `blake160(proposal type script)`.
    pub proposal_id: [u8; constants::PROPOSAL_ID_LEN],
    pub direction: u8,
    pub start_hash: u16,
    pub end_hash: u16,
    pub vote_amount: u64,
    pub capacity: u64,
    pub lock: Script,
    pub block: Option<u64>,
}

/// A proposal cell that lives in a [`Fixture`] context.
#[derive(Debug, Clone)]
pub struct Proposal {
    pub out_point: OutPoint,
    /// The proposal type script itself.
    pub script: Script,
    /// `blake160(proposal type script)`: the id vote and counting cells carry.
    pub id: [u8; constants::PROPOSAL_ID_LEN],
    pub requested_amount: u64,
    pub recipient_lock_hash: [u8; 20],
    pub capacity: u64,
    /// The `origin_block_number` recorded in the cell data.
    pub origin_block_number: u64,
    /// The header of the creating block; list it in `header_deps` when a script
    /// reads this cell's block number.
    pub block_hash: Byte32,
    pub block_number: u64,
}

/// A cell created in a [`Fixture`] context.
#[derive(Debug, Clone)]
pub struct PlacedCell {
    pub out_point: OutPoint,
    /// Set when the cell was linked to a block; list it in `header_deps`.
    pub block_hash: Option<Byte32>,
}

/// The output side of a proposal update.
///
/// The type script is always the one of the consumed proposal cell, so that
/// both ends fall in the same script group.
#[derive(Debug, Clone)]
pub struct ProposalOutput {
    pub status: u8,
    pub total_yes: u64,
    /// The `origin_block_number` of the output cell data.
    pub origin_block_number: u64,
    pub requested_amount: u64,
    pub recipient_lock_hash: [u8; 20],
    pub capacity: u64,
    pub lock: Script,
}

// --------------------------------------------------------------------------
// Fixture
// --------------------------------------------------------------------------

/// A context with the four contracts deployed and a default config cell.
pub struct Fixture {
    pub context: Context,
    pub config_out_point: OutPoint,
    pub proposal_out_point: OutPoint,
    pub vote_out_point: OutPoint,
    pub counting_out_point: OutPoint,

    /// `always success` with empty args: the initiator's lock.
    pub lock: Script,
    /// `always success` with distinct args: the grant recipient.
    pub recipient_lock: Script,
    /// `always success` with distinct args: a voter.
    pub voter_lock: Script,
    /// `always success` with distinct args: the administrator.
    pub veto_lock: Script,

    pub vote_code_hash: [u8; 32],
    pub vote_hash_type: u8,
    pub counting_code_hash: [u8; 32],
    pub counting_hash_type: u8,
    pub always_success_code_hash: [u8; 32],
    pub always_success_hash_type: u8,

    /// `blake160(config type script)`.
    pub config_id: [u8; constants::CONFIG_ID_LEN],
    pub config_script: Script,
    pub config_cell: OutPoint,
    pub config: ConfigValues,

    headers: HashMap<u64, HeaderView>,
}

impl Fixture {
    /// Deploys the contracts and mints the default config cell.
    pub fn new() -> Self {
        let mut context = Context::default();
        let always_success_out_point = context.deploy_cell_by_name("always-success");
        let config_out_point = context.deploy_cell_by_name("config-type-script");
        let proposal_out_point = context.deploy_cell_by_name("proposal-type-script");
        let vote_out_point = context.deploy_cell_by_name("vote-type-script");
        let counting_out_point = context.deploy_cell_by_name("counting-type-script");

        let lock = always_success_lock(&mut context, &always_success_out_point, Bytes::new());
        let recipient_lock = always_success_lock(
            &mut context,
            &always_success_out_point,
            Bytes::from(vec![1u8]),
        );
        let voter_lock = always_success_lock(
            &mut context,
            &always_success_out_point,
            Bytes::from(vec![2u8]),
        );
        let veto_lock = always_success_lock(
            &mut context,
            &always_success_out_point,
            Bytes::from(vec![3u8]),
        );

        let config_script = context
            .build_script(
                &config_out_point,
                Bytes::from(vec![0x11u8; constants::TYPE_ID_LEN]),
            )
            .expect("config script");

        // The code hash of a script depends only on the deployed code cell, so
        // any args do; the config records the identity of the real binaries.
        let vote_template = context
            .build_script(&vote_out_point, Bytes::new())
            .expect("vote script");
        let counting_template = context
            .build_script(&counting_out_point, Bytes::new())
            .expect("counting script");

        let mut fixture = Fixture {
            always_success_code_hash: digest_of(lock.code_hash()),
            always_success_hash_type: lock.hash_type().as_slice()[0],
            vote_code_hash: digest_of(vote_template.code_hash()),
            vote_hash_type: vote_template.hash_type().as_slice()[0],
            counting_code_hash: digest_of(counting_template.code_hash()),
            counting_hash_type: counting_template.hash_type().as_slice()[0],
            config_id: hash::blake160(config_script.as_slice()),
            context,
            config_out_point,
            proposal_out_point,
            vote_out_point,
            counting_out_point,
            lock,
            recipient_lock,
            voter_lock,
            veto_lock,
            config_script,
            config_cell: OutPoint::default(),
            config: ConfigValues::default(),
            headers: HashMap::new(),
        };
        fixture.config.veto_lock_script_hash = hash::script_hash(&fixture.veto_lock);
        fixture.refresh_config_cell();
        fixture
    }

    /// Builds a script from `out_point` and `args`.
    pub fn script(&mut self, out_point: &OutPoint, args: Bytes) -> Script {
        self.context
            .build_script(out_point, args)
            .expect("build script")
    }

    /// The default config type script, with caller chosen `args`.
    pub fn config_script_with_args(&mut self, args: Bytes) -> Script {
        let out_point = self.config_out_point.clone();
        self.script(&out_point, args)
    }

    /// A lock script unrelated to `always success`, for negative lock tests.
    ///
    /// It is built from the config contract, which the tests deploy anyway, so
    /// the output cell does not need an extra code cell.
    pub fn foreign_lock(&mut self) -> Script {
        let out_point = self.config_out_point.clone();
        self.script(&out_point, Bytes::new())
    }

    /// The proposal type script for `type_id`, bound to this fixture's config.
    pub fn proposal_script(&mut self, type_id: [u8; constants::TYPE_ID_LEN]) -> Script {
        let mut args = self.config_id.to_vec();
        args.extend_from_slice(&type_id);
        let out_point = self.proposal_out_point.clone();
        self.script(&out_point, args.into())
    }

    /// The proposal type script that follows the Type ID rule for the given
    /// first input and output index.
    pub fn proposal_script_for_creation(
        &mut self,
        first_input: &CellInput,
        output_index: u64,
    ) -> Script {
        let args = proposal_args(&self.config_script, first_input, output_index);
        let out_point = self.proposal_out_point.clone();
        self.script(&out_point, args)
    }

    /// The proposal type script with caller chosen raw `args`.
    pub fn proposal_script_with_args(&mut self, args: Bytes) -> Script {
        let out_point = self.proposal_out_point.clone();
        self.script(&out_point, args)
    }

    /// The vote type script that points at `proposal_id`.
    pub fn vote_script(&mut self, proposal_id: [u8; constants::PROPOSAL_ID_LEN]) -> Script {
        let out_point = self.vote_out_point.clone();
        self.script(&out_point, proposal_id.to_vec().into())
    }

    /// The counting type script that points at `proposal_id`.
    pub fn counting_script(&mut self, proposal_id: [u8; constants::PROPOSAL_ID_LEN]) -> Script {
        let out_point = self.counting_out_point.clone();
        self.script(&out_point, proposal_id.to_vec().into())
    }

    /// The current config cell payload.
    pub fn config_data(&self) -> Bytes {
        VotingConfig::new_builder()
            .emergent_halt(self.config.emergent_halt)
            .vote_code_hash(self.vote_code_hash)
            .vote_hash_type(self.vote_hash_type)
            .counting_code_hash(self.counting_code_hash)
            .counting_hash_type(self.counting_hash_type)
            .always_success_code_hash(self.always_success_code_hash)
            .always_success_hash_type(self.always_success_hash_type)
            .yes_threshold(self.config.yes_threshold.to_le_bytes())
            .minimal_proposal_capacity(self.config.minimal_proposal_capacity.to_le_bytes())
            .vote_duration(self.config.vote_duration.to_le_bytes())
            .vote_window(self.config.vote_window.to_le_bytes())
            .challenge_time(self.config.challenge_time.to_le_bytes())
            .veto_lock_script_hash(self.config.veto_lock_script_hash)
            .minimal_vote_amount(self.config.minimal_vote_amount.to_le_bytes())
            .build()
            .as_slice()
            .to_vec()
            .into()
    }

    /// Creates a config cell out of the current [`ConfigValues`] and remembers
    /// it as *the* config cell of this fixture.
    pub fn refresh_config_cell(&mut self) -> OutPoint {
        let data = self.config_data();
        let lock = self.lock.clone();
        let script = self.config_script.clone();
        let out_point = self.create_cell(&lock, CONFIG_CAPACITY, Some(script), data);
        self.config_cell = out_point.clone();
        out_point
    }

    /// Creates a cell with no type script.
    pub fn funding_cell(&mut self, lock: &Script, capacity: u64) -> OutPoint {
        self.create_cell(lock, capacity, None, Bytes::new())
    }

    /// Creates a cell, optionally with a type script.
    pub fn create_cell(
        &mut self,
        lock: &Script,
        capacity: u64,
        type_script: Option<Script>,
        data: Bytes,
    ) -> OutPoint {
        let mut output = CellOutput::new_builder()
            .capacity(capacity)
            .lock(lock.clone());
        if let Some(script) = type_script {
            output = output.type_(Some(script).pack());
        }
        self.context.create_cell(output.build(), data)
    }

    /// Inserts `out_point` into the chain at `block` and returns the hash of
    /// that block.
    ///
    /// Any script that reads the block number of a cell through
    /// `load_header(..., Source::CellDep)` only sees the value when the cell is
    /// linked this way *and* the returned hash is listed in the transaction's
    /// `header_deps`.
    pub fn at_block(&mut self, out_point: &OutPoint, block: u64) -> Byte32 {
        let header = match self.headers.get(&block) {
            Some(header) => header.clone(),
            None => {
                // A non-genesis header needs a well formed epoch: number 0,
                // index 0, length 1. The epoch length lives at bit 40 of the
                // packed `EpochNumberWithFraction`.
                let header = HeaderBuilder::default()
                    .number(block)
                    .epoch(1u64 << 40)
                    .build();
                self.context.insert_header(header.clone());
                self.headers.insert(block, header.clone());
                header
            }
        };
        self.context
            .link_cell_with_block(out_point.clone(), header.hash(), 0);
        header.hash()
    }

    /// `header_deps` in a canonical (sorted, deduplicated) order.
    pub fn header_deps(mut hashes: Vec<Byte32>) -> Vec<Byte32> {
        hashes.sort();
        hashes.dedup();
        hashes
    }

    /// The ckb-blake160-hash of the fixture's recipient lock.
    pub fn recipient_lock_hash(&self) -> [u8; 20] {
        hash::blake160(self.recipient_lock.as_slice())
    }

    /// The default proposal cell description of this fixture.
    pub fn proposal_spec(&self) -> ProposalSpec {
        ProposalSpec {
            type_id: PROPOSAL_TYPE_ID,
            status: status::PROPOSAL_STATUS_OPEN,
            total_yes: 0,
            origin_block_number: 0,
            requested_amount: VOTE_AMOUNT,
            recipient_lock_hash: hash::blake160(self.recipient_lock.as_slice()),
            capacity: PROPOSAL_CAPACITY,
            lock: self.lock.clone(),
            block: PROPOSAL_BLOCK,
        }
    }

    /// Creates a proposal cell and links it to `spec.block`.
    pub fn proposal_cell(&mut self, spec: &ProposalSpec) -> Proposal {
        let script = self.proposal_script(spec.type_id);
        let data = proposal_data(
            spec.status,
            spec.requested_amount,
            spec.recipient_lock_hash,
            spec.total_yes,
            spec.origin_block_number,
        );
        let out_point = self.create_cell(&spec.lock, spec.capacity, Some(script.clone()), data);
        let block_hash = self.at_block(&out_point, spec.block);
        Proposal {
            id: hash::script_id(&script),
            out_point,
            script,
            requested_amount: spec.requested_amount,
            recipient_lock_hash: spec.recipient_lock_hash,
            capacity: spec.capacity,
            origin_block_number: spec.origin_block_number,
            block_hash,
            block_number: spec.block,
        }
    }

    /// The default update output for `proposal`: the bond is preserved and the
    /// finalized lock is the fixture's `always success` lock.
    pub fn proposal_output(&self, proposal: &Proposal) -> ProposalOutput {
        ProposalOutput {
            status: status::PROPOSAL_STATUS_FINALIZED,
            total_yes: 0,
            origin_block_number: proposal.origin_block_number,
            requested_amount: proposal.requested_amount,
            recipient_lock_hash: proposal.recipient_lock_hash,
            capacity: proposal.capacity,
            lock: self.lock.clone(),
        }
    }

    /// The default vote cell description of this fixture, pointing at
    /// `proposal`.
    pub fn vote_spec(&self, proposal: &Proposal) -> VoteSpec {
        VoteSpec {
            proposal_id: proposal.id,
            direction: status::DIRECTION_YES,
            vote_amount: VOTE_AMOUNT,
            capacity: CERTIFICATE_CAPACITY,
            lock: self.voter_lock.clone(),
            block: None,
        }
    }

    /// Creates a vote cell.
    pub fn vote_cell(&mut self, spec: &VoteSpec) -> PlacedCell {
        let script = self.vote_script(spec.proposal_id);
        let data = vote_data(spec.vote_amount, spec.direction);
        let out_point = self.create_cell(&spec.lock, spec.capacity, Some(script), data);
        let block_hash = spec.block.map(|block| self.at_block(&out_point, block));
        PlacedCell {
            out_point,
            block_hash,
        }
    }

    /// The default counting cell description of this fixture, pointing at
    /// `proposal` and covering every voter.
    pub fn counting_spec(&self, proposal: &Proposal) -> CountingSpec {
        CountingSpec {
            proposal_id: proposal.id,
            direction: status::DIRECTION_YES,
            start_hash: 0,
            end_hash: u16::MAX,
            vote_amount: VOTE_AMOUNT,
            capacity: CERTIFICATE_CAPACITY,
            lock: self.lock.clone(),
            block: Some(proposal.block_number + VOTE_DURATION + 1),
        }
    }

    /// Creates a counting cell.
    pub fn counting_cell(&mut self, spec: &CountingSpec) -> PlacedCell {
        let script = self.counting_script(spec.proposal_id);
        let data = counting_data(
            spec.start_hash,
            spec.end_hash,
            spec.direction,
            spec.vote_amount,
        );
        let out_point = self.create_cell(&spec.lock, spec.capacity, Some(script), data);
        let block_hash = spec.block.map(|block| self.at_block(&out_point, block));
        PlacedCell {
            out_point,
            block_hash,
        }
    }

    /// Creates a Nervos DAO deposit cell, the only cell a vote may be backed by.
    pub fn dao_deposit_cell(
        &mut self,
        lock: &Script,
        capacity: u64,
        block: Option<u64>,
    ) -> PlacedCell {
        let out_point = self.create_cell(lock, capacity, Some(dao_type_script()), Bytes::new());
        let block_hash = block.map(|block| self.at_block(&out_point, block));
        PlacedCell {
            out_point,
            block_hash,
        }
    }
}

// --------------------------------------------------------------------------
// Assertions
// --------------------------------------------------------------------------

/// Asserts that `tx` is rejected by a script with exactly the exit code of
/// `expected`.
///
/// The exit code is extracted from the `ckb_error::Error` chain: the top level
/// error is a `ckb_script::TransactionScriptError` whose `ScriptError` is the
/// `ValidationFailure` carrying the numeric code the script returned.
pub fn assert_script_error(context: &Context, tx: &TransactionView, expected: Error) {
    match context.verify_tx(tx, MAX_CYCLES) {
        Ok(cycles) => panic!(
            "expected the transaction to be rejected with {:?} (exit code {}), \
             but it succeeded after {} cycles",
            expected, expected as i8, cycles
        ),
        Err(error) => {
            let Some(script_error) = error.downcast_ref::<TransactionScriptError>() else {
                panic!(
                    "expected a script validation failure with {:?} (exit code {}), \
                     but the transaction failed with a different error: {}",
                    expected, expected as i8, error
                );
            };
            match script_error.script_error() {
                ScriptError::ValidationFailure(_, code) => {
                    assert_eq!(
                        *code, expected as i8,
                        "the script rejected the transaction with exit code {} instead of \
                         {:?} (exit code {}); full error: {}",
                        code, expected, expected as i8, error
                    );
                }
                other => panic!(
                    "expected a script validation failure with {:?} (exit code {}), \
                     but the script failed with {:?}; full error: {}",
                    expected, expected as i8, other, error
                ),
            }
        }
    }
}
