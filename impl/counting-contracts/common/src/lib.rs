#![no_std]

extern crate alloc;

use alloc::vec::Vec;
use ckb_hash::new_blake2b;

pub const VERSION: u8 = 3;
pub const PROPOSAL_DATA_LEN: usize = 194;
pub const COUNTING_CELL_DATA_LEN: usize = 24;
pub const COUNTING_CONFIG_LEN: usize = 339;
pub const RESULT_DATA_LEN: usize = 202;

pub type Hash = [u8; 32];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodecError {
    Truncated,
    TrailingBytes,
    InvalidVersion,
    InvalidValue,
    Overflow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct OutPoint {
    pub tx_hash: Hash,
    pub index: u32,
}

impl OutPoint {
    pub const ENCODED_LEN: usize = 36;

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
        if amount == 0 || count == 0 {
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
        if self.direction > 1 || self.amount == 0 || self.dao_out_points.is_empty() {
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

    pub fn encode(&self) -> Result<Vec<u8>, CodecError> {
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
        Self::decode(&output)?;
        Ok(output)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ProposalPhase {
    Open = 0,
    Closed = 1,
    Finalized = 2,
}

impl TryFrom<u8> for ProposalPhase {
    type Error = CodecError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Open),
            1 => Ok(Self::Closed),
            2 => Ok(Self::Finalized),
            _ => Err(CodecError::InvalidValue),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposalData {
    pub phase: ProposalPhase,
    pub start_block: u64,
    pub end_block: u64,
    pub challenge_period: u64,
    pub minimum_vote_capacity: u64,
    pub requested_amount: u64,
    pub yes_amount: u128,
    pub yes_vote_count: u64,
    pub receiver_lock_hash: Hash,
    pub proposer_lock_hash: Hash,
    pub config_type_hash: Hash,
    pub metadata_hash: Hash,
}

impl ProposalData {
    pub fn decode(data: &[u8]) -> Result<Self, CodecError> {
        if data.len() != PROPOSAL_DATA_LEN {
            return Err(CodecError::InvalidValue);
        }
        let mut reader = Reader::new(data);
        reader.version()?;
        let value = Self {
            phase: ProposalPhase::try_from(reader.u8()?)?,
            start_block: reader.u64()?,
            end_block: reader.u64()?,
            challenge_period: reader.u64()?,
            minimum_vote_capacity: reader.u64()?,
            requested_amount: reader.u64()?,
            yes_amount: reader.u128()?,
            yes_vote_count: reader.u64()?,
            receiver_lock_hash: reader.hash()?,
            proposer_lock_hash: reader.hash()?,
            config_type_hash: reader.hash()?,
            metadata_hash: reader.hash()?,
        };
        reader.finish()?;
        let tally_is_zero = value.yes_amount == 0 && value.yes_vote_count == 0;
        if value.start_block >= value.end_block
            || value.challenge_period == 0
            || value.minimum_vote_capacity == 0
            || value.requested_amount == 0
            || value.proposer_lock_hash == [0; 32]
            || value.config_type_hash == [0; 32]
            || (value.phase == ProposalPhase::Finalized) == tally_is_zero
        {
            return Err(CodecError::InvalidValue);
        }
        Ok(value)
    }

    pub fn encode(&self) -> Result<Vec<u8>, CodecError> {
        let mut output = Vec::with_capacity(PROPOSAL_DATA_LEN);
        output.push(VERSION);
        output.push(self.phase as u8);
        output.extend_from_slice(&self.start_block.to_le_bytes());
        output.extend_from_slice(&self.end_block.to_le_bytes());
        output.extend_from_slice(&self.challenge_period.to_le_bytes());
        output.extend_from_slice(&self.minimum_vote_capacity.to_le_bytes());
        output.extend_from_slice(&self.requested_amount.to_le_bytes());
        output.extend_from_slice(&self.yes_amount.to_le_bytes());
        output.extend_from_slice(&self.yes_vote_count.to_le_bytes());
        output.extend_from_slice(&self.receiver_lock_hash);
        output.extend_from_slice(&self.proposer_lock_hash);
        output.extend_from_slice(&self.config_type_hash);
        output.extend_from_slice(&self.metadata_hash);
        Self::decode(&output)?;
        Ok(output)
    }

    pub fn immutable_fields_equal(&self, other: &Self) -> bool {
        let mut lhs = self.clone();
        let mut rhs = other.clone();
        lhs.phase = ProposalPhase::Open;
        rhs.phase = ProposalPhase::Open;
        lhs.yes_amount = 0;
        rhs.yes_amount = 0;
        lhs.yes_vote_count = 0;
        rhs.yes_vote_count = 0;
        lhs == rhs
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CountingCellData {
    pub direction: u8,
    pub range_start: u8,
    pub range_end: u8,
    pub amount: u128,
    pub vote_count: u32,
}

impl CountingCellData {
    pub fn decode(data: &[u8]) -> Result<Self, CodecError> {
        if data.len() != COUNTING_CELL_DATA_LEN {
            return Err(CodecError::InvalidValue);
        }
        let mut reader = Reader::new(data);
        reader.version()?;
        let value = Self {
            direction: reader.u8()?,
            range_start: reader.u8()?,
            range_end: reader.u8()?,
            amount: reader.u128()?,
            vote_count: reader.u32()?,
        };
        reader.finish()?;
        if value.direction > 1
            || value.range_start > value.range_end
            || value.amount == 0
            || value.vote_count == 0
        {
            return Err(CodecError::InvalidValue);
        }
        Ok(value)
    }

    pub fn encode(&self) -> Result<Vec<u8>, CodecError> {
        let mut output = Vec::with_capacity(COUNTING_CELL_DATA_LEN);
        output.push(VERSION);
        output.push(self.direction);
        output.push(self.range_start);
        output.push(self.range_end);
        output.extend_from_slice(&self.amount.to_le_bytes());
        output.extend_from_slice(&self.vote_count.to_le_bytes());
        Self::decode(&output)?;
        Ok(output)
    }

    pub fn contains_lock_hash(&self, lock_hash: &Hash) -> bool {
        self.range_start <= lock_hash[0] && lock_hash[0] <= self.range_end
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CountingConfig {
    pub approval_bps: u16,
    pub minimum_total_votes: u128,
    pub maximum_proposal_amount: u64,
    pub minimum_challenge_period: u64,
    pub max_votes_per_counting_cell: u32,
    pub minimum_proposal_bond: u64,
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
    pub counting_code_hash: Hash,
    pub counting_hash_type: u8,
    pub policy_type_hash: Hash,
}

impl CountingConfig {
    pub fn decode(data: &[u8]) -> Result<Self, CodecError> {
        if data.len() != COUNTING_CONFIG_LEN {
            return Err(CodecError::InvalidValue);
        }
        let mut reader = Reader::new(data);
        reader.version()?;
        let value = Self {
            approval_bps: reader.u16()?,
            minimum_total_votes: reader.u128()?,
            maximum_proposal_amount: reader.u64()?,
            minimum_challenge_period: reader.u64()?,
            max_votes_per_counting_cell: reader.u32()?,
            minimum_proposal_bond: reader.u64()?,
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
            counting_code_hash: reader.hash()?,
            counting_hash_type: reader.u8()?,
            policy_type_hash: reader.hash()?,
        };
        reader.finish()?;
        if value.approval_bps == 0
            || value.approval_bps > 10_000
            || value.minimum_total_votes == 0
            || value.maximum_proposal_amount == 0
            || value.minimum_challenge_period == 0
            || value.max_votes_per_counting_cell == 0
            || value.minimum_proposal_bond == 0
            || value.treasury_lock_hash == [0; 32]
            || value.proposal_lock_hash == [0; 32]
            || value.guardian_lock_hash == [0; 32]
            || value.proposal_bond_burn_lock_hash == [0; 32]
            || value.dao_code_hash == [0; 32]
            || value.proposal_code_hash == [0; 32]
            || value.vote_code_hash == [0; 32]
            || value.counting_code_hash == [0; 32]
            || value.policy_type_hash == [0; 32]
            || !valid_hash_type(value.dao_hash_type)
            || !valid_hash_type(value.proposal_hash_type)
            || !valid_hash_type(value.vote_hash_type)
            || !valid_hash_type(value.counting_hash_type)
        {
            return Err(CodecError::InvalidValue);
        }
        Ok(value)
    }

    pub fn encode(&self) -> Result<Vec<u8>, CodecError> {
        let mut output = Vec::with_capacity(COUNTING_CONFIG_LEN);
        output.push(VERSION);
        output.extend_from_slice(&self.approval_bps.to_le_bytes());
        output.extend_from_slice(&self.minimum_total_votes.to_le_bytes());
        output.extend_from_slice(&self.maximum_proposal_amount.to_le_bytes());
        output.extend_from_slice(&self.minimum_challenge_period.to_le_bytes());
        output.extend_from_slice(&self.max_votes_per_counting_cell.to_le_bytes());
        output.extend_from_slice(&self.minimum_proposal_bond.to_le_bytes());
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
        output.extend_from_slice(&self.counting_code_hash);
        output.push(self.counting_hash_type);
        output.extend_from_slice(&self.policy_type_hash);
        Self::decode(&output)?;
        Ok(output)
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
            && weighted_yes > threshold
    }
}

fn valid_hash_type(value: u8) -> bool {
    matches!(value, 0 | 1 | 2 | 4)
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
        let end = self.offset.checked_add(N).ok_or(CodecError::Truncated)?;
        let bytes = self
            .data
            .get(self.offset..end)
            .ok_or(CodecError::Truncated)?;
        self.offset = end;
        bytes.try_into().map_err(|_| CodecError::Truncated)
    }

    fn version(&mut self) -> Result<(), CodecError> {
        if self.u8()? == VERSION {
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

    fn config() -> CountingConfig {
        CountingConfig {
            approval_bps: 6_000,
            minimum_total_votes: 100,
            maximum_proposal_amount: 1_000,
            minimum_challenge_period: 5,
            max_votes_per_counting_cell: 1_000,
            minimum_proposal_bond: 100,
            treasury_lock_hash: [1; 32],
            proposal_lock_hash: [2; 32],
            guardian_lock_hash: [3; 32],
            proposal_bond_burn_lock_hash: [4; 32],
            dao_code_hash: [5; 32],
            dao_hash_type: 1,
            proposal_code_hash: [6; 32],
            proposal_hash_type: 1,
            vote_code_hash: [7; 32],
            vote_hash_type: 1,
            counting_code_hash: [8; 32],
            counting_hash_type: 1,
            policy_type_hash: [9; 32],
        }
    }

    #[test]
    fn fixed_models_round_trip() {
        let proposal = ProposalData {
            phase: ProposalPhase::Open,
            start_block: 10,
            end_block: 20,
            challenge_period: 5,
            minimum_vote_capacity: 1,
            requested_amount: 1_000,
            yes_amount: 0,
            yes_vote_count: 0,
            receiver_lock_hash: [1; 32],
            proposer_lock_hash: [2; 32],
            config_type_hash: [3; 32],
            metadata_hash: [4; 32],
        };
        assert_eq!(proposal.encode().unwrap().len(), PROPOSAL_DATA_LEN);
        assert_eq!(
            ProposalData::decode(&proposal.encode().unwrap()).unwrap(),
            proposal
        );

        let counting = CountingCellData {
            direction: 1,
            range_start: 0x20,
            range_end: 0x3f,
            amount: 42,
            vote_count: 2,
        };
        assert_eq!(counting.encode().unwrap().len(), COUNTING_CELL_DATA_LEN);
        assert_eq!(
            CountingCellData::decode(&counting.encode().unwrap()).unwrap(),
            counting
        );

        let config = config();
        assert_eq!(config.encode().unwrap().len(), COUNTING_CONFIG_LEN);
        assert_eq!(
            CountingConfig::decode(&config.encode().unwrap()).unwrap(),
            config
        );
    }

    #[test]
    fn finalized_proposal_requires_a_positive_yes_certificate() {
        let mut proposal = ProposalData {
            phase: ProposalPhase::Open,
            start_block: 10,
            end_block: 20,
            challenge_period: 5,
            minimum_vote_capacity: 1,
            requested_amount: 1_000,
            yes_amount: 0,
            yes_vote_count: 0,
            receiver_lock_hash: [1; 32],
            proposer_lock_hash: [2; 32],
            config_type_hash: [3; 32],
            metadata_hash: [4; 32],
        };
        proposal.phase = ProposalPhase::Finalized;
        assert_eq!(proposal.encode(), Err(CodecError::InvalidValue));
        proposal.yes_amount = 100;
        proposal.yes_vote_count = 1;
        assert!(proposal.encode().is_ok());
    }

    #[test]
    fn policy_uses_quorum_ratio_and_amount_cap() {
        let config = config();
        assert!(config.passes(601, 399, 1_000));
        assert!(!config.passes(600, 400, 1_000));
        assert!(!config.passes(60, 40, 1_001));
    }
}
