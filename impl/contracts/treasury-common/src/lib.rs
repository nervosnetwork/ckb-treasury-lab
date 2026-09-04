#![no_std]

extern crate alloc;

use alloc::{collections::VecDeque, vec::Vec};
use ckb_hash::new_blake2b;
use merkle_cbt::{MerkleProof, merkle_tree::Merge};
use sparse_merkle_tree::{CompiledMerkleProof, H256, blake2b::Blake2bHasher};

pub const VERSION: u8 = 2;
pub const TALLY_WITNESS_VERSION: u8 = 7;
pub const VOTE_STATE_NAMESPACE: u8 = 0;
pub const EVENT_STATE_NAMESPACE: u8 = 1;
pub const PROPOSAL_DATA_LEN: usize = 182;
pub const TALLY_STATE_LEN: usize = 162;
pub const RESULT_DATA_LEN: usize = 202;
pub const PROPOSAL_CONFIG_LEN: usize = 367;
pub const TREASURY_CONFIG_LEN: usize = 97;
pub const EVENT_PRESENT: Hash = [
    1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

pub const TALLY_ACTION_ADVANCE: u8 = 0;
pub const TALLY_ACTION_CHALLENGE_VOTE: u8 = 1;
pub const TALLY_ACTION_FINALIZE: u8 = 3;
pub const TALLY_ACTION_CLEANUP_VETOED: u8 = 4;

pub type Hash = [u8; 32];

pub fn namespaced_state_key(namespace: u8, key: Hash) -> Option<Hash> {
    if namespace > EVENT_STATE_NAMESPACE {
        return None;
    }
    let mut output = [0u8; 32];
    let mut hasher = new_blake2b();
    hasher.update(b"CKB Treasury state key V1");
    hasher.update(&[namespace]);
    hasher.update(&key);
    hasher.finalize(&mut output);
    Some(output)
}

fn is_valid_script_hash_type(value: u8) -> bool {
    matches!(value, 0 | 1 | 2 | 4)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodecError {
    Truncated,
    TrailingBytes,
    InvalidVersion,
    InvalidValue,
    Overflow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ProposalPhase {
    Open = 0,
    Closed = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ProposalOutcome {
    Passed = 0,
    RejectedByVote = 1,
    Vetoed = 2,
}

impl TryFrom<u8> for ProposalOutcome {
    type Error = CodecError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Passed),
            1 => Ok(Self::RejectedByVote),
            2 => Ok(Self::Vetoed),
            _ => Err(CodecError::InvalidValue),
        }
    }
}

impl TryFrom<u8> for ProposalPhase {
    type Error = CodecError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Open),
            1 => Ok(Self::Closed),
            _ => Err(CodecError::InvalidValue),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TallyPhase {
    Active = 0,
    Candidate = 1,
}

impl TryFrom<u8> for TallyPhase {
    type Error = CodecError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Active),
            1 => Ok(Self::Candidate),
            _ => Err(CodecError::InvalidValue),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct OutPoint {
    pub tx_hash: Hash,
    pub index: u32,
}

impl OutPoint {
    pub const ENCODED_LEN: usize = 36;

    pub fn key(&self) -> Hash {
        let mut encoded = [0u8; Self::ENCODED_LEN];
        encoded[..32].copy_from_slice(&self.tx_hash);
        encoded[32..].copy_from_slice(&self.index.to_le_bytes());
        blake2b_256(&encoded)
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, CodecError> {
        Ok(Self {
            tx_hash: reader.hash()?,
            index: reader.u32()?,
        })
    }

    fn encode_into(&self, output: &mut Vec<u8>) {
        output.extend_from_slice(&self.tx_hash);
        output.extend_from_slice(&self.index.to_le_bytes());
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoteData {
    pub direction: u8,
    pub amount: u64,
    pub dao_out_points: Vec<OutPoint>,
}

impl VoteData {
    pub fn decode(data: &[u8]) -> Result<Self, CodecError> {
        let mut reader = Reader::new(data);
        reader.version()?;
        let direction = reader.u8()?;
        if direction > 1 {
            return Err(CodecError::InvalidValue);
        }
        let amount = reader.u64()?;
        let count = reader.u16()? as usize;
        if count == 0 {
            return Err(CodecError::InvalidValue);
        }
        let mut dao_out_points = Vec::with_capacity(count);
        for _ in 0..count {
            dao_out_points.push(OutPoint::decode(&mut reader)?);
        }
        reader.finish()?;
        Ok(Self {
            direction,
            amount,
            dao_out_points,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>, CodecError> {
        if self.direction > 1 || self.dao_out_points.is_empty() {
            return Err(CodecError::InvalidValue);
        }
        let count: u16 = self
            .dao_out_points
            .len()
            .try_into()
            .map_err(|_| CodecError::Overflow)?;
        let mut output = Vec::with_capacity(12 + self.dao_out_points.len() * OutPoint::ENCODED_LEN);
        output.push(VERSION);
        output.push(self.direction);
        output.extend_from_slice(&self.amount.to_le_bytes());
        output.extend_from_slice(&count.to_le_bytes());
        for out_point in &self.dao_out_points {
            out_point.encode_into(&mut output);
        }
        Ok(output)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoteRecord {
    pub voter_lock_hash: Hash,
    pub direction: u8,
    pub amount: u64,
    pub block_number: u64,
    pub tx_index: u32,
}

impl VoteRecord {
    pub fn decode(data: &[u8]) -> Result<Self, CodecError> {
        let mut reader = Reader::new(data);
        reader.version()?;
        let voter_lock_hash = reader.hash()?;
        let direction = reader.u8()?;
        if direction > 1 {
            return Err(CodecError::InvalidValue);
        }
        let amount = reader.u64()?;
        let block_number = reader.u64()?;
        let tx_index = reader.u32()?;
        reader.finish()?;
        Ok(Self {
            voter_lock_hash,
            direction,
            amount,
            block_number,
            tx_index,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>, CodecError> {
        if self.direction > 1 {
            return Err(CodecError::InvalidValue);
        }
        let mut output = Vec::with_capacity(54);
        output.push(VERSION);
        output.extend_from_slice(&self.voter_lock_hash);
        output.push(self.direction);
        output.extend_from_slice(&self.amount.to_le_bytes());
        output.extend_from_slice(&self.block_number.to_le_bytes());
        output.extend_from_slice(&self.tx_index.to_le_bytes());
        Ok(output)
    }

    pub fn value_hash(&self) -> Result<Hash, CodecError> {
        self.encode().map(|encoded| blake2b_256(&encoded))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposalData {
    pub phase: ProposalPhase,
    pub start_block: u64,
    pub end_block: u64,
    pub challenge_period: u64,
    pub max_events_per_batch: u16,
    pub max_dao_deps_per_vote: u16,
    pub max_state_keys_per_batch: u16,
    pub max_batch_sequence: u16,
    pub max_batch_witness_bytes: u32,
    pub minimum_vote_capacity: u64,
    pub requested_amount: u64,
    pub receiver_lock_hash: Hash,
    pub proposer_lock_hash: Hash,
    pub proposal_config_type_hash: Hash,
    pub metadata_hash: Hash,
}

impl ProposalData {
    pub fn decode(data: &[u8]) -> Result<Self, CodecError> {
        if data.len() != PROPOSAL_DATA_LEN {
            return Err(CodecError::InvalidValue);
        }
        let mut reader = Reader::new(data);
        reader.version()?;
        let phase = ProposalPhase::try_from(reader.u8()?)?;
        let value = Self {
            phase,
            start_block: reader.u64()?,
            end_block: reader.u64()?,
            challenge_period: reader.u64()?,
            max_events_per_batch: reader.u16()?,
            max_dao_deps_per_vote: reader.u16()?,
            max_state_keys_per_batch: reader.u16()?,
            max_batch_sequence: reader.u16()?,
            max_batch_witness_bytes: reader.u32()?,
            minimum_vote_capacity: reader.u64()?,
            requested_amount: reader.u64()?,
            receiver_lock_hash: reader.hash()?,
            proposer_lock_hash: reader.hash()?,
            proposal_config_type_hash: reader.hash()?,
            metadata_hash: reader.hash()?,
        };
        reader.finish()?;
        if value.start_block >= value.end_block
            || value.challenge_period == 0
            || value.max_events_per_batch == 0
            || value.max_dao_deps_per_vote == 0
            || value.max_state_keys_per_batch == 0
            || value.max_batch_sequence == 0
            || value.max_batch_witness_bytes == 0
            || value.minimum_vote_capacity == 0
            || value.requested_amount == 0
            || value.proposer_lock_hash == [0; 32]
            || value.proposal_config_type_hash == [0; 32]
        {
            return Err(CodecError::InvalidValue);
        }
        Ok(value)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut output = Vec::with_capacity(PROPOSAL_DATA_LEN);
        output.push(VERSION);
        output.push(self.phase as u8);
        output.extend_from_slice(&self.start_block.to_le_bytes());
        output.extend_from_slice(&self.end_block.to_le_bytes());
        output.extend_from_slice(&self.challenge_period.to_le_bytes());
        output.extend_from_slice(&self.max_events_per_batch.to_le_bytes());
        output.extend_from_slice(&self.max_dao_deps_per_vote.to_le_bytes());
        output.extend_from_slice(&self.max_state_keys_per_batch.to_le_bytes());
        output.extend_from_slice(&self.max_batch_sequence.to_le_bytes());
        output.extend_from_slice(&self.max_batch_witness_bytes.to_le_bytes());
        output.extend_from_slice(&self.minimum_vote_capacity.to_le_bytes());
        output.extend_from_slice(&self.requested_amount.to_le_bytes());
        output.extend_from_slice(&self.receiver_lock_hash);
        output.extend_from_slice(&self.proposer_lock_hash);
        output.extend_from_slice(&self.proposal_config_type_hash);
        output.extend_from_slice(&self.metadata_hash);
        output
    }

    pub fn immutable_fields_equal(&self, other: &Self) -> bool {
        let mut lhs = self.clone();
        let mut rhs = other.clone();
        lhs.phase = ProposalPhase::Open;
        rhs.phase = ProposalPhase::Open;
        lhs == rhs
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TallyState {
    pub phase: TallyPhase,
    pub proposal_id: Hash,
    pub operator_lock_hash: Hash,
    pub sequence: u32,
    pub next_block: u64,
    pub next_tx_index: u32,
    pub state_root: Hash,
    pub yes: u128,
    pub no: u128,
    pub processed_events: u64,
    pub candidate_since: u64,
}

impl TallyState {
    pub fn decode(data: &[u8]) -> Result<Self, CodecError> {
        if data.len() != TALLY_STATE_LEN {
            return Err(CodecError::InvalidValue);
        }
        let mut reader = Reader::new(data);
        reader.version()?;
        let value = Self {
            phase: TallyPhase::try_from(reader.u8()?)?,
            proposal_id: reader.hash()?,
            operator_lock_hash: reader.hash()?,
            sequence: reader.u32()?,
            next_block: reader.u64()?,
            next_tx_index: reader.u32()?,
            state_root: reader.hash()?,
            yes: reader.u128()?,
            no: reader.u128()?,
            processed_events: reader.u64()?,
            candidate_since: reader.u64()?,
        };
        reader.finish()?;
        Ok(value)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut output = Vec::with_capacity(TALLY_STATE_LEN);
        output.push(VERSION);
        output.push(self.phase as u8);
        output.extend_from_slice(&self.proposal_id);
        output.extend_from_slice(&self.operator_lock_hash);
        output.extend_from_slice(&self.sequence.to_le_bytes());
        output.extend_from_slice(&self.next_block.to_le_bytes());
        output.extend_from_slice(&self.next_tx_index.to_le_bytes());
        output.extend_from_slice(&self.state_root);
        output.extend_from_slice(&self.yes.to_le_bytes());
        output.extend_from_slice(&self.no.to_le_bytes());
        output.extend_from_slice(&self.processed_events.to_le_bytes());
        output.extend_from_slice(&self.candidate_since.to_le_bytes());
        output
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResultData {
    pub outcome: ProposalOutcome,
    pub proposal_id: Hash,
    pub requested_amount: u64,
    pub receiver_lock_hash: Hash,
    pub yes: u128,
    pub no: u128,
    pub final_state_hash: Hash,
    pub proposal_config_data_hash: Hash,
    pub veto_reason_hash: Hash,
}

impl ResultData {
    pub fn decode(data: &[u8]) -> Result<Self, CodecError> {
        if data.len() != RESULT_DATA_LEN {
            return Err(CodecError::InvalidValue);
        }
        let mut reader = Reader::new(data);
        reader.version()?;
        let value = Self {
            outcome: ProposalOutcome::try_from(reader.u8()?)?,
            proposal_id: reader.hash()?,
            requested_amount: reader.u64()?,
            receiver_lock_hash: reader.hash()?,
            yes: reader.u128()?,
            no: reader.u128()?,
            final_state_hash: reader.hash()?,
            proposal_config_data_hash: reader.hash()?,
            veto_reason_hash: reader.hash()?,
        };
        reader.finish()?;
        let zero_tally = value.yes == 0 && value.no == 0 && value.final_state_hash == [0; 32];
        match value.outcome {
            ProposalOutcome::Vetoed if !zero_tally || value.veto_reason_hash == [0; 32] => {
                return Err(CodecError::InvalidValue);
            }
            ProposalOutcome::Passed | ProposalOutcome::RejectedByVote
                if value.veto_reason_hash != [0; 32] =>
            {
                return Err(CodecError::InvalidValue);
            }
            _ => {}
        }
        Ok(value)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut output = Vec::with_capacity(RESULT_DATA_LEN);
        output.push(VERSION);
        output.push(self.outcome as u8);
        output.extend_from_slice(&self.proposal_id);
        output.extend_from_slice(&self.requested_amount.to_le_bytes());
        output.extend_from_slice(&self.receiver_lock_hash);
        output.extend_from_slice(&self.yes.to_le_bytes());
        output.extend_from_slice(&self.no.to_le_bytes());
        output.extend_from_slice(&self.final_state_hash);
        output.extend_from_slice(&self.proposal_config_data_hash);
        output.extend_from_slice(&self.veto_reason_hash);
        output
    }

    pub fn is_passed(&self) -> bool {
        self.outcome == ProposalOutcome::Passed
    }

    pub fn is_vetoed(&self) -> bool {
        self.outcome == ProposalOutcome::Vetoed
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProposalConfig {
    pub approval_bps: u16,
    pub minimum_total_votes: u128,
    pub maximum_proposal_amount: u64,
    pub minimum_challenge_period: u64,
    pub minimum_tally_bond: u64,
    pub treasury_lock_hash: Hash,
    pub proposal_lock_hash: Hash,
    pub guardian_lock_hash: Hash,
    pub proposal_bond_burn_lock_hash: Hash,
    pub dao_code_hash: Hash,
    pub dao_hash_type: u8,
    pub proposal_code_hash: Hash,
    pub proposal_hash_type: u8,
    pub vote_code_hash: Hash,
    pub vote_hash_type: u8,
    pub tally_code_hash: Hash,
    pub tally_hash_type: u8,
    pub candidate_lock_hash: Hash,
    pub policy_type_hash: Hash,
}

impl ProposalConfig {
    pub fn decode(data: &[u8]) -> Result<Self, CodecError> {
        if data.len() != PROPOSAL_CONFIG_LEN {
            return Err(CodecError::InvalidValue);
        }
        let mut reader = Reader::new(data);
        reader.version()?;
        let value = Self {
            approval_bps: reader.u16()?,
            minimum_total_votes: reader.u128()?,
            maximum_proposal_amount: reader.u64()?,
            minimum_challenge_period: reader.u64()?,
            minimum_tally_bond: reader.u64()?,
            treasury_lock_hash: reader.hash()?,
            proposal_lock_hash: reader.hash()?,
            guardian_lock_hash: reader.hash()?,
            proposal_bond_burn_lock_hash: reader.hash()?,
            dao_code_hash: reader.hash()?,
            dao_hash_type: reader.u8()?,
            proposal_code_hash: reader.hash()?,
            proposal_hash_type: reader.u8()?,
            vote_code_hash: reader.hash()?,
            vote_hash_type: reader.u8()?,
            tally_code_hash: reader.hash()?,
            tally_hash_type: reader.u8()?,
            candidate_lock_hash: reader.hash()?,
            policy_type_hash: reader.hash()?,
        };
        reader.finish()?;
        if value.approval_bps == 0
            || value.approval_bps > 10_000
            || value.minimum_total_votes == 0
            || value.maximum_proposal_amount == 0
            || value.minimum_challenge_period == 0
            || value.minimum_tally_bond == 0
            || value.treasury_lock_hash == [0; 32]
            || value.proposal_lock_hash == [0; 32]
            || value.guardian_lock_hash == [0; 32]
            || value.proposal_bond_burn_lock_hash == [0; 32]
            || value.proposal_lock_hash == value.guardian_lock_hash
            || value.proposal_lock_hash == value.proposal_bond_burn_lock_hash
            || value.guardian_lock_hash == value.proposal_bond_burn_lock_hash
            || value.dao_code_hash == [0; 32]
            || value.proposal_code_hash == [0; 32]
            || value.vote_code_hash == [0; 32]
            || value.tally_code_hash == [0; 32]
            || value.candidate_lock_hash == [0; 32]
            || value.policy_type_hash == [0; 32]
            || !is_valid_script_hash_type(value.dao_hash_type)
            || !is_valid_script_hash_type(value.proposal_hash_type)
            || !is_valid_script_hash_type(value.vote_hash_type)
            || !is_valid_script_hash_type(value.tally_hash_type)
        {
            return Err(CodecError::InvalidValue);
        }
        Ok(value)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut output = Vec::with_capacity(PROPOSAL_CONFIG_LEN);
        output.push(VERSION);
        output.extend_from_slice(&self.approval_bps.to_le_bytes());
        output.extend_from_slice(&self.minimum_total_votes.to_le_bytes());
        output.extend_from_slice(&self.maximum_proposal_amount.to_le_bytes());
        output.extend_from_slice(&self.minimum_challenge_period.to_le_bytes());
        output.extend_from_slice(&self.minimum_tally_bond.to_le_bytes());
        output.extend_from_slice(&self.treasury_lock_hash);
        output.extend_from_slice(&self.proposal_lock_hash);
        output.extend_from_slice(&self.guardian_lock_hash);
        output.extend_from_slice(&self.proposal_bond_burn_lock_hash);
        output.extend_from_slice(&self.dao_code_hash);
        output.push(self.dao_hash_type);
        output.extend_from_slice(&self.proposal_code_hash);
        output.push(self.proposal_hash_type);
        output.extend_from_slice(&self.vote_code_hash);
        output.push(self.vote_hash_type);
        output.extend_from_slice(&self.tally_code_hash);
        output.push(self.tally_hash_type);
        output.extend_from_slice(&self.candidate_lock_hash);
        output.extend_from_slice(&self.policy_type_hash);
        output
    }

    pub fn passes(&self, yes: u128, no: u128, requested_amount: u64) -> bool {
        let Some(total) = yes.checked_add(no) else {
            return false;
        };
        let Some(weighted_yes) = yes.checked_mul(10_000) else {
            return false;
        };
        let Some(threshold) = total.checked_mul(self.approval_bps as u128) else {
            return false;
        };
        requested_amount <= self.maximum_proposal_amount
            && total >= self.minimum_total_votes
            && weighted_yes >= threshold
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TreasuryConfig {
    pub burn_expiry_blocks: u64,
    pub base_burn_incentive: u64,
    pub burn_incentive_rate: u64,
    pub maximum_burn_incentive: u64,
    pub result_type_hash: Hash,
    pub zero_lock_hash: Hash,
}

impl TreasuryConfig {
    pub fn decode(data: &[u8]) -> Result<Self, CodecError> {
        if data.len() != TREASURY_CONFIG_LEN {
            return Err(CodecError::InvalidValue);
        }
        let mut reader = Reader::new(data);
        reader.version()?;
        let value = Self {
            burn_expiry_blocks: reader.u64()?,
            base_burn_incentive: reader.u64()?,
            burn_incentive_rate: reader.u64()?,
            maximum_burn_incentive: reader.u64()?,
            result_type_hash: reader.hash()?,
            zero_lock_hash: reader.hash()?,
        };
        reader.finish()?;
        if value.burn_expiry_blocks == 0
            || value.maximum_burn_incentive < value.base_burn_incentive
            || value.result_type_hash == [0; 32]
            || value.zero_lock_hash == [0; 32]
        {
            return Err(CodecError::InvalidValue);
        }
        Ok(value)
    }

    pub fn burn_incentive(&self, relative_blocks: u64) -> Option<u64> {
        if relative_blocks < self.burn_expiry_blocks {
            return None;
        }
        let delay = relative_blocks - self.burn_expiry_blocks;
        let variable = self.burn_incentive_rate.checked_mul(delay)?;
        Some(
            self.base_burn_incentive
                .checked_add(variable)?
                .min(self.maximum_burn_incentive),
        )
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut output = Vec::with_capacity(TREASURY_CONFIG_LEN);
        output.push(VERSION);
        output.extend_from_slice(&self.burn_expiry_blocks.to_le_bytes());
        output.extend_from_slice(&self.base_burn_incentive.to_le_bytes());
        output.extend_from_slice(&self.burn_incentive_rate.to_le_bytes());
        output.extend_from_slice(&self.maximum_burn_incentive.to_le_bytes());
        output.extend_from_slice(&self.result_type_hash);
        output.extend_from_slice(&self.zero_lock_hash);
        output
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeafTransition {
    pub key: Hash,
    pub old_value: Hash,
    pub new_value: Hash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenTransaction {
    pub block_number: u64,
    pub header_dep_index: u16,
    pub tx_index: u32,
    pub tx_count: u32,
    pub raw_transaction: Vec<u8>,
    pub vote: Option<ProvenVote>,
    pub witnesses_root: Hash,
    pub lemmas: Vec<Hash>,
}

impl ProvenTransaction {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, CodecError> {
        let block_number = reader.u64()?;
        let header_dep_index = reader.u16()?;
        let tx_index = reader.u32()?;
        let tx_count = reader.u32()?;
        let raw_transaction = reader.length_prefixed_bytes()?;
        let vote = decode_optional_vote(reader)?;
        let witnesses_root = reader.hash()?;
        let lemma_count = reader.u16()? as usize;
        let mut lemmas = Vec::with_capacity(lemma_count);
        for _ in 0..lemma_count {
            lemmas.push(reader.hash()?);
        }
        Ok(Self {
            block_number,
            header_dep_index,
            tx_index,
            tx_count,
            raw_transaction,
            vote,
            witnesses_root,
            lemmas,
        })
    }

    fn encode_into(&self, output: &mut Vec<u8>) -> Result<(), CodecError> {
        let raw_len: u32 = self
            .raw_transaction
            .len()
            .try_into()
            .map_err(|_| CodecError::Overflow)?;
        let lemma_count: u16 = self
            .lemmas
            .len()
            .try_into()
            .map_err(|_| CodecError::Overflow)?;
        output.extend_from_slice(&self.block_number.to_le_bytes());
        output.extend_from_slice(&self.header_dep_index.to_le_bytes());
        output.extend_from_slice(&self.tx_index.to_le_bytes());
        output.extend_from_slice(&self.tx_count.to_le_bytes());
        output.extend_from_slice(&raw_len.to_le_bytes());
        output.extend_from_slice(&self.raw_transaction);
        encode_optional_vote(&self.vote, output)?;
        output.extend_from_slice(&self.witnesses_root);
        output.extend_from_slice(&lemma_count.to_le_bytes());
        for lemma in &self.lemmas {
            output.extend_from_slice(lemma);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenVote {
    pub vote_cell: OutPoint,
    pub cell_dep_index: u16,
    pub voter_lock_hash: Hash,
    pub data: VoteData,
}

impl ProvenVote {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, CodecError> {
        Ok(Self {
            vote_cell: OutPoint::decode(reader)?,
            cell_dep_index: reader.u16()?,
            voter_lock_hash: reader.hash()?,
            data: VoteData::decode(&reader.length_prefixed_bytes()?)?,
        })
    }

    fn encode_into(&self, output: &mut Vec<u8>) -> Result<(), CodecError> {
        self.vote_cell.encode_into(output);
        output.extend_from_slice(&self.cell_dep_index.to_le_bytes());
        output.extend_from_slice(&self.voter_lock_hash);
        encode_length_prefixed(&self.data.encode()?, output)
    }
}

fn decode_optional_vote(reader: &mut Reader<'_>) -> Result<Option<ProvenVote>, CodecError> {
    match reader.u8()? {
        0 => Ok(None),
        1 => ProvenVote::decode(reader).map(Some),
        _ => Err(CodecError::InvalidValue),
    }
}

fn encode_optional_vote(vote: &Option<ProvenVote>, output: &mut Vec<u8>) -> Result<(), CodecError> {
    if let Some(vote) = vote {
        output.push(1);
        vote.encode_into(output)
    } else {
        output.push(0);
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenBlockEvent {
    pub tx_index: u32,
    pub raw_transaction: Vec<u8>,
    pub vote: Option<ProvenVote>,
}

impl ProvenBlockEvent {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, CodecError> {
        Ok(Self {
            tx_index: reader.u32()?,
            raw_transaction: Vec::new(),
            vote: decode_optional_vote(reader)?,
        })
    }

    fn encode_into(&self, output: &mut Vec<u8>) -> Result<(), CodecError> {
        if !self.raw_transaction.is_empty() || self.vote.is_none() {
            return Err(CodecError::InvalidValue);
        }
        output.extend_from_slice(&self.tx_index.to_le_bytes());
        encode_optional_vote(&self.vote, output)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenBlock {
    pub block_number: u64,
    pub header_dep_index: u16,
    pub tx_count: u32,
    pub witnesses_root: Hash,
    pub events: Vec<ProvenBlockEvent>,
    pub lemmas: Vec<Hash>,
}

impl ProvenBlock {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, CodecError> {
        let block_number = reader.u64()?;
        let header_dep_index = reader.u16()?;
        let tx_count = reader.u32()?;
        let witnesses_root = reader.hash()?;
        let event_count = reader.u16()? as usize;
        let mut events = Vec::with_capacity(event_count);
        for _ in 0..event_count {
            events.push(ProvenBlockEvent::decode(reader)?);
        }
        let lemma_count = reader.u16()? as usize;
        let mut lemmas = Vec::with_capacity(lemma_count);
        for _ in 0..lemma_count {
            lemmas.push(reader.hash()?);
        }
        Ok(Self {
            block_number,
            header_dep_index,
            tx_count,
            witnesses_root,
            events,
            lemmas,
        })
    }

    fn encode_into(&self, output: &mut Vec<u8>) -> Result<(), CodecError> {
        let event_count: u16 = self
            .events
            .len()
            .try_into()
            .map_err(|_| CodecError::Overflow)?;
        let lemma_count: u16 = self
            .lemmas
            .len()
            .try_into()
            .map_err(|_| CodecError::Overflow)?;
        output.extend_from_slice(&self.block_number.to_le_bytes());
        output.extend_from_slice(&self.header_dep_index.to_le_bytes());
        output.extend_from_slice(&self.tx_count.to_le_bytes());
        output.extend_from_slice(&self.witnesses_root);
        output.extend_from_slice(&event_count.to_le_bytes());
        for event in &self.events {
            event.encode_into(output)?;
        }
        output.extend_from_slice(&lemma_count.to_le_bytes());
        for lemma in &self.lemmas {
            output.extend_from_slice(lemma);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchWitness {
    pub end_block: u64,
    pub end_tx_index: u32,
    pub blocks: Vec<ProvenBlock>,
    pub previous_vote_records: Vec<VoteRecord>,
    pub state_transitions: Vec<LeafTransition>,
    pub state_proof: Vec<u8>,
}

impl BatchWitness {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, CodecError> {
        let end_block = reader.u64()?;
        let end_tx_index = reader.u32()?;
        let block_count = reader.u16()? as usize;
        let mut blocks = Vec::with_capacity(block_count);
        for _ in 0..block_count {
            blocks.push(ProvenBlock::decode(reader)?);
        }

        let record_count = reader.u16()? as usize;
        let mut previous_vote_records = Vec::with_capacity(record_count);
        for _ in 0..record_count {
            previous_vote_records.push(VoteRecord::decode(&reader.length_prefixed_bytes()?)?);
        }
        Ok(Self {
            end_block,
            end_tx_index,
            blocks,
            previous_vote_records,
            state_transitions: decode_transitions(reader)?,
            state_proof: reader.length_prefixed_bytes()?,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>, CodecError> {
        let block_count: u16 = self
            .blocks
            .len()
            .try_into()
            .map_err(|_| CodecError::Overflow)?;
        let record_count: u16 = self
            .previous_vote_records
            .len()
            .try_into()
            .map_err(|_| CodecError::Overflow)?;
        let mut output = Vec::new();
        output.push(TALLY_WITNESS_VERSION);
        output.push(TALLY_ACTION_ADVANCE);
        output.extend_from_slice(&self.end_block.to_le_bytes());
        output.extend_from_slice(&self.end_tx_index.to_le_bytes());
        output.extend_from_slice(&block_count.to_le_bytes());
        for block in &self.blocks {
            block.encode_into(&mut output)?;
        }
        output.extend_from_slice(&record_count.to_le_bytes());
        for record in &self.previous_vote_records {
            encode_length_prefixed(&record.encode()?, &mut output)?;
        }
        encode_transitions(&self.state_transitions, &mut output)?;
        encode_length_prefixed(&self.state_proof, &mut output)?;
        Ok(output)
    }

    pub fn event_count(&self) -> Option<usize> {
        self.blocks
            .iter()
            .try_fold(0usize, |count, block| count.checked_add(block.events.len()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TallyWitness {
    Advance(BatchWitness),
    ChallengeVote {
        omitted: ProvenTransaction,
        event_proof: Vec<u8>,
    },
    Finalize,
    CleanupVetoed,
}

impl TallyWitness {
    pub fn decode(data: &[u8]) -> Result<Self, CodecError> {
        let mut reader = Reader::new(data);
        reader.expected_version(TALLY_WITNESS_VERSION)?;
        let witness = match reader.u8()? {
            TALLY_ACTION_ADVANCE => Self::Advance(BatchWitness::decode(&mut reader)?),
            TALLY_ACTION_CHALLENGE_VOTE => Self::ChallengeVote {
                omitted: ProvenTransaction::decode(&mut reader)?,
                event_proof: reader.length_prefixed_bytes()?,
            },
            TALLY_ACTION_FINALIZE => Self::Finalize,
            TALLY_ACTION_CLEANUP_VETOED => Self::CleanupVetoed,
            _ => return Err(CodecError::InvalidValue),
        };
        reader.finish()?;
        Ok(witness)
    }

    pub fn encode(&self) -> Result<Vec<u8>, CodecError> {
        if let Self::Advance(batch) = self {
            return batch.encode();
        }
        let mut output = Vec::new();
        output.push(TALLY_WITNESS_VERSION);
        match self {
            Self::Advance(_) => unreachable!(),
            Self::ChallengeVote {
                omitted,
                event_proof,
            } => {
                output.push(TALLY_ACTION_CHALLENGE_VOTE);
                omitted.encode_into(&mut output)?;
                encode_length_prefixed(event_proof, &mut output)?;
            }
            Self::Finalize => output.push(TALLY_ACTION_FINALIZE),
            Self::CleanupVetoed => output.push(TALLY_ACTION_CLEANUP_VETOED),
        }
        Ok(output)
    }
}

fn decode_transitions(reader: &mut Reader<'_>) -> Result<Vec<LeafTransition>, CodecError> {
    let count = reader.u16()? as usize;
    let mut transitions = Vec::with_capacity(count);
    for _ in 0..count {
        transitions.push(LeafTransition {
            key: reader.hash()?,
            old_value: reader.hash()?,
            new_value: reader.hash()?,
        });
    }
    Ok(transitions)
}

fn encode_transitions(
    transitions: &[LeafTransition],
    output: &mut Vec<u8>,
) -> Result<(), CodecError> {
    let count: u16 = transitions
        .len()
        .try_into()
        .map_err(|_| CodecError::Overflow)?;
    output.extend_from_slice(&count.to_le_bytes());
    for transition in transitions {
        output.extend_from_slice(&transition.key);
        output.extend_from_slice(&transition.old_value);
        output.extend_from_slice(&transition.new_value);
    }
    Ok(())
}

fn encode_length_prefixed(data: &[u8], output: &mut Vec<u8>) -> Result<(), CodecError> {
    let len: u32 = data.len().try_into().map_err(|_| CodecError::Overflow)?;
    output.extend_from_slice(&len.to_le_bytes());
    output.extend_from_slice(data);
    Ok(())
}

pub fn verify_smt_transition(
    old_root: Hash,
    new_root: Hash,
    proof: &[u8],
    leaves: &[LeafTransition],
) -> bool {
    if leaves.is_empty() || !keys_strictly_ascending(leaves) {
        return false;
    }
    let compiled = CompiledMerkleProof(proof.to_vec());
    let old_leaves = leaves
        .iter()
        .map(|leaf| (H256::from(leaf.key), H256::from(leaf.old_value)))
        .collect();
    let new_leaves = leaves
        .iter()
        .map(|leaf| (H256::from(leaf.key), H256::from(leaf.new_value)))
        .collect();
    compiled
        .verify::<Blake2bHasher>(&H256::from(old_root), old_leaves)
        .ok()
        == Some(true)
        && compiled
            .verify::<Blake2bHasher>(&H256::from(new_root), new_leaves)
            .ok()
            == Some(true)
}

fn keys_strictly_ascending(leaves: &[LeafTransition]) -> bool {
    leaves.windows(2).all(|pair| pair[0].key < pair[1].key)
}

pub struct MergeHash;

impl Merge for MergeHash {
    type Item = Hash;

    fn merge(left: &Self::Item, right: &Self::Item) -> Self::Item {
        hash_pair(left, right)
    }
}

pub fn verify_cbmt_inclusion(
    leaf: Hash,
    tx_index: u32,
    tx_count: u32,
    lemmas: &[Hash],
    expected_root: Hash,
) -> bool {
    if tx_count == 0 || tx_index >= tx_count {
        return false;
    }
    let Some(tree_index) = tx_count
        .checked_sub(1)
        .and_then(|base| base.checked_add(tx_index))
    else {
        return false;
    };
    MerkleProof::<Hash, MergeHash>::new(alloc::vec![tree_index], lemmas.to_vec())
        .verify(&expected_root, &[leaf])
}

pub fn cbmt_multi_root(
    tx_count: u32,
    indexed_leaves: &[(u32, Hash)],
    lemmas: &[Hash],
) -> Option<Hash> {
    if tx_count == 0
        || indexed_leaves.is_empty()
        || indexed_leaves.windows(2).any(|pair| pair[0].0 >= pair[1].0)
        || indexed_leaves.iter().any(|(index, _)| *index >= tx_count)
    {
        return None;
    }
    let base = tx_count.checked_sub(1)?;
    let mut queue = indexed_leaves
        .iter()
        .rev()
        .map(|(index, leaf)| {
            base.checked_add(*index)
                .map(|tree_index| (tree_index, *leaf))
        })
        .collect::<Option<VecDeque<_>>>()?;
    let mut lemmas = lemmas.iter();

    while let Some((index, node)) = queue.pop_front() {
        if index == 0 {
            return if queue.is_empty() && lemmas.next().is_none() {
                Some(node)
            } else {
                None
            };
        }
        let sibling_index = if index & 1 == 1 {
            index.checked_add(1)?
        } else {
            index - 1
        };
        let sibling = if queue
            .front()
            .is_some_and(|(queued_index, _)| *queued_index == sibling_index)
        {
            queue.pop_front().map(|(_, hash)| hash)?
        } else {
            *lemmas.next()?
        };
        let parent = (index - 1) >> 1;
        let parent_hash = if index & 1 == 1 {
            hash_pair(&node, &sibling)
        } else {
            hash_pair(&sibling, &node)
        };
        queue.push_back((parent, parent_hash));
    }
    None
}

pub fn transactions_root(raw_transactions_root: Hash, witnesses_root: Hash) -> Hash {
    hash_pair(&raw_transactions_root, &witnesses_root)
}

pub fn hash_pair(left: &Hash, right: &Hash) -> Hash {
    let mut hasher = new_blake2b();
    let mut output = [0u8; 32];
    hasher.update(left);
    hasher.update(right);
    hasher.finalize(&mut output);
    output
}

pub fn blake2b_256(data: &[u8]) -> Hash {
    let mut hasher = new_blake2b();
    let mut output = [0u8; 32];
    hasher.update(data);
    hasher.finalize(&mut output);
    output
}

struct Reader<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    fn read<const N: usize>(&mut self) -> Result<[u8; N], CodecError> {
        let end = self.offset.checked_add(N).ok_or(CodecError::Overflow)?;
        let bytes = self
            .data
            .get(self.offset..end)
            .ok_or(CodecError::Truncated)?;
        self.offset = end;
        bytes.try_into().map_err(|_| CodecError::Truncated)
    }

    fn version(&mut self) -> Result<(), CodecError> {
        self.expected_version(VERSION)
    }

    fn expected_version(&mut self, version: u8) -> Result<(), CodecError> {
        if self.u8()? == version {
            Ok(())
        } else {
            Err(CodecError::InvalidVersion)
        }
    }

    fn u8(&mut self) -> Result<u8, CodecError> {
        Ok(self.read::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16, CodecError> {
        Ok(u16::from_le_bytes(self.read()?))
    }

    fn u32(&mut self) -> Result<u32, CodecError> {
        Ok(u32::from_le_bytes(self.read()?))
    }

    fn u64(&mut self) -> Result<u64, CodecError> {
        Ok(u64::from_le_bytes(self.read()?))
    }

    fn u128(&mut self) -> Result<u128, CodecError> {
        Ok(u128::from_le_bytes(self.read()?))
    }

    fn hash(&mut self) -> Result<Hash, CodecError> {
        self.read()
    }

    fn length_prefixed_bytes(&mut self) -> Result<Vec<u8>, CodecError> {
        let len = self.u32()? as usize;
        let end = self.offset.checked_add(len).ok_or(CodecError::Overflow)?;
        let bytes = self
            .data
            .get(self.offset..end)
            .ok_or(CodecError::Truncated)?;
        self.offset = end;
        Ok(bytes.to_vec())
    }

    fn finish(&self) -> Result<(), CodecError> {
        if self.offset == self.data.len() {
            Ok(())
        } else {
            Err(CodecError::TrailingBytes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sparse_merkle_tree::{SparseMerkleTree, default_store::DefaultStore, traits::Value};
    extern crate std;

    #[derive(Default, Clone)]
    struct Word(Hash);

    impl Value for Word {
        fn to_h256(&self) -> H256 {
            self.0.into()
        }

        fn zero() -> Self {
            Self::default()
        }
    }

    #[test]
    fn vote_record_round_trip_and_hash_commits_to_vote_position() {
        let record = VoteRecord {
            voter_lock_hash: [1; 32],
            direction: 1,
            amount: 42,
            block_number: 100,
            tx_index: 3,
        };
        let encoded = record.encode().unwrap();
        assert_eq!(VoteRecord::decode(&encoded).unwrap(), record);

        let mut changed = record.clone();
        changed.tx_index = 4;
        assert_ne!(record.value_hash().unwrap(), changed.value_hash().unwrap());
    }

    #[test]
    fn fixed_models_round_trip() {
        let proposal = ProposalData {
            phase: ProposalPhase::Open,
            start_block: 10,
            end_block: 20,
            challenge_period: 5,
            max_events_per_batch: 100,
            max_dao_deps_per_vote: 64,
            max_state_keys_per_batch: 4096,
            max_batch_sequence: 128,
            max_batch_witness_bytes: 500_000,
            minimum_vote_capacity: 1,
            requested_amount: 1000,
            receiver_lock_hash: [1; 32],
            proposer_lock_hash: [4; 32],
            proposal_config_type_hash: [6; 32],
            metadata_hash: [5; 32],
        };
        assert_eq!(proposal.encode().len(), PROPOSAL_DATA_LEN);
        assert_eq!(ProposalData::decode(&proposal.encode()).unwrap(), proposal);

        let tally = TallyState {
            phase: TallyPhase::Active,
            proposal_id: [1; 32],
            operator_lock_hash: [2; 32],
            sequence: 1,
            next_block: 11,
            next_tx_index: 0,
            state_root: [3; 32],
            yes: 6,
            no: 7,
            processed_events: 8,
            candidate_since: 0,
        };
        assert_eq!(tally.encode().len(), TALLY_STATE_LEN);
        assert_eq!(TallyState::decode(&tally.encode()).unwrap(), tally);

        let policy = ProposalConfig {
            approval_bps: 6000,
            minimum_total_votes: 100,
            maximum_proposal_amount: 1_000,
            minimum_challenge_period: 5,
            minimum_tally_bond: 5_000,
            treasury_lock_hash: [6; 32],
            proposal_lock_hash: [7; 32],
            guardian_lock_hash: [15; 32],
            proposal_bond_burn_lock_hash: [16; 32],
            dao_code_hash: [9; 32],
            dao_hash_type: 1,
            proposal_code_hash: [10; 32],
            proposal_hash_type: 1,
            vote_code_hash: [11; 32],
            vote_hash_type: 1,
            tally_code_hash: [12; 32],
            tally_hash_type: 1,
            candidate_lock_hash: [14; 32],
            policy_type_hash: [13; 32],
        };
        assert_eq!(policy.encode().len(), PROPOSAL_CONFIG_LEN);
        assert_eq!(ProposalConfig::decode(&policy.encode()).unwrap(), policy);

        let vetoed = ResultData {
            outcome: ProposalOutcome::Vetoed,
            proposal_id: [1; 32],
            requested_amount: 1_000,
            receiver_lock_hash: [2; 32],
            yes: 0,
            no: 0,
            final_state_hash: [0; 32],
            proposal_config_data_hash: [3; 32],
            veto_reason_hash: [4; 32],
        };
        assert_eq!(vetoed.encode().len(), RESULT_DATA_LEN);
        assert_eq!(ResultData::decode(&vetoed.encode()).unwrap(), vetoed);

        let mut malformed_veto = vetoed.clone();
        malformed_veto.yes = 1;
        assert_eq!(
            ResultData::decode(&malformed_veto.encode()),
            Err(CodecError::InvalidValue)
        );
        let mut unexplained_veto = vetoed.clone();
        unexplained_veto.veto_reason_hash = [0; 32];
        assert_eq!(
            ResultData::decode(&unexplained_veto.encode()),
            Err(CodecError::InvalidValue)
        );

        let treasury = TreasuryConfig {
            burn_expiry_blocks: 100,
            base_burn_incentive: 10,
            burn_incentive_rate: 1,
            maximum_burn_incentive: 20,
            result_type_hash: [7; 32],
            zero_lock_hash: [8; 32],
        };
        assert_eq!(treasury.encode().len(), TREASURY_CONFIG_LEN);
        assert_eq!(
            TreasuryConfig::decode(&treasury.encode()).unwrap(),
            treasury
        );
    }

    #[test]
    fn immutable_configs_reject_unsafe_values() {
        let mut proposal = ProposalData {
            phase: ProposalPhase::Open,
            start_block: 10,
            end_block: 20,
            challenge_period: 5,
            max_events_per_batch: 100,
            max_dao_deps_per_vote: 64,
            max_state_keys_per_batch: 4096,
            max_batch_sequence: 128,
            max_batch_witness_bytes: 500_000,
            minimum_vote_capacity: 1,
            requested_amount: 1000,
            receiver_lock_hash: [1; 32],
            proposer_lock_hash: [4; 32],
            proposal_config_type_hash: [6; 32],
            metadata_hash: [5; 32],
        };
        proposal.proposal_config_type_hash = [0; 32];
        assert_eq!(
            ProposalData::decode(&proposal.encode()),
            Err(CodecError::InvalidValue)
        );

        let invalid_policy = ProposalConfig {
            approval_bps: 6000,
            minimum_total_votes: 0,
            maximum_proposal_amount: 1_000,
            minimum_challenge_period: 5,
            minimum_tally_bond: 5_000,
            treasury_lock_hash: [6; 32],
            proposal_lock_hash: [7; 32],
            guardian_lock_hash: [15; 32],
            proposal_bond_burn_lock_hash: [16; 32],
            dao_code_hash: [9; 32],
            dao_hash_type: 1,
            proposal_code_hash: [10; 32],
            proposal_hash_type: 1,
            vote_code_hash: [11; 32],
            vote_hash_type: 1,
            tally_code_hash: [12; 32],
            tally_hash_type: 1,
            candidate_lock_hash: [14; 32],
            policy_type_hash: [13; 32],
        };
        assert_eq!(
            ProposalConfig::decode(&invalid_policy.encode()),
            Err(CodecError::InvalidValue)
        );
        let invalid_challenge_period = ProposalConfig {
            minimum_total_votes: 1,
            minimum_challenge_period: 0,
            ..invalid_policy
        };
        assert_eq!(
            ProposalConfig::decode(&invalid_challenge_period.encode()),
            Err(CodecError::InvalidValue)
        );
        let invalid_tally_bond = ProposalConfig {
            minimum_total_votes: 1,
            minimum_tally_bond: 0,
            ..invalid_policy
        };
        assert_eq!(
            ProposalConfig::decode(&invalid_tally_bond.encode()),
            Err(CodecError::InvalidValue)
        );

        let invalid_treasury = TreasuryConfig {
            burn_expiry_blocks: 100,
            base_burn_incentive: 20,
            burn_incentive_rate: 1,
            maximum_burn_incentive: 10,
            result_type_hash: [7; 32],
            zero_lock_hash: [8; 32],
        };
        assert_eq!(
            TreasuryConfig::decode(&invalid_treasury.encode()),
            Err(CodecError::InvalidValue)
        );
    }

    #[test]
    fn policy_uses_quorum_amount_cap_and_approval_ratio() {
        let policy = ProposalConfig {
            approval_bps: 6000,
            minimum_total_votes: 100,
            maximum_proposal_amount: 1_000,
            minimum_challenge_period: 5,
            minimum_tally_bond: 5_000,
            treasury_lock_hash: [9; 32],
            proposal_lock_hash: [7; 32],
            guardian_lock_hash: [15; 32],
            proposal_bond_burn_lock_hash: [16; 32],
            dao_code_hash: [8; 32],
            dao_hash_type: 1,
            proposal_code_hash: [10; 32],
            proposal_hash_type: 1,
            vote_code_hash: [11; 32],
            vote_hash_type: 1,
            tally_code_hash: [12; 32],
            tally_hash_type: 1,
            candidate_lock_hash: [14; 32],
            policy_type_hash: [13; 32],
        };
        assert!(policy.passes(60, 40, 1_000));
        assert!(!policy.passes(59, 41, 1_000));
        assert!(!policy.passes(60, 40, 1_001));
        assert!(!policy.passes(59, 40, 1_000));
    }

    #[test]
    fn verifies_old_and_new_smt_roots_with_one_compiled_proof() {
        type Smt = SparseMerkleTree<Blake2bHasher, Word, DefaultStore<Word>>;
        let key: H256 = [1u8; 32].into();
        let old_value = [2u8; 32];
        let new_value = [3u8; 32];
        let mut tree = Smt::default();
        tree.update(key, Word(old_value)).unwrap();
        let old_root: Hash = (*tree.root()).into();
        let proof = tree
            .merkle_proof(alloc::vec![key])
            .unwrap()
            .compile(alloc::vec![key])
            .unwrap();
        tree.update(key, Word(new_value)).unwrap();
        let new_root: Hash = (*tree.root()).into();

        assert!(verify_smt_transition(
            old_root,
            new_root,
            &proof.0,
            &[LeafTransition {
                key: key.into(),
                old_value,
                new_value,
            }],
        ));
    }

    #[test]
    fn verifies_ckb_cbmt_transaction_root_shape() {
        type Tree = merkle_cbt::CBMT<Hash, MergeHash>;
        let leaves = alloc::vec![[1; 32], [2; 32], [3; 32]];
        let raw_root = Tree::build_merkle_root(&leaves);
        let proof = Tree::build_merkle_proof(&leaves, &[1]).unwrap();
        assert!(verify_cbmt_inclusion(
            leaves[1],
            1,
            leaves.len() as u32,
            proof.lemmas(),
            raw_root,
        ));
        assert_eq!(
            transactions_root(raw_root, [9; 32]),
            hash_pair(&raw_root, &[9; 32])
        );
    }

    #[test]
    fn verifies_cbmt_multiproof_with_index_binding() {
        type Tree = merkle_cbt::CBMT<Hash, MergeHash>;
        let leaves = alloc::vec![[1; 32], [2; 32], [3; 32], [4; 32], [5; 32]];
        let root = Tree::build_merkle_root(&leaves);
        let proof = Tree::build_merkle_proof(&leaves, &[1, 3]).unwrap();
        let indexed = alloc::vec![(1, leaves[1]), (3, leaves[3])];
        assert_eq!(
            cbmt_multi_root(leaves.len() as u32, &indexed, proof.lemmas()),
            Some(root)
        );

        let mut modified = indexed.clone();
        modified[0].1[0] ^= 1;
        assert_ne!(
            cbmt_multi_root(leaves.len() as u32, &modified, proof.lemmas()),
            Some(root)
        );
        assert_eq!(
            cbmt_multi_root(
                leaves.len() as u32,
                &[(3, leaves[3]), (1, leaves[1])],
                proof.lemmas()
            ),
            None
        );
        assert_eq!(
            cbmt_multi_root(
                leaves.len() as u32,
                &[(1, leaves[1]), (1, leaves[1])],
                proof.lemmas()
            ),
            None
        );
        assert_eq!(
            cbmt_multi_root(leaves.len() as u32, &indexed, &proof.lemmas()[1..]),
            None
        );
    }

    #[test]
    fn cbmt_multiproof_matches_ckb_tree_shapes() {
        type Tree = merkle_cbt::CBMT<Hash, MergeHash>;
        for leaf_count in 1usize..=32 {
            let leaves = (0..leaf_count)
                .map(|index| blake2b_256(&(index as u64).to_le_bytes()))
                .collect::<Vec<_>>();
            let mut selections = alloc::vec![
                alloc::vec![0u32],
                alloc::vec![(leaf_count - 1) as u32],
                (0..leaf_count as u32).step_by(2).collect(),
                (0..leaf_count as u32).collect(),
            ];
            selections.dedup();
            for indices in selections {
                let proof = Tree::build_merkle_proof(&leaves, &indices).unwrap();
                let indexed = indices
                    .iter()
                    .map(|index| (*index, leaves[*index as usize]))
                    .collect::<Vec<_>>();
                assert_eq!(
                    cbmt_multi_root(leaf_count as u32, &indexed, proof.lemmas()),
                    Some(Tree::build_merkle_root(&leaves)),
                    "leaf_count={leaf_count} indices={indices:?}"
                );
            }
        }
    }

    #[test]
    fn tally_witness_uses_explicit_v7_encoding() {
        let encoded = TallyWitness::Finalize.encode().unwrap();
        assert_eq!(encoded[0], TALLY_WITNESS_VERSION);
        assert_eq!(TallyWitness::decode(&encoded), Ok(TallyWitness::Finalize));
        let cleanup = TallyWitness::CleanupVetoed.encode().unwrap();
        assert_eq!(
            TallyWitness::decode(&cleanup),
            Ok(TallyWitness::CleanupVetoed)
        );

        let mut legacy = encoded;
        legacy[0] = VERSION;
        assert_eq!(
            TallyWitness::decode(&legacy),
            Err(CodecError::InvalidVersion)
        );
        assert_eq!(
            TallyWitness::decode(&[TALLY_WITNESS_VERSION, 2]),
            Err(CodecError::InvalidValue)
        );
    }

    #[test]
    fn state_key_namespaces_are_disjoint() {
        let key = [0xff; 32];
        let vote = namespaced_state_key(VOTE_STATE_NAMESPACE, key).unwrap();
        let event = namespaced_state_key(EVENT_STATE_NAMESPACE, key).unwrap();
        assert_ne!(vote, event);
        assert_eq!(
            vote,
            namespaced_state_key(VOTE_STATE_NAMESPACE, key).unwrap()
        );
        assert!(namespaced_state_key(2, key).is_none());
    }

    #[test]
    #[ignore = "SMT proof size benchmark"]
    fn benchmark_compiled_smt_proof_sizes() {
        type Smt = SparseMerkleTree<Blake2bHasher, Word, DefaultStore<Word>>;

        for existing_leaves in [0u32, 300, 3_000, 30_000] {
            let mut tree = Smt::default();
            for index in 0..existing_leaves {
                tree.update(benchmark_key(index).into(), Word([1; 32]))
                    .unwrap();
            }
            let keys = (existing_leaves..existing_leaves + 200)
                .map(|index| benchmark_key(index).into())
                .collect::<Vec<H256>>();
            let proof = tree
                .merkle_proof(keys.clone())
                .unwrap()
                .compile(keys)
                .unwrap();
            std::println!(
                "existing_leaves={existing_leaves} target_leaves=200 smt_proof_bytes={}",
                proof.0.len()
            );
        }
    }

    fn benchmark_key(index: u32) -> Hash {
        blake2b_256(&index.to_le_bytes())
    }

    #[test]
    fn rejects_noncanonical_smt_transition_keys() {
        type Smt = SparseMerkleTree<Blake2bHasher, Word, DefaultStore<Word>>;
        let first_key: H256 = [1u8; 32].into();
        let second_key: H256 = [2u8; 32].into();
        let old_value = [3u8; 32];
        let new_value = [4u8; 32];
        let mut tree = Smt::default();
        tree.update(first_key, Word(old_value)).unwrap();
        tree.update(second_key, Word(old_value)).unwrap();
        let old_root: Hash = (*tree.root()).into();
        let proof = tree
            .merkle_proof(alloc::vec![first_key, second_key])
            .unwrap()
            .compile(alloc::vec![first_key, second_key])
            .unwrap();
        tree.update(first_key, Word(new_value)).unwrap();
        tree.update(second_key, Word(new_value)).unwrap();
        let new_root: Hash = (*tree.root()).into();
        let sorted = alloc::vec![
            LeafTransition {
                key: first_key.into(),
                old_value,
                new_value,
            },
            LeafTransition {
                key: second_key.into(),
                old_value,
                new_value,
            },
        ];
        assert!(verify_smt_transition(old_root, new_root, &proof.0, &sorted));

        let mut unsorted = sorted.clone();
        unsorted.reverse();
        assert!(!verify_smt_transition(
            old_root, new_root, &proof.0, &unsorted
        ));

        let duplicated = alloc::vec![sorted[0], sorted[0]];
        assert!(!verify_smt_transition(
            old_root,
            new_root,
            &proof.0,
            &duplicated
        ));
    }
}
