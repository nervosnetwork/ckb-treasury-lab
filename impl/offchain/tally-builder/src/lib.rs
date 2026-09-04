use std::collections::{BTreeMap, BTreeSet};

use ckb_gen_types::{
    packed::{RawTransaction, Transaction, WitnessArgs},
    prelude::*,
};
use merkle_cbt::CBMT;
use sparse_merkle_tree::{
    H256, SparseMerkleTree, blake2b::Blake2bHasher, default_store::DefaultStore,
};
use treasury_common::{
    BatchWitness, EVENT_PRESENT, EVENT_STATE_NAMESPACE, Hash, LeafTransition, MergeHash, OutPoint,
    ProposalConfig, ProposalData, ProvenBlock, ProvenBlockEvent, ProvenTransaction, ProvenVote,
    TallyPhase, TallyState, VOTE_STATE_NAMESPACE, VoteData, VoteRecord, namespaced_state_key,
};

mod rpc;

pub use rpc::{CkbRpcClient, RpcError};

type Smt = SparseMerkleTree<Blake2bHasher, H256, DefaultStore<H256>>;
const ZERO: Hash = [0; 32];

#[derive(Debug)]
pub enum BuilderError {
    InvalidState,
    InvalidEvent,
    BatchLimit,
    CursorInvalid,
    MissingStateKey,
    TallyOverflow,
    Encoding,
    Smt,
}

#[derive(Debug)]
pub enum ScanError<E> {
    Builder(BuilderError),
    Source(E),
}

impl<E> From<BuilderError> for ScanError<E> {
    fn from(error: BuilderError) -> Self {
        Self::Builder(error)
    }
}

#[derive(Debug)]
pub enum ReplayError<E> {
    Builder(BuilderError),
    Source(E),
}

impl<E> From<BuilderError> for ReplayError<E> {
    fn from(error: BuilderError) -> Self {
        Self::Builder(error)
    }
}

pub trait ChainSource {
    type Error;

    fn block_by_number(&self, block_number: u64) -> Result<ChainBlock, Self::Error>;

    fn submit_transaction(
        &self,
        transaction: ckb_gen_types::packed::Transaction,
    ) -> Result<Hash, Self::Error>;
}

/// Supplies committed transactions needed to reconstruct a tally session from
/// its candidate output back to the initialization transaction.
pub trait TransactionSource {
    type Error;

    fn transaction_by_hash(&self, tx_hash: Hash) -> Result<Transaction, Self::Error>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainBlock {
    pub block_number: u64,
    pub block_hash: Hash,
    pub parent_hash: Hash,
    pub raw_transactions: Vec<Vec<u8>>,
    pub witnesses_root: Hash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScannedBatch {
    pub blocks: Vec<ProvenBlock>,
    pub header_deps: Vec<Hash>,
    pub end_block: u64,
    pub end_tx_index: u32,
    pub candidate_since: u64,
}

pub struct TallyBuilder {
    config: ProposalConfig,
    state: TallyState,
    committed_state: Smt,
    records: BTreeMap<Hash, VoteRecord>,
}

pub struct BlockTransactions {
    pub block_number: u64,
    pub header_dep_index: u16,
    pub raw_transactions: Vec<Vec<u8>>,
    pub witnesses_root: Hash,
}

impl TallyBuilder {
    pub fn new(
        proposal_id: Hash,
        operator_lock_hash: Hash,
        start_block: u64,
        config: ProposalConfig,
    ) -> Self {
        Self {
            config,
            state: TallyState {
                phase: TallyPhase::Active,
                proposal_id,
                operator_lock_hash,
                sequence: 0,
                next_block: start_block,
                next_tx_index: 0,
                state_root: ZERO,
                yes: 0,
                no: 0,
                processed_events: 0,
                candidate_since: 0,
            },
            committed_state: Smt::default(),
            records: BTreeMap::new(),
        }
    }

    pub fn state(&self) -> &TallyState {
        &self.state
    }

    /// Scans one ordered block and returns exactly the vote transactions that
    /// must be included in the next batch.
    pub fn discover_block_events(
        &self,
        proposal: &ProposalData,
        block: &BlockTransactions,
    ) -> Result<Vec<ProvenBlock>, BuilderError> {
        let mut view = self.clone_builder();
        let mut event_indices = Vec::new();
        for (index, raw_bytes) in block.raw_transactions.iter().enumerate() {
            let raw =
                RawTransaction::from_slice(raw_bytes).map_err(|_| BuilderError::InvalidEvent)?;
            if !view.is_relevant(&raw)? {
                continue;
            }
            let tx_index = index.try_into().map_err(|_| BuilderError::InvalidEvent)?;
            let mut vote_keys = BTreeSet::new();
            let mut state_keys = BTreeSet::new();
            view.apply_event(
                proposal,
                block.block_number,
                tx_index,
                raw_bytes,
                &mut vote_keys,
                &mut state_keys,
            )?;
            event_indices.push(tx_index);
        }
        if event_indices.is_empty() {
            Ok(Vec::new())
        } else {
            Ok(vec![prove_block_transactions(
                block.block_number,
                block.header_dep_index,
                &block.raw_transactions,
                block.witnesses_root,
                &event_indices,
            )?])
        }
    }

    /// Scans consecutive canonical blocks while carrying the temporary reducer
    /// state across block boundaries. The returned header hashes must be added
    /// to the batch transaction in exactly this order.
    pub fn scan_blocks(
        &self,
        proposal: &ProposalData,
        blocks: &[ChainBlock],
    ) -> Result<ScannedBatch, BuilderError> {
        if self.state.phase != TallyPhase::Active || blocks.is_empty() {
            return Err(BuilderError::InvalidState);
        }
        let mut expected_number = self.state.next_block;
        let mut previous_hash = None;
        for block in blocks {
            if block.block_number != expected_number || block.block_number > proposal.end_block {
                return Err(BuilderError::CursorInvalid);
            }
            if previous_hash.is_some_and(|hash| block.parent_hash != hash) {
                return Err(BuilderError::CursorInvalid);
            }
            expected_number = expected_number
                .checked_add(1)
                .ok_or(BuilderError::CursorInvalid)?;
            previous_hash = Some(block.block_hash);
        }

        let mut view = self.clone_builder();
        let mut proven_blocks = Vec::new();
        let mut event_count = 0usize;
        let mut header_deps = Vec::new();
        for (block_offset, block) in blocks.iter().enumerate() {
            let start_index = if block_offset == 0 {
                self.state.next_tx_index as usize
            } else {
                0
            };
            if start_index > block.raw_transactions.len() {
                return Err(BuilderError::CursorInvalid);
            }
            let mut event_indices = Vec::new();
            let mut block_header_dep_index = None;
            for (index, raw_bytes) in block.raw_transactions.iter().enumerate().skip(start_index) {
                let raw = RawTransaction::from_slice(raw_bytes)
                    .map_err(|_| BuilderError::InvalidEvent)?;
                if !view.is_relevant(&raw)? {
                    continue;
                }
                if event_count == proposal.max_events_per_batch as usize {
                    if !event_indices.is_empty() {
                        proven_blocks.push(prove_block_transactions(
                            block.block_number,
                            block_header_dep_index.ok_or(BuilderError::InvalidState)?,
                            &block.raw_transactions,
                            block.witnesses_root,
                            &event_indices,
                        )?);
                    }
                    return Ok(ScannedBatch {
                        blocks: proven_blocks,
                        header_deps,
                        end_block: block.block_number,
                        end_tx_index: index.try_into().map_err(|_| BuilderError::CursorInvalid)?,
                        candidate_since: 0,
                    });
                }
                if block_header_dep_index.is_none() {
                    block_header_dep_index =
                        Some(header_dep_index(&mut header_deps, block.block_hash)?);
                }
                let tx_index = index.try_into().map_err(|_| BuilderError::InvalidEvent)?;
                let mut vote_keys = BTreeSet::new();
                let mut state_keys = BTreeSet::new();
                view.apply_event(
                    proposal,
                    block.block_number,
                    tx_index,
                    raw_bytes,
                    &mut vote_keys,
                    &mut state_keys,
                )?;
                event_indices.push(tx_index);
                event_count += 1;
            }
            if !event_indices.is_empty() {
                proven_blocks.push(prove_block_transactions(
                    block.block_number,
                    block_header_dep_index.ok_or(BuilderError::InvalidState)?,
                    &block.raw_transactions,
                    block.witnesses_root,
                    &event_indices,
                )?);
            }
        }

        let last = blocks.last().ok_or(BuilderError::InvalidState)?;
        let end_block = last
            .block_number
            .checked_add(1)
            .ok_or(BuilderError::CursorInvalid)?;
        let final_block = proposal
            .end_block
            .checked_add(1)
            .ok_or(BuilderError::CursorInvalid)?;
        let candidate_since = if end_block == final_block {
            header_dep_index(&mut header_deps, last.block_hash)?;
            proposal.end_block
        } else {
            0
        };
        Ok(ScannedBatch {
            blocks: proven_blocks,
            header_deps,
            end_block,
            end_tx_index: 0,
            candidate_since,
        })
    }

    pub fn scan_source_batch<S: ChainSource>(
        &self,
        proposal: &ProposalData,
        source: &S,
        through_block: u64,
    ) -> Result<ScannedBatch, ScanError<S::Error>> {
        if through_block < self.state.next_block || through_block > proposal.end_block {
            return Err(BuilderError::CursorInvalid.into());
        }
        let mut blocks = Vec::new();
        for block_number in self.state.next_block..=through_block {
            blocks.push(
                source
                    .block_by_number(block_number)
                    .map_err(ScanError::Source)?,
            );
        }
        self.scan_blocks(proposal, &blocks).map_err(Into::into)
    }

    pub fn build_batch(
        &mut self,
        proposal: &ProposalData,
        mut proven_blocks: Vec<ProvenBlock>,
        end_block: u64,
        end_tx_index: u32,
        candidate_since: u64,
    ) -> Result<(TallyState, BatchWitness), BuilderError> {
        let event_count = proven_blocks
            .iter()
            .try_fold(0usize, |count, block| count.checked_add(block.events.len()));
        let event_count = event_count.ok_or(BuilderError::BatchLimit)?;
        if self.state.phase != TallyPhase::Active
            || event_count > proposal.max_events_per_batch as usize
        {
            return Err(BuilderError::BatchLimit);
        }
        let final_cursor = (
            proposal
                .end_block
                .checked_add(1)
                .ok_or(BuilderError::CursorInvalid)?,
            0,
        );
        let input_cursor = (self.state.next_block, self.state.next_tx_index);
        let output_cursor = (end_block, end_tx_index);
        if output_cursor <= input_cursor || output_cursor > final_cursor {
            return Err(BuilderError::CursorInvalid);
        }

        let mut next = self.clone_builder();
        let mut vote_keys = BTreeSet::new();
        let mut state_keys = BTreeSet::new();
        let mut previous_cursor = None;
        let mut previous_block = None;
        for block in &mut proven_blocks {
            if block.events.is_empty()
                || previous_block.is_some_and(|number| number >= block.block_number)
            {
                return Err(BuilderError::InvalidEvent);
            }
            previous_block = Some(block.block_number);
            let mut previous_tx_index = None;
            for event in &mut block.events {
                let cursor = (block.block_number, event.tx_index);
                if cursor < input_cursor
                    || cursor >= output_cursor
                    || block.block_number < proposal.start_block
                    || block.block_number > proposal.end_block
                    || event.tx_index >= block.tx_count
                    || previous_tx_index.is_some_and(|index| index >= event.tx_index)
                    || previous_cursor.is_some_and(|previous| cursor <= previous)
                {
                    return Err(BuilderError::InvalidEvent);
                }
                previous_tx_index = Some(event.tx_index);
                previous_cursor = Some(cursor);
                next.apply_proven_event(
                    proposal,
                    block.block_number,
                    event,
                    &mut vote_keys,
                    &mut state_keys,
                )?;
            }
        }
        if state_keys.len() > proposal.max_state_keys_per_batch as usize {
            return Err(BuilderError::BatchLimit);
        }

        let (state_transitions, state_proof, previous_vote_records) = if event_count == 0 {
            (Vec::new(), Vec::new(), Vec::new())
        } else {
            let state_transitions =
                transitions(&self.committed_state, &next.committed_state, &state_keys)?;
            let state_proof = compiled_proof(&self.committed_state, &state_keys)?;
            let previous_vote_records = vote_keys
                .iter()
                .map(|voter| {
                    if value(
                        &self.committed_state,
                        state_key(VOTE_STATE_NAMESPACE, *voter),
                    )? == ZERO
                    {
                        Ok(None)
                    } else {
                        self.records
                            .get(voter)
                            .cloned()
                            .map(Some)
                            .ok_or(BuilderError::MissingStateKey)
                    }
                })
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect();
            (state_transitions, state_proof, previous_vote_records)
        };

        next.state.phase = if output_cursor == final_cursor {
            TallyPhase::Candidate
        } else {
            TallyPhase::Active
        };
        next.state.sequence = next
            .state
            .sequence
            .checked_add(1)
            .ok_or(BuilderError::TallyOverflow)?;
        if next.state.sequence > proposal.max_batch_sequence as u32 {
            return Err(BuilderError::BatchLimit);
        }
        next.state.next_block = end_block;
        next.state.next_tx_index = end_tx_index;
        next.state.processed_events = next
            .state
            .processed_events
            .checked_add(event_count as u64)
            .ok_or(BuilderError::TallyOverflow)?;
        let committed_root = root(&next.committed_state);
        next.state.state_root = committed_root;
        next.state.candidate_since = if next.state.phase == TallyPhase::Candidate {
            if candidate_since < proposal.end_block {
                return Err(BuilderError::CursorInvalid);
            }
            candidate_since
        } else {
            0
        };

        let witness = BatchWitness {
            end_block,
            end_tx_index,
            blocks: proven_blocks,
            previous_vote_records,
            state_transitions,
            state_proof,
        };
        if witness.encode().map_err(|_| BuilderError::Encoding)?.len()
            > proposal.max_batch_witness_bytes as usize
        {
            return Err(BuilderError::BatchLimit);
        }
        let output_state = next.state.clone();
        *self = next;
        Ok((output_state, witness))
    }

    /// Reconstructs the complete SMT behind a candidate using only committed
    /// tally transactions and their on-chain advance witnesses.
    pub fn replay_candidate<S: TransactionSource>(
        proposal_id: Hash,
        proposal: &ProposalData,
        config: ProposalConfig,
        source: &S,
        candidate_out_point: OutPoint,
    ) -> Result<Self, ReplayError<S::Error>> {
        let mut current_transaction = load_source_transaction(source, candidate_out_point.tx_hash)?;
        let (tally_type, candidate_state) =
            tally_output(&current_transaction, candidate_out_point)?;
        if tally_type.code_hash().as_slice() != config.tally_code_hash
            || tally_type.hash_type().as_slice()[0] != config.tally_hash_type
            || candidate_state.phase != TallyPhase::Candidate
            || candidate_state.proposal_id != proposal_id
            || candidate_state.sequence == 0
            || candidate_state.sequence > proposal.max_batch_sequence as u32
        {
            return Err(BuilderError::InvalidState.into());
        }

        let mut current_state = candidate_state.clone();
        let mut reversed_steps = Vec::with_capacity(current_state.sequence as usize);
        while current_state.sequence != 0 {
            let mut tally_input = None;
            for (input_index, input) in current_transaction.raw().inputs().into_iter().enumerate() {
                let previous_out_point = unpack_out_point(&input.previous_output());
                let previous_transaction =
                    load_source_transaction(source, previous_out_point.tx_hash)?;
                let Ok((previous_type, previous_state)) =
                    tally_output(&previous_transaction, previous_out_point)
                else {
                    continue;
                };
                if previous_type != tally_type {
                    continue;
                }
                if tally_input.is_some() {
                    return Err(BuilderError::InvalidState.into());
                }
                let batch = advance_witness(&current_transaction, input_index)?;
                tally_input = Some((previous_transaction, previous_state, batch));
            }
            let (previous_transaction, previous_state, batch) =
                tally_input.ok_or(BuilderError::InvalidState)?;
            if previous_state.sequence.checked_add(1) != Some(current_state.sequence) {
                return Err(BuilderError::InvalidState.into());
            }
            reversed_steps.push((previous_state.clone(), current_state, batch));
            current_transaction = previous_transaction;
            current_state = previous_state;
        }

        let mut replayed = Self::new(
            candidate_state.proposal_id,
            current_state.operator_lock_hash,
            proposal.start_block,
            config,
        );
        if replayed.state != current_state {
            return Err(BuilderError::InvalidState.into());
        }
        for (input_state, expected_output, batch) in reversed_steps.into_iter().rev() {
            if replayed.state != input_state {
                return Err(BuilderError::InvalidState.into());
            }
            let (actual_output, _) = replayed.build_batch(
                proposal,
                batch.blocks.clone(),
                batch.end_block,
                batch.end_tx_index,
                expected_output.candidate_since,
            )?;
            if actual_output != expected_output {
                return Err(BuilderError::InvalidState.into());
            }
        }
        if replayed.state != candidate_state {
            return Err(BuilderError::InvalidState.into());
        }
        Ok(replayed)
    }

    pub fn build_omitted_vote_challenge_from_candidate<S: TransactionSource>(
        proposal_id: Hash,
        proposal: &ProposalData,
        config: ProposalConfig,
        source: &S,
        candidate_out_point: OutPoint,
        omitted: ProvenTransaction,
    ) -> Result<treasury_common::TallyWitness, ReplayError<S::Error>> {
        Self::replay_candidate(proposal_id, proposal, config, source, candidate_out_point)?
            .build_omitted_vote_challenge(omitted)
            .map_err(Into::into)
    }

    pub fn build_omitted_vote_challenge(
        &self,
        mut omitted: ProvenTransaction,
    ) -> Result<treasury_common::TallyWitness, BuilderError> {
        let raw = RawTransaction::from_slice(&omitted.raw_transaction)
            .map_err(|_| BuilderError::InvalidEvent)?;
        let tx_hash: Hash = raw.calc_tx_hash().as_slice().try_into().unwrap();
        omitted.vote = Some(self.extract_vote(&raw, tx_hash)?);
        omitted.raw_transaction.clear();
        let event_key = state_key(EVENT_STATE_NAMESPACE, tx_hash);
        if value(&self.committed_state, event_key)? != ZERO {
            return Err(BuilderError::InvalidEvent);
        }
        let keys = BTreeSet::from([event_key]);
        Ok(treasury_common::TallyWitness::ChallengeVote {
            omitted,
            event_proof: compiled_proof(&self.committed_state, &keys)?,
        })
    }

    fn apply_event(
        &mut self,
        proposal: &ProposalData,
        block_number: u64,
        tx_index: u32,
        raw_transaction: &[u8],
        vote_keys: &mut BTreeSet<Hash>,
        state_keys: &mut BTreeSet<Hash>,
    ) -> Result<(), BuilderError> {
        let mut event = ProvenBlockEvent {
            tx_index,
            raw_transaction: raw_transaction.to_vec(),
            vote: None,
        };
        self.apply_proven_event(proposal, block_number, &mut event, vote_keys, state_keys)
    }

    fn apply_proven_event(
        &mut self,
        proposal: &ProposalData,
        block_number: u64,
        event: &mut ProvenBlockEvent,
        vote_keys: &mut BTreeSet<Hash>,
        state_keys: &mut BTreeSet<Hash>,
    ) -> Result<(), BuilderError> {
        let raw = if event.raw_transaction.is_empty() {
            None
        } else {
            Some(
                RawTransaction::from_slice(&event.raw_transaction)
                    .map_err(|_| BuilderError::InvalidEvent)?,
            )
        };
        let raw_hash = raw
            .as_ref()
            .map(|raw| raw.calc_tx_hash().as_slice().try_into().unwrap());
        if event.vote.is_none()
            && let (Some(raw), Some(tx_hash)) = (raw.as_ref(), raw_hash)
        {
            event.vote = self.extract_vote_optional(raw, tx_hash)?;
        }
        let tx_hash = match (raw_hash, event.vote.as_ref()) {
            (Some(raw_hash), Some(vote)) if raw_hash != vote.vote_cell.tx_hash => {
                return Err(BuilderError::InvalidEvent);
            }
            (Some(raw_hash), _) => raw_hash,
            (None, Some(vote)) => vote.vote_cell.tx_hash,
            (None, None) => return Err(BuilderError::InvalidEvent),
        };
        let vote = event.vote.as_ref().ok_or(BuilderError::InvalidEvent)?;
        let event_key = state_key(EVENT_STATE_NAMESPACE, tx_hash);
        if value(&self.committed_state, event_key)? != ZERO {
            return Err(BuilderError::InvalidEvent);
        }
        state_keys.insert(event_key);
        update(&mut self.committed_state, event_key, EVENT_PRESENT)?;

        if vote.data.dao_out_points.len() > proposal.max_dao_deps_per_vote as usize {
            return Err(BuilderError::BatchLimit);
        }
        self.apply_record(
            VoteRecord {
                voter_lock_hash: vote.voter_lock_hash,
                direction: vote.data.direction,
                amount: vote.data.amount,
                block_number,
                tx_index: event.tx_index,
            },
            vote_keys,
            state_keys,
        )?;
        event.raw_transaction.clear();
        Ok(())
    }

    fn extract_vote_optional(
        &self,
        raw: &RawTransaction,
        tx_hash: Hash,
    ) -> Result<Option<ProvenVote>, BuilderError> {
        let mut found = None;
        for (index, output) in raw.outputs().into_iter().enumerate() {
            let Some(type_script) = output.type_().to_opt() else {
                continue;
            };
            if type_script.code_hash().as_slice() != self.config.vote_code_hash
                || type_script.hash_type().as_slice()[0] != self.config.vote_hash_type
                || type_script.args().raw_data().as_ref() != self.state.proposal_id
            {
                continue;
            }
            if found.is_some() {
                return Err(BuilderError::InvalidEvent);
            }
            let data = raw
                .outputs_data()
                .get(index)
                .ok_or(BuilderError::InvalidEvent)?
                .raw_data();
            found = Some(ProvenVote {
                vote_cell: OutPoint {
                    tx_hash,
                    index: index.try_into().map_err(|_| BuilderError::InvalidEvent)?,
                },
                cell_dep_index: 0,
                voter_lock_hash: output
                    .lock()
                    .calc_script_hash()
                    .as_slice()
                    .try_into()
                    .unwrap(),
                data: VoteData::decode(&data).map_err(|_| BuilderError::InvalidEvent)?,
            });
        }
        Ok(found)
    }

    fn extract_vote(
        &self,
        raw: &RawTransaction,
        tx_hash: Hash,
    ) -> Result<ProvenVote, BuilderError> {
        self.extract_vote_optional(raw, tx_hash)?
            .ok_or(BuilderError::InvalidEvent)
    }

    fn is_relevant(&self, raw: &RawTransaction) -> Result<bool, BuilderError> {
        Ok(raw.outputs().into_iter().any(|output| {
            output.type_().to_opt().is_some_and(|script| {
                script.code_hash().as_slice() == self.config.vote_code_hash
                    && script.hash_type().as_slice()[0] == self.config.vote_hash_type
                    && script.args().raw_data().as_ref() == self.state.proposal_id
            })
        }))
    }

    fn remove_record(
        &mut self,
        voter: Hash,
        vote_keys: &mut BTreeSet<Hash>,
        state_keys: &mut BTreeSet<Hash>,
    ) -> Result<(), BuilderError> {
        vote_keys.insert(voter);
        let vote_key = state_key(VOTE_STATE_NAMESPACE, voter);
        state_keys.insert(vote_key);
        let record = self
            .records
            .remove(&voter)
            .ok_or(BuilderError::MissingStateKey)?;
        update(&mut self.committed_state, vote_key, ZERO)?;
        self.subtract_tally(record.direction, record.amount)?;
        Ok(())
    }

    fn apply_record(
        &mut self,
        record: VoteRecord,
        vote_keys: &mut BTreeSet<Hash>,
        state_keys: &mut BTreeSet<Hash>,
    ) -> Result<(), BuilderError> {
        let voter = record.voter_lock_hash;
        vote_keys.insert(voter);
        let vote_key = state_key(VOTE_STATE_NAMESPACE, voter);
        state_keys.insert(vote_key);
        if value(&self.committed_state, vote_key)? != ZERO {
            self.remove_record(voter, vote_keys, state_keys)?;
        }
        self.add_tally(record.direction, record.amount)?;
        let record_hash = record.value_hash().map_err(|_| BuilderError::Encoding)?;
        update(&mut self.committed_state, vote_key, record_hash)?;
        self.records.insert(voter, record);
        Ok(())
    }

    fn add_tally(&mut self, direction: u8, amount: u64) -> Result<(), BuilderError> {
        let target = if direction == 1 {
            &mut self.state.yes
        } else {
            &mut self.state.no
        };
        *target = target
            .checked_add(amount as u128)
            .ok_or(BuilderError::TallyOverflow)?;
        Ok(())
    }

    fn subtract_tally(&mut self, direction: u8, amount: u64) -> Result<(), BuilderError> {
        let target = if direction == 1 {
            &mut self.state.yes
        } else {
            &mut self.state.no
        };
        *target = target
            .checked_sub(amount as u128)
            .ok_or(BuilderError::InvalidState)?;
        Ok(())
    }

    fn clone_builder(&self) -> Self {
        Self {
            config: self.config,
            state: self.state.clone(),
            committed_state: clone_tree(&self.committed_state),
            records: self.records.clone(),
        }
    }
}

fn load_source_transaction<S: TransactionSource>(
    source: &S,
    expected_hash: Hash,
) -> Result<Transaction, ReplayError<S::Error>> {
    let transaction = source
        .transaction_by_hash(expected_hash)
        .map_err(ReplayError::Source)?;
    let actual_hash: Hash = transaction
        .raw()
        .calc_tx_hash()
        .as_slice()
        .try_into()
        .unwrap();
    if actual_hash != expected_hash {
        return Err(BuilderError::InvalidState.into());
    }
    Ok(transaction)
}

fn tally_output(
    transaction: &Transaction,
    out_point: OutPoint,
) -> Result<(ckb_gen_types::packed::Script, TallyState), BuilderError> {
    let index: usize = out_point
        .index
        .try_into()
        .map_err(|_| BuilderError::InvalidState)?;
    let output = transaction
        .raw()
        .outputs()
        .get(index)
        .ok_or(BuilderError::InvalidState)?;
    let tally_type = output.type_().to_opt().ok_or(BuilderError::InvalidState)?;
    let data = transaction
        .raw()
        .outputs_data()
        .get(index)
        .ok_or(BuilderError::InvalidState)?
        .raw_data();
    let state = TallyState::decode(&data).map_err(|_| BuilderError::InvalidState)?;
    Ok((tally_type, state))
}

fn advance_witness(
    transaction: &Transaction,
    input_index: usize,
) -> Result<BatchWitness, BuilderError> {
    let witness = transaction
        .witnesses()
        .get(input_index)
        .ok_or(BuilderError::Encoding)?;
    let witness_args =
        WitnessArgs::from_slice(&witness.raw_data()).map_err(|_| BuilderError::Encoding)?;
    let input_type = witness_args
        .input_type()
        .to_opt()
        .ok_or(BuilderError::Encoding)?
        .raw_data();
    match treasury_common::TallyWitness::decode(&input_type).map_err(|_| BuilderError::Encoding)? {
        treasury_common::TallyWitness::Advance(batch) => Ok(batch),
        _ => Err(BuilderError::Encoding),
    }
}

pub fn prove_block_transactions(
    block_number: u64,
    header_dep_index: u16,
    raw_transactions: &[Vec<u8>],
    witnesses_root: Hash,
    tx_indices: &[u32],
) -> Result<ProvenBlock, BuilderError> {
    if raw_transactions.is_empty()
        || tx_indices.is_empty()
        || tx_indices.windows(2).any(|pair| pair[0] >= pair[1])
        || tx_indices
            .iter()
            .any(|index| *index as usize >= raw_transactions.len())
    {
        return Err(BuilderError::InvalidEvent);
    }
    let hashes = raw_transactions
        .iter()
        .map(|raw| {
            RawTransaction::from_slice(raw)
                .map(|tx| tx.calc_tx_hash().as_slice().try_into().unwrap())
                .map_err(|_| BuilderError::InvalidEvent)
        })
        .collect::<Result<Vec<Hash>, _>>()?;
    let proof = CBMT::<Hash, MergeHash>::build_merkle_proof(&hashes, tx_indices)
        .ok_or(BuilderError::InvalidEvent)?;
    let events = tx_indices
        .iter()
        .map(|index| ProvenBlockEvent {
            tx_index: *index,
            raw_transaction: raw_transactions[*index as usize].clone(),
            vote: None,
        })
        .collect();
    Ok(ProvenBlock {
        block_number,
        header_dep_index,
        tx_count: hashes
            .len()
            .try_into()
            .map_err(|_| BuilderError::BatchLimit)?,
        witnesses_root,
        events,
        lemmas: proof.lemmas().to_vec(),
    })
}

pub fn prove_transaction(
    block_number: u64,
    header_dep_index: u16,
    raw_transactions: &[Vec<u8>],
    witnesses_root: Hash,
    tx_index: u32,
) -> Result<ProvenTransaction, BuilderError> {
    if tx_index as usize >= raw_transactions.len() || raw_transactions.is_empty() {
        return Err(BuilderError::InvalidEvent);
    }
    let hashes = raw_transactions
        .iter()
        .map(|raw| {
            RawTransaction::from_slice(raw)
                .map(|tx| tx.calc_tx_hash().as_slice().try_into().unwrap())
                .map_err(|_| BuilderError::InvalidEvent)
        })
        .collect::<Result<Vec<Hash>, _>>()?;
    let proof = CBMT::<Hash, MergeHash>::build_merkle_proof(&hashes, &[tx_index])
        .ok_or(BuilderError::InvalidEvent)?;
    Ok(ProvenTransaction {
        block_number,
        header_dep_index,
        tx_index,
        tx_count: hashes.len() as u32,
        raw_transaction: raw_transactions[tx_index as usize].clone(),
        vote: None,
        witnesses_root,
        lemmas: proof.lemmas().to_vec(),
    })
}

fn transitions(
    old: &Smt,
    new: &Smt,
    keys: &BTreeSet<Hash>,
) -> Result<Vec<LeafTransition>, BuilderError> {
    keys.iter()
        .map(|key| {
            Ok(LeafTransition {
                key: *key,
                old_value: value(old, *key)?,
                new_value: value(new, *key)?,
            })
        })
        .collect()
}

fn compiled_proof(tree: &Smt, keys: &BTreeSet<Hash>) -> Result<Vec<u8>, BuilderError> {
    if keys.is_empty() {
        return Err(BuilderError::MissingStateKey);
    }
    let keys = keys.iter().copied().map(H256::from).collect::<Vec<_>>();
    tree.merkle_proof(keys.clone())
        .and_then(|proof| proof.compile(keys))
        .map(|proof| proof.0)
        .map_err(|_| BuilderError::Smt)
}

fn value(tree: &Smt, key: Hash) -> Result<Hash, BuilderError> {
    tree.get(&H256::from(key))
        .map(Into::into)
        .map_err(|_| BuilderError::Smt)
}

fn update(tree: &mut Smt, key: Hash, value: Hash) -> Result<(), BuilderError> {
    tree.update(H256::from(key), H256::from(value))
        .map(|_| ())
        .map_err(|_| BuilderError::Smt)
}

fn root(tree: &Smt) -> Hash {
    (*tree.root()).into()
}

fn state_key(namespace: u8, key: Hash) -> Hash {
    namespaced_state_key(namespace, key).expect("fixed state namespace")
}

fn clone_tree(tree: &Smt) -> Smt {
    Smt::new(*tree.root(), tree.store().clone())
}

fn unpack_out_point(out_point: &ckb_gen_types::packed::OutPoint) -> OutPoint {
    OutPoint {
        tx_hash: out_point.tx_hash().as_slice().try_into().unwrap(),
        index: out_point.index().unpack(),
    }
}

fn header_dep_index(header_deps: &mut Vec<Hash>, block_hash: Hash) -> Result<u16, BuilderError> {
    if let Some(index) = header_deps.iter().position(|hash| *hash == block_hash) {
        return index.try_into().map_err(|_| BuilderError::BatchLimit);
    }
    let index = header_deps
        .len()
        .try_into()
        .map_err(|_| BuilderError::BatchLimit)?;
    header_deps.push(block_hash);
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_gen_types::bytes::Bytes;
    use ckb_gen_types::packed;
    use treasury_common::{
        ProposalPhase, cbmt_multi_root, verify_cbmt_inclusion, verify_smt_transition,
    };

    #[derive(Default)]
    struct MemoryTransactionSource {
        transactions: BTreeMap<Hash, Transaction>,
    }

    impl MemoryTransactionSource {
        fn insert(&mut self, transaction: Transaction) -> OutPoint {
            let tx_hash = transaction
                .raw()
                .calc_tx_hash()
                .as_slice()
                .try_into()
                .unwrap();
            self.transactions.insert(tx_hash, transaction);
            OutPoint { tx_hash, index: 0 }
        }
    }

    impl TransactionSource for MemoryTransactionSource {
        type Error = ();

        fn transaction_by_hash(&self, tx_hash: Hash) -> Result<Transaction, Self::Error> {
            self.transactions.get(&tx_hash).cloned().ok_or(())
        }
    }

    fn proposal() -> ProposalData {
        ProposalData {
            phase: ProposalPhase::Closed,
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
            proposer_lock_hash: [7; 32],
            proposal_config_type_hash: [6; 32],
            metadata_hash: [5; 32],
        }
    }

    fn proposal_config() -> ProposalConfig {
        ProposalConfig {
            approval_bps: 6_000,
            minimum_total_votes: 1,
            maximum_proposal_amount: 1_000,
            minimum_challenge_period: 5,
            minimum_tally_bond: 5_000,
            treasury_lock_hash: [1; 32],
            proposal_lock_hash: [6; 32],
            guardian_lock_hash: [7; 32],
            proposal_bond_burn_lock_hash: [8; 32],
            dao_code_hash: [9; 32],
            dao_hash_type: 1,
            proposal_code_hash: [10; 32],
            proposal_hash_type: 1,
            vote_code_hash: [2; 32],
            vote_hash_type: 1,
            tally_code_hash: [3; 32],
            tally_hash_type: 1,
            candidate_lock_hash: [5; 32],
            policy_type_hash: [4; 32],
        }
    }

    fn raw_transaction(nonce: u32) -> Vec<u8> {
        packed::RawTransaction::new_builder()
            .version(nonce)
            .build()
            .as_slice()
            .to_vec()
    }

    fn packed_out_point(out_point: OutPoint) -> packed::OutPoint {
        packed::OutPoint::new_builder()
            .tx_hash(out_point.tx_hash.pack())
            .index(out_point.index)
            .build()
    }

    fn tally_transaction(
        inputs: &[OutPoint],
        tally_type: &packed::Script,
        state: &TallyState,
        advance_input_index: Option<(usize, BatchWitness)>,
    ) -> Transaction {
        let raw = packed::RawTransaction::new_builder()
            .inputs(
                inputs
                    .iter()
                    .map(|out_point| {
                        packed::CellInput::new_builder()
                            .previous_output(packed_out_point(*out_point))
                            .build()
                    })
                    .collect::<Vec<_>>()
                    .pack(),
            )
            .outputs(
                [packed::CellOutput::new_builder()
                    .type_(Some(tally_type.clone()).pack())
                    .build()]
                .pack(),
            )
            .outputs_data([Bytes::from(state.encode())].pack())
            .build();
        let mut witnesses = vec![Bytes::new(); inputs.len()];
        if let Some((input_index, batch)) = advance_input_index {
            witnesses[input_index] = WitnessArgs::new_builder()
                .input_type(
                    Some(Bytes::from(
                        treasury_common::TallyWitness::Advance(batch)
                            .encode()
                            .unwrap(),
                    ))
                    .pack(),
                )
                .build()
                .as_bytes();
        }
        Transaction::new_builder()
            .raw(raw)
            .witnesses(witnesses.pack())
            .build()
    }

    fn funding_transaction() -> Transaction {
        let raw = packed::RawTransaction::new_builder()
            .outputs([packed::CellOutput::new_builder().build()].pack())
            .outputs_data([Bytes::new()].pack())
            .build();
        Transaction::new_builder().raw(raw).build()
    }

    fn vote_and_spend_transactions(proposal_id: Hash) -> (Vec<u8>, Vec<u8>) {
        let dao_out_point = packed::OutPoint::new_builder()
            .tx_hash([9u8; 32].pack())
            .index(0u32)
            .build();
        let vote_type = packed::Script::new_builder()
            .code_hash([2u8; 32].pack())
            .hash_type(1u8)
            .args(Bytes::from(proposal_id.to_vec()).pack())
            .build();
        let vote_data = VoteData {
            direction: 1,
            amount: 100,
            dao_out_points: vec![unpack_out_point(&dao_out_point)],
        }
        .encode()
        .unwrap();
        let vote = packed::RawTransaction::new_builder()
            .cell_deps(
                [packed::CellDep::new_builder()
                    .out_point(dao_out_point.clone())
                    .build()]
                .pack(),
            )
            .outputs(
                [packed::CellOutput::new_builder()
                    .type_(Some(vote_type).pack())
                    .build()]
                .pack(),
            )
            .outputs_data([Bytes::from(vote_data)].pack())
            .build();
        let spend = packed::RawTransaction::new_builder()
            .inputs(
                [packed::CellInput::new_builder()
                    .previous_output(dao_out_point)
                    .build()]
                .pack(),
            )
            .build();
        (vote.as_slice().to_vec(), spend.as_slice().to_vec())
    }

    #[test]
    fn builds_transaction_inclusion_proof_compatible_with_contract_verifier() {
        let raws = vec![raw_transaction(0), raw_transaction(1), raw_transaction(2)];
        let proven = prove_transaction(10, 0, &raws, [9; 32], 1).unwrap();
        let hashes = raws
            .iter()
            .map(|raw| {
                RawTransaction::from_slice(raw)
                    .unwrap()
                    .calc_tx_hash()
                    .as_slice()
                    .try_into()
                    .unwrap()
            })
            .collect::<Vec<Hash>>();
        let root = CBMT::<Hash, MergeHash>::build_merkle_root(&hashes);
        assert!(verify_cbmt_inclusion(hashes[1], 1, 3, &proven.lemmas, root));
    }

    #[test]
    fn builds_canonical_block_multiproof() {
        let raws = vec![
            raw_transaction(0),
            raw_transaction(1),
            raw_transaction(2),
            raw_transaction(3),
        ];
        let proven = prove_block_transactions(10, 0, &raws, [9; 32], &[0, 2, 3]).unwrap();
        let indexed = proven
            .events
            .iter()
            .map(|transaction| {
                let raw = RawTransaction::from_slice(&transaction.raw_transaction).unwrap();
                (
                    transaction.tx_index,
                    raw.calc_tx_hash().as_slice().try_into().unwrap(),
                )
            })
            .collect::<Vec<_>>();
        let hashes = raws
            .iter()
            .map(|raw| {
                RawTransaction::from_slice(raw)
                    .unwrap()
                    .calc_tx_hash()
                    .as_slice()
                    .try_into()
                    .unwrap()
            })
            .collect::<Vec<Hash>>();
        assert_eq!(
            cbmt_multi_root(proven.tx_count, &indexed, &proven.lemmas),
            Some(CBMT::<Hash, MergeHash>::build_merkle_root(&hashes))
        );
        assert!(prove_block_transactions(10, 0, &raws, [9; 32], &[2, 1]).is_err());
        assert!(prove_block_transactions(10, 0, &raws, [9; 32], &[1, 1]).is_err());
        assert!(prove_block_transactions(10, 0, &raws, [9; 32], &[4]).is_err());
    }

    #[test]
    fn generated_smt_transition_verifies_with_shared_contract_logic() {
        let mut old = Smt::default();
        let key = [1; 32];
        update(&mut old, key, [2; 32]).unwrap();
        let mut new = clone_tree(&old);
        update(&mut new, key, [3; 32]).unwrap();
        let keys = BTreeSet::from([key]);
        let transition = transitions(&old, &new, &keys).unwrap();
        let proof = compiled_proof(&old, &keys).unwrap();
        assert!(verify_smt_transition(
            root(&old),
            root(&new),
            &proof,
            &transition
        ));
    }

    #[test]
    fn new_builder_matches_proposal_start() {
        let proposal = proposal();
        let builder = TallyBuilder::new([7; 32], [8; 32], proposal.start_block, proposal_config());
        assert_eq!(builder.state().next_block, proposal.start_block);
        assert_eq!(builder.state().state_root, ZERO);
    }

    #[test]
    fn empty_final_batch_preserves_roots_and_tally() {
        let proposal = proposal();
        let mut builder =
            TallyBuilder::new([7; 32], [8; 32], proposal.start_block, proposal_config());
        let (state, batch) = builder
            .build_batch(
                &proposal,
                Vec::new(),
                proposal.end_block + 1,
                0,
                proposal.end_block,
            )
            .unwrap();
        assert_eq!(state.phase, TallyPhase::Candidate);
        assert_eq!(state.state_root, ZERO);
        assert_eq!(state.yes, 0);
        assert!(batch.state_transitions.is_empty());
        assert!(batch.state_proof.is_empty());
    }

    #[test]
    fn scanner_ignores_dao_spend_after_vote() {
        let mut proposal = proposal();
        proposal.end_block = 11;
        let proposal_id = [7; 32];
        let builder = TallyBuilder::new(
            proposal_id,
            [8; 32],
            proposal.start_block,
            proposal_config(),
        );
        let (vote, spend) = vote_and_spend_transactions(proposal_id);
        let scanned = builder
            .scan_blocks(
                &proposal,
                &[
                    ChainBlock {
                        block_number: 10,
                        block_hash: [10; 32],
                        parent_hash: [9; 32],
                        raw_transactions: vec![vote],
                        witnesses_root: [20; 32],
                    },
                    ChainBlock {
                        block_number: 11,
                        block_hash: [11; 32],
                        parent_hash: [10; 32],
                        raw_transactions: vec![spend],
                        witnesses_root: [21; 32],
                    },
                ],
            )
            .unwrap();
        assert_eq!(scanned.blocks.len(), 1);
        assert_eq!(scanned.header_deps, vec![[10; 32], [11; 32]]);
        assert_eq!((scanned.end_block, scanned.end_tx_index), (12, 0));

        let mut builder = builder;
        let (candidate, batch) = builder
            .build_batch(
                &proposal,
                scanned.blocks,
                scanned.end_block,
                scanned.end_tx_index,
                scanned.candidate_since,
            )
            .unwrap();
        assert!(batch.blocks[0].events[0].raw_transaction.is_empty());
        assert!(batch.blocks[0].events[0].vote.is_some());
        assert_eq!(candidate.phase, TallyPhase::Candidate);
        assert_eq!(candidate.yes, 100);
        assert_eq!(candidate.processed_events, 1);
    }

    #[test]
    fn fresh_challenger_replays_candidate_history_before_building_proof() {
        let proposal = proposal();
        let config = proposal_config();
        let proposal_id = [7; 32];
        let operator_lock_hash = [8; 32];
        let tally_type = packed::Script::new_builder()
            .code_hash(config.tally_code_hash.pack())
            .hash_type(config.tally_hash_type)
            .args(Bytes::from(vec![42; 32]).pack())
            .build();
        let (vote_raw, _) = vote_and_spend_transactions(proposal_id);
        let vote_block =
            prove_block_transactions(10, 0, core::slice::from_ref(&vote_raw), [20; 32], &[0])
                .unwrap();

        let mut operator_builder = TallyBuilder::new(
            proposal_id,
            operator_lock_hash,
            proposal.start_block,
            config,
        );
        let initial_state = operator_builder.state().clone();
        let (active_state, first_batch) = operator_builder
            .build_batch(&proposal, vec![vote_block], 11, 0, 0)
            .unwrap();
        let (candidate_state, final_batch) = operator_builder
            .build_batch(
                &proposal,
                Vec::new(),
                proposal.end_block + 1,
                0,
                proposal.end_block,
            )
            .unwrap();
        drop(operator_builder);

        let mut source = MemoryTransactionSource::default();
        let initial_out_point =
            source.insert(tally_transaction(&[], &tally_type, &initial_state, None));
        let funding_out_point = source.insert(funding_transaction());
        let active_out_point = source.insert(tally_transaction(
            &[funding_out_point, initial_out_point],
            &tally_type,
            &active_state,
            Some((1, first_batch)),
        ));
        let candidate_out_point = source.insert(tally_transaction(
            &[active_out_point],
            &tally_type,
            &candidate_state,
            Some((0, final_batch)),
        ));

        let replayed = TallyBuilder::replay_candidate(
            proposal_id,
            &proposal,
            config,
            &source,
            candidate_out_point,
        )
        .unwrap();
        assert_eq!(replayed.state(), &candidate_state);

        let omitted_raw = RawTransaction::from_slice(&vote_raw)
            .unwrap()
            .as_builder()
            .version(1u32)
            .build();
        let omitted =
            prove_transaction(12, 0, &[omitted_raw.as_slice().to_vec()], [21; 32], 0).unwrap();
        let omitted_hash = omitted_raw.calc_tx_hash().as_slice().try_into().unwrap();
        let challenge = TallyBuilder::build_omitted_vote_challenge_from_candidate(
            proposal_id,
            &proposal,
            config,
            &source,
            candidate_out_point,
            omitted,
        )
        .unwrap();
        let treasury_common::TallyWitness::ChallengeVote { event_proof, .. } = challenge else {
            panic!("expected omitted-vote challenge");
        };
        let event_key = state_key(EVENT_STATE_NAMESPACE, omitted_hash);
        assert!(verify_smt_transition(
            candidate_state.state_root,
            candidate_state.state_root,
            &event_proof,
            &[LeafTransition {
                key: event_key,
                old_value: ZERO,
                new_value: ZERO,
            }],
        ));
    }
}
