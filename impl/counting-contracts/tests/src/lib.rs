#![cfg(test)]

use ckb_testtool::{
    builtin::ALWAYS_SUCCESS,
    ckb_types::{
        bytes::Bytes,
        core::{EpochNumberWithFraction, HeaderBuilder, TransactionBuilder, TransactionView},
        packed::{CellDep, CellInput, CellOutput, OutPoint, Script},
        prelude::*,
    },
    context::Context,
};
use counting_common::{
    CountingCellData, CountingConfig, OutPoint as CommonOutPoint, ProposalData, ProposalOutcome,
    ProposalPhase, ResultData, VoteData, blake2b_256,
};

const CKB: u64 = 100_000_000;
const VERIFY_CYCLES: u64 = 100_000_000;

struct Fixture {
    context: Context,
    always_success: OutPoint,
    owner_lock: Script,
    proposer_lock: Script,
    challenger_lock: Script,
    guardian_lock: Script,
    burn_lock: Script,
    proposal_type: Script,
    vote_type: Script,
    counting_type: Script,
    policy_type: Script,
    config_type: Script,
    config_cell: OutPoint,
    config: CountingConfig,
    proposal_id: [u8; 32],
}

impl Fixture {
    fn new() -> Self {
        let mut context = Context::default();
        let config_code = context.deploy_cell_by_name("counting-config-type-script");
        let proposal_code = context.deploy_cell_by_name("counting-proposal-type-script");
        let vote_code = context.deploy_cell_by_name("counting-vote-type-script");
        let counting_code = context.deploy_cell_by_name("counting-type-script");
        let policy_code = context.deploy_cell_by_name("counting-policy-type-script");
        let always_success = context.deploy_cell(ALWAYS_SUCCESS.clone());
        let owner_lock = context
            .build_script(&always_success, Bytes::from(vec![1]))
            .unwrap();
        let proposer_lock = context
            .build_script(&always_success, Bytes::from(vec![2]))
            .unwrap();
        let challenger_lock = context
            .build_script(&always_success, Bytes::from(vec![3]))
            .unwrap();
        let guardian_lock = context
            .build_script(&always_success, Bytes::from(vec![4]))
            .unwrap();
        let burn_lock = context
            .build_script(&always_success, Bytes::from(vec![5]))
            .unwrap();
        let treasury_lock = context
            .build_script(&always_success, Bytes::from(vec![6]))
            .unwrap();
        let dao_type = context.build_script(&always_success, Bytes::new()).unwrap();
        let config_type = context
            .build_script(&config_code, Bytes::from(vec![8; 32]))
            .unwrap();
        let config_type_hash = script_hash(&config_type);
        let proposal_type = context
            .build_script(&proposal_code, Bytes::from(vec![0x91; 32]))
            .unwrap();
        let proposal_id = script_hash(&proposal_type);
        let vote_type = context
            .build_script(&vote_code, Bytes::from(proposal_id.to_vec()))
            .unwrap();
        let counting_type = context
            .build_script(&counting_code, Bytes::from(proposal_id.to_vec()))
            .unwrap();
        let policy_type = context
            .build_script(&policy_code, Bytes::from(config_type_hash.to_vec()))
            .unwrap();
        let config = CountingConfig {
            approval_bps: 6_000,
            minimum_total_votes: 100 * CKB as u128,
            maximum_proposal_amount: 1_000 * CKB,
            minimum_challenge_period: 5,
            max_votes_per_counting_cell: 1_000,
            minimum_proposal_bond: 1_000 * CKB,
            treasury_lock_hash: script_hash(&treasury_lock),
            proposal_lock_hash: script_hash(&owner_lock),
            guardian_lock_hash: script_hash(&guardian_lock),
            proposal_bond_burn_lock_hash: script_hash(&burn_lock),
            dao_code_hash: dao_type.code_hash().as_slice().try_into().unwrap(),
            dao_hash_type: dao_type.hash_type().as_slice()[0],
            proposal_code_hash: proposal_type.code_hash().as_slice().try_into().unwrap(),
            proposal_hash_type: proposal_type.hash_type().as_slice()[0],
            vote_code_hash: vote_type.code_hash().as_slice().try_into().unwrap(),
            vote_hash_type: vote_type.hash_type().as_slice()[0],
            counting_code_hash: counting_type.code_hash().as_slice().try_into().unwrap(),
            counting_hash_type: counting_type.hash_type().as_slice()[0],
            policy_type_hash: script_hash(&policy_type),
        };
        let config_cell = context.create_cell(
            CellOutput::new_builder()
                .capacity(1_000 * CKB)
                .lock(owner_lock.clone())
                .type_(Some(config_type.clone()).pack())
                .build(),
            Bytes::from(config.encode().unwrap()),
        );
        Self {
            context,
            always_success,
            owner_lock,
            proposer_lock,
            challenger_lock,
            guardian_lock,
            burn_lock,
            proposal_type,
            vote_type,
            counting_type,
            policy_type,
            config_type,
            config_cell,
            config,
            proposal_id,
        }
    }

    fn proposal(
        &self,
        phase: ProposalPhase,
        yes_amount: u128,
        yes_vote_count: u64,
    ) -> ProposalData {
        ProposalData {
            phase,
            start_block: 10,
            end_block: 20,
            challenge_period: 5,
            minimum_vote_capacity: 10 * CKB,
            requested_amount: 1_000 * CKB,
            yes_amount,
            yes_vote_count,
            receiver_lock_hash: [0x44; 32],
            proposer_lock_hash: script_hash(&self.proposer_lock),
            config_type_hash: script_hash(&self.config_type),
            metadata_hash: [0x55; 32],
        }
    }

    fn proposal_cell(&mut self, proposal: &ProposalData) -> OutPoint {
        self.context.create_cell(
            CellOutput::new_builder()
                .capacity(1_000 * CKB)
                .lock(self.owner_lock.clone())
                .type_(Some(self.proposal_type.clone()).pack())
                .build(),
            Bytes::from(proposal.encode().unwrap()),
        )
    }

    fn counting_cell(&mut self, data: CountingCellData) -> OutPoint {
        let lock = if data.direction == 1 {
            self.proposer_lock.clone()
        } else {
            self.challenger_lock.clone()
        };
        self.context.create_cell(
            CellOutput::new_builder()
                .capacity(200 * CKB)
                .lock(lock)
                .type_(Some(self.counting_type.clone()).pack())
                .build(),
            Bytes::from(data.encode().unwrap()),
        )
    }

    fn config_dep(&self) -> CellDep {
        CellDep::new_builder()
            .out_point(self.config_cell.clone())
            .build()
    }
}

fn script_hash(script: &Script) -> [u8; 32] {
    script.calc_script_hash().as_slice().try_into().unwrap()
}

fn common_out_point(out_point: &OutPoint) -> CommonOutPoint {
    CommonOutPoint {
        tx_hash: out_point.tx_hash().as_slice().try_into().unwrap(),
        index: out_point.index().unpack(),
    }
}

fn cell_dep(out_point: OutPoint) -> CellDep {
    CellDep::new_builder().out_point(out_point).build()
}

fn input(out_point: OutPoint) -> CellInput {
    CellInput::new_builder().previous_output(out_point).build()
}

fn type_id(first_input: &CellInput, output_index: u64) -> [u8; 32] {
    let mut preimage = first_input.as_slice().to_vec();
    preimage.extend_from_slice(&output_index.to_le_bytes());
    blake2b_256(&preimage)
}

fn counting_output(lock: Script, type_script: Script, data: &CountingCellData) -> TransactionView {
    TransactionBuilder::default()
        .output(
            CellOutput::new_builder()
                .capacity(200 * CKB)
                .lock(lock)
                .type_(Some(type_script).pack())
                .build(),
        )
        .output_data(Bytes::from(data.encode().unwrap()).pack())
        .build()
}

#[test]
fn proposal_creation_requires_the_configured_bond() {
    let mut fixture = Fixture::new();
    let funding_cell = fixture.context.create_cell(
        CellOutput::new_builder()
            .capacity(2_000 * CKB)
            .lock(fixture.proposer_lock.clone())
            .build(),
        Bytes::new(),
    );
    let funding_input = input(funding_cell);
    let proposal_type = fixture
        .proposal_type
        .clone()
        .as_builder()
        .args(Bytes::from(type_id(&funding_input, 0).to_vec()).pack())
        .build();
    let proposal = fixture.proposal(ProposalPhase::Open, 0, 0);
    let config_dep = fixture.config_dep();
    let proposal_lock = fixture.owner_lock.clone();
    let build = |capacity| {
        TransactionBuilder::default()
            .cell_dep(config_dep.clone())
            .input(funding_input.clone())
            .output(
                CellOutput::new_builder()
                    .capacity(capacity)
                    .lock(proposal_lock.clone())
                    .type_(Some(proposal_type.clone()).pack())
                    .build(),
            )
            .output_data(Bytes::from(proposal.encode().unwrap()).pack())
            .build()
    };

    let too_small = fixture
        .context
        .complete_tx(build(fixture.config.minimum_proposal_bond - 1));
    assert!(
        fixture
            .context
            .verify_tx(&too_small, VERIFY_CYCLES)
            .is_err()
    );

    let valid = fixture
        .context
        .complete_tx(build(fixture.config.minimum_proposal_bond));
    fixture.context.verify_tx(&valid, VERIFY_CYCLES).unwrap();
}

#[test]
fn vote_cell_authenticates_live_pre_proposal_dao_deposits() {
    let mut fixture = Fixture::new();
    let proposal = fixture.proposal(ProposalPhase::Open, 0, 0);
    let proposal_cell = fixture.proposal_cell(&proposal);
    let dao_type = fixture
        .context
        .build_script(&fixture.always_success, Bytes::new())
        .unwrap();
    let dao_capacity = 500 * CKB;
    let dao_cell = fixture.context.create_cell(
        CellOutput::new_builder()
            .capacity(dao_capacity)
            .lock(fixture.owner_lock.clone())
            .type_(Some(dao_type).pack())
            .build(),
        Bytes::from(vec![0; 8]),
    );
    let dao_header = HeaderBuilder::default()
        .number(9u64)
        .epoch(EpochNumberWithFraction::new(0, 9, 100))
        .build();
    let proposal_header = HeaderBuilder::default()
        .number(10u64)
        .epoch(EpochNumberWithFraction::new(0, 10, 100))
        .build();
    fixture.context.insert_header(dao_header.clone());
    fixture.context.insert_header(proposal_header.clone());
    fixture
        .context
        .link_cell_with_block(dao_cell.clone(), dao_header.hash(), 1);
    fixture
        .context
        .link_cell_with_block(proposal_cell.clone(), proposal_header.hash(), 1);
    let owner_input = fixture.context.create_cell(
        CellOutput::new_builder()
            .capacity(600 * CKB)
            .lock(fixture.owner_lock.clone())
            .build(),
        Bytes::new(),
    );
    let vote = VoteData {
        direction: 1,
        amount: dao_capacity,
        dao_out_points: vec![common_out_point(&dao_cell)],
    };
    let tx = TransactionBuilder::default()
        .cell_dep(cell_dep(proposal_cell))
        .cell_dep(fixture.config_dep())
        .cell_dep(cell_dep(dao_cell))
        .header_dep(dao_header.hash())
        .header_dep(proposal_header.hash())
        .input(input(owner_input))
        .output(
            CellOutput::new_builder()
                .capacity(500 * CKB)
                .lock(fixture.owner_lock.clone())
                .type_(Some(fixture.vote_type.clone()).pack())
                .build(),
        )
        .output_data(Bytes::from(vote.encode().unwrap()).pack())
        .build();
    let tx = fixture.context.complete_tx(tx);
    fixture.context.verify_tx(&tx, VERIFY_CYCLES).unwrap();
}

#[test]
fn counting_cell_recomputes_amount_and_rejects_duplicate_voter_locks() {
    let mut fixture = Fixture::new();
    let proposal = fixture.proposal(ProposalPhase::Closed, 0, 0);
    let proposal_cell = fixture.proposal_cell(&proposal);
    let lock_a = fixture
        .context
        .build_script(&fixture.always_success, Bytes::from(vec![0x21]))
        .unwrap();
    let lock_b = fixture
        .context
        .build_script(&fixture.always_success, Bytes::from(vec![0x22]))
        .unwrap();
    let mut votes = [(lock_a, 400 * CKB), (lock_b, 300 * CKB)];
    votes.sort_by_key(|(lock, _)| script_hash(lock));
    let vote_cells = votes
        .iter()
        .enumerate()
        .map(|(index, (lock, amount))| {
            fixture.context.create_cell(
                CellOutput::new_builder()
                    .capacity(100 * CKB)
                    .lock(lock.clone())
                    .type_(Some(fixture.vote_type.clone()).pack())
                    .build(),
                Bytes::from(
                    VoteData {
                        direction: 1,
                        amount: *amount,
                        dao_out_points: vec![CommonOutPoint {
                            tx_hash: [index as u8 + 1; 32],
                            index: 0,
                        }],
                    }
                    .encode()
                    .unwrap(),
                ),
            )
        })
        .collect::<Vec<_>>();
    let first = script_hash(&votes[0].0)[0];
    let second = script_hash(&votes[1].0)[0];
    let data = CountingCellData {
        direction: 1,
        range_start: first.min(second),
        range_end: first.max(second),
        amount: 700 * CKB as u128,
        vote_count: 2,
    };
    let proposer_auth = fixture.context.create_cell(
        CellOutput::new_builder()
            .capacity(200 * CKB)
            .lock(fixture.proposer_lock.clone())
            .build(),
        Bytes::new(),
    );
    let base = counting_output(
        fixture.proposer_lock.clone(),
        fixture.counting_type.clone(),
        &data,
    )
    .as_advanced_builder()
    .input(input(proposer_auth))
    .build();
    let config_dep = fixture.config_dep();
    let build = |deps: Vec<OutPoint>, data: CountingCellData| {
        base.as_advanced_builder()
            .set_cell_deps(
                core::iter::once(cell_dep(proposal_cell.clone()))
                    .chain(core::iter::once(config_dep.clone()))
                    .chain(deps.into_iter().map(cell_dep))
                    .collect(),
            )
            .set_outputs_data(vec![Bytes::from(data.encode().unwrap()).pack()])
            .build()
    };
    let stranger_auth = fixture.context.create_cell(
        CellOutput::new_builder()
            .capacity(200 * CKB)
            .lock(fixture.owner_lock.clone())
            .build(),
        Bytes::new(),
    );
    let unauthorized = counting_output(
        fixture.proposer_lock.clone(),
        fixture.counting_type.clone(),
        &data,
    )
    .as_advanced_builder()
    .set_cell_deps(
        core::iter::once(cell_dep(proposal_cell.clone()))
            .chain(core::iter::once(config_dep.clone()))
            .chain(vote_cells.iter().cloned().map(cell_dep))
            .collect(),
    )
    .input(input(stranger_auth))
    .build();
    let unauthorized = fixture.context.complete_tx(unauthorized);
    assert!(
        fixture
            .context
            .verify_tx(&unauthorized, VERIFY_CYCLES)
            .is_err()
    );

    let valid = fixture.context.complete_tx(build(vote_cells.clone(), data));
    fixture.context.verify_tx(&valid, VERIFY_CYCLES).unwrap();

    let wrong_amount = fixture.context.complete_tx(build(
        vote_cells.clone(),
        CountingCellData { amount: 1, ..data },
    ));
    assert!(
        fixture
            .context
            .verify_tx(&wrong_amount, VERIFY_CYCLES)
            .is_err()
    );

    let duplicate = fixture.context.create_cell(
        CellOutput::new_builder()
            .capacity(200 * CKB)
            .lock(votes[0].0.clone())
            .type_(Some(fixture.vote_type.clone()).pack())
            .build(),
        Bytes::from(
            VoteData {
                direction: 1,
                amount: 300 * CKB,
                dao_out_points: vec![CommonOutPoint {
                    tx_hash: [9; 32],
                    index: 0,
                }],
            }
            .encode()
            .unwrap(),
        ),
    );
    let duplicate_data = CountingCellData {
        amount: 700 * CKB as u128,
        vote_count: 2,
        range_start: first,
        range_end: first,
        ..data
    };
    let duplicate_tx = fixture.context.complete_tx(build(
        vec![vote_cells[0].clone(), duplicate],
        duplicate_data,
    ));
    assert!(
        fixture
            .context
            .verify_tx(&duplicate_tx, VERIFY_CYCLES)
            .is_err()
    );
}

#[test]
fn proposal_finalization_aggregates_only_non_overlapping_yes_ranges() {
    let mut fixture = Fixture::new();
    let closed = fixture.proposal(ProposalPhase::Closed, 0, 0);
    let proposal_cell = fixture.proposal_cell(&closed);
    let left = fixture.counting_cell(CountingCellData {
        direction: 1,
        range_start: 0,
        range_end: 100,
        amount: 400 * CKB as u128,
        vote_count: 4,
    });
    let right = fixture.counting_cell(CountingCellData {
        direction: 1,
        range_start: 101,
        range_end: 255,
        amount: 300 * CKB as u128,
        vote_count: 3,
    });
    let mut finalized = closed.clone();
    finalized.phase = ProposalPhase::Finalized;
    finalized.yes_amount = 700 * CKB as u128;
    finalized.yes_vote_count = 7;
    let overlap = fixture.counting_cell(CountingCellData {
        direction: 1,
        range_start: 100,
        range_end: 255,
        amount: 300 * CKB as u128,
        vote_count: 3,
    });
    let config_dep = fixture.config_dep();
    let owner_lock = fixture.owner_lock.clone();
    let proposal_type = fixture.proposal_type.clone();
    let build = |right: OutPoint| {
        TransactionBuilder::default()
            .cell_dep(config_dep.clone())
            .input(input(proposal_cell.clone()))
            .input(input(left.clone()))
            .input(input(right))
            .output(
                CellOutput::new_builder()
                    .capacity(1_000 * CKB)
                    .lock(owner_lock.clone())
                    .type_(Some(proposal_type.clone()).pack())
                    .build(),
            )
            .output_data(Bytes::from(finalized.encode().unwrap()).pack())
            .output(
                CellOutput::new_builder()
                    .capacity(400 * CKB)
                    .lock(owner_lock.clone())
                    .build(),
            )
            .output_data(Bytes::new().pack())
            .build()
    };
    let valid = fixture.context.complete_tx(build(right));
    fixture.context.verify_tx(&valid, VERIFY_CYCLES).unwrap();

    let invalid = fixture.context.complete_tx(build(overlap));
    assert!(fixture.context.verify_tx(&invalid, VERIFY_CYCLES).is_err());

    let unauthorized_left = fixture.context.create_cell(
        CellOutput::new_builder()
            .capacity(200 * CKB)
            .lock(fixture.owner_lock.clone())
            .type_(Some(fixture.counting_type.clone()).pack())
            .build(),
        Bytes::from(
            CountingCellData {
                direction: 1,
                range_start: 0,
                range_end: 100,
                amount: 400 * CKB as u128,
                vote_count: 4,
            }
            .encode()
            .unwrap(),
        ),
    );
    let unauthorized_right = fixture.context.create_cell(
        CellOutput::new_builder()
            .capacity(200 * CKB)
            .lock(fixture.owner_lock.clone())
            .type_(Some(fixture.counting_type.clone()).pack())
            .build(),
        Bytes::from(
            CountingCellData {
                direction: 1,
                range_start: 101,
                range_end: 255,
                amount: 300 * CKB as u128,
                vote_count: 3,
            }
            .encode()
            .unwrap(),
        ),
    );
    let unauthorized = fixture.context.complete_tx(build(unauthorized_right));
    let unauthorized = unauthorized
        .as_advanced_builder()
        .set_inputs(vec![
            input(proposal_cell),
            input(unauthorized_left),
            unauthorized.inputs().get(2).unwrap(),
        ])
        .build();
    assert!(
        fixture
            .context
            .verify_tx(&unauthorized, VERIFY_CYCLES)
            .is_err()
    );
}

#[test]
fn no_counting_cells_can_challenge_a_finalized_candidate() {
    let mut fixture = Fixture::new();
    let finalized = fixture.proposal(ProposalPhase::Finalized, 600 * CKB as u128, 6);
    let proposal_cell = fixture.proposal_cell(&finalized);
    let no_cell = fixture.counting_cell(CountingCellData {
        direction: 0,
        range_start: 0,
        range_end: 255,
        amount: 400 * CKB as u128,
        vote_count: 4,
    });
    let config_data = fixture.config.encode().unwrap();
    let result = ResultData {
        outcome: ProposalOutcome::RejectedByVote,
        proposal_id: fixture.proposal_id,
        requested_amount: finalized.requested_amount,
        receiver_lock_hash: finalized.receiver_lock_hash,
        yes: finalized.yes_amount,
        no: 400 * CKB as u128,
        final_state_hash: blake2b_256(&finalized.encode().unwrap()),
        proposal_config_data_hash: blake2b_256(&config_data),
        veto_reason_hash: [0; 32],
    };
    let config_dep = fixture.config_dep();
    let policy_type = fixture.policy_type.clone();
    let challenger_lock = fixture.challenger_lock.clone();
    let build = |result_capacity: u64, result_lock: Script| {
        TransactionBuilder::default()
            .cell_dep(config_dep.clone())
            .input(input(proposal_cell.clone()))
            .input(input(no_cell.clone()))
            .output(
                CellOutput::new_builder()
                    .capacity(result_capacity)
                    .lock(result_lock)
                    .type_(Some(policy_type.clone()).pack())
                    .build(),
            )
            .output_data(Bytes::from(result.encode().unwrap()).pack())
            .output(
                CellOutput::new_builder()
                    .capacity(1_200 * CKB - result_capacity)
                    .lock(challenger_lock.clone())
                    .build(),
            )
            .output_data(Bytes::new().pack())
            .build()
    };

    let left_no = fixture.context.create_cell(
        CellOutput::new_builder()
            .capacity(200 * CKB)
            .lock(fixture.challenger_lock.clone())
            .type_(Some(fixture.counting_type.clone()).pack())
            .build(),
        Bytes::from(
            CountingCellData {
                direction: 0,
                range_start: 0,
                range_end: 127,
                amount: 200 * CKB as u128,
                vote_count: 2,
            }
            .encode()
            .unwrap(),
        ),
    );
    let foreign_no = fixture.context.create_cell(
        CellOutput::new_builder()
            .capacity(200 * CKB)
            .lock(fixture.owner_lock.clone())
            .type_(Some(fixture.counting_type.clone()).pack())
            .build(),
        Bytes::from(
            CountingCellData {
                direction: 0,
                range_start: 128,
                range_end: 255,
                amount: 200 * CKB as u128,
                vote_count: 2,
            }
            .encode()
            .unwrap(),
        ),
    );
    let mixed_challengers = TransactionBuilder::default()
        .cell_dep(config_dep.clone())
        .input(input(proposal_cell.clone()))
        .input(input(left_no))
        .input(input(foreign_no))
        .output(
            CellOutput::new_builder()
                .capacity(1_000 * CKB)
                .lock(fixture.challenger_lock.clone())
                .type_(Some(policy_type.clone()).pack())
                .build(),
        )
        .output_data(Bytes::from(result.encode().unwrap()).pack())
        .output(
            CellOutput::new_builder()
                .capacity(400 * CKB)
                .lock(fixture.challenger_lock.clone())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .build();
    let mixed_challengers = fixture.context.complete_tx(mixed_challengers);
    assert!(
        fixture
            .context
            .verify_tx(&mixed_challengers, VERIFY_CYCLES)
            .is_err()
    );

    let redirected = fixture
        .context
        .complete_tx(build(1_000 * CKB, fixture.owner_lock.clone()));
    assert!(
        fixture
            .context
            .verify_tx(&redirected, VERIFY_CYCLES)
            .is_err()
    );

    let reduced = fixture.context.complete_tx(build(
        fixture.config.minimum_proposal_bond - 1,
        fixture.challenger_lock.clone(),
    ));
    assert!(fixture.context.verify_tx(&reduced, VERIFY_CYCLES).is_err());

    let valid = fixture.context.complete_tx(build(
        fixture.config.minimum_proposal_bond,
        fixture.challenger_lock.clone(),
    ));
    fixture.context.verify_tx(&valid, VERIFY_CYCLES).unwrap();
}

#[test]
fn passed_result_requires_the_relative_challenge_period() {
    let mut fixture = Fixture::new();
    let finalized = fixture.proposal(ProposalPhase::Finalized, 600 * CKB as u128, 6);
    let proposal_cell = fixture.proposal_cell(&finalized);
    let result = ResultData {
        outcome: ProposalOutcome::Passed,
        proposal_id: fixture.proposal_id,
        requested_amount: finalized.requested_amount,
        receiver_lock_hash: finalized.receiver_lock_hash,
        yes: finalized.yes_amount,
        no: 0,
        final_state_hash: blake2b_256(&finalized.encode().unwrap()),
        proposal_config_data_hash: blake2b_256(&fixture.config.encode().unwrap()),
        veto_reason_hash: [0; 32],
    };
    let config_dep = fixture.config_dep();
    let proposer_lock = fixture.proposer_lock.clone();
    let policy_type = fixture.policy_type.clone();
    let build = |blocks: u64| {
        TransactionBuilder::default()
            .cell_dep(config_dep.clone())
            .input(
                CellInput::new_builder()
                    .since(0x8000_0000_0000_0000 | blocks)
                    .previous_output(proposal_cell.clone())
                    .build(),
            )
            .output(
                CellOutput::new_builder()
                    .capacity(1_000 * CKB)
                    .lock(proposer_lock.clone())
                    .type_(Some(policy_type.clone()).pack())
                    .build(),
            )
            .output_data(Bytes::from(result.encode().unwrap()).pack())
            .build()
    };
    let too_early = fixture.context.complete_tx(build(4));
    assert!(
        fixture
            .context
            .verify_tx(&too_early, VERIFY_CYCLES)
            .is_err()
    );
    let mature = fixture.context.complete_tx(build(5));
    fixture.context.verify_tx(&mature, VERIFY_CYCLES).unwrap();
}

#[test]
fn guardian_can_veto_before_final_settlement() {
    let mut fixture = Fixture::new();
    let proposal = fixture.proposal(ProposalPhase::Closed, 0, 0);
    let proposal_cell = fixture.proposal_cell(&proposal);
    let guardian_auth = fixture.context.create_cell(
        CellOutput::new_builder()
            .capacity(100 * CKB)
            .lock(fixture.guardian_lock.clone())
            .build(),
        Bytes::new(),
    );
    let result = ResultData {
        outcome: ProposalOutcome::Vetoed,
        proposal_id: fixture.proposal_id,
        requested_amount: proposal.requested_amount,
        receiver_lock_hash: proposal.receiver_lock_hash,
        yes: 0,
        no: 0,
        final_state_hash: [0; 32],
        proposal_config_data_hash: blake2b_256(&fixture.config.encode().unwrap()),
        veto_reason_hash: [0xaa; 32],
    };
    let tx = TransactionBuilder::default()
        .cell_dep(fixture.config_dep())
        .input(input(proposal_cell))
        .input(input(guardian_auth))
        .output(
            CellOutput::new_builder()
                .capacity(1_000 * CKB)
                .lock(fixture.burn_lock.clone())
                .type_(Some(fixture.policy_type.clone()).pack())
                .build(),
        )
        .output_data(Bytes::from(result.encode().unwrap()).pack())
        .output(
            CellOutput::new_builder()
                .capacity(100 * CKB)
                .lock(fixture.guardian_lock.clone())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .build();
    let tx = fixture.context.complete_tx(tx);
    fixture.context.verify_tx(&tx, VERIFY_CYCLES).unwrap();
}

fn benchmark_counting_batch(vote_count: usize) -> (u64, usize) {
    let mut fixture = Fixture::new();
    let proposal = fixture.proposal(ProposalPhase::Closed, 0, 0);
    let proposal_cell = fixture.proposal_cell(&proposal);
    let mut votes = (0..vote_count)
        .map(|index| {
            let lock = fixture
                .context
                .build_script(
                    &fixture.always_success,
                    Bytes::from((index as u32).to_le_bytes().to_vec()),
                )
                .unwrap();
            (lock, index)
        })
        .collect::<Vec<_>>();
    votes.sort_unstable_by_key(|(lock, _)| script_hash(lock));
    let mut vote_deps = Vec::with_capacity(vote_count);
    for (lock, index) in &votes {
        let vote = VoteData {
            direction: 1,
            amount: 10 * CKB,
            dao_out_points: vec![CommonOutPoint {
                tx_hash: blake2b_256(&(index.to_le_bytes())),
                index: 0,
            }],
        };
        let vote_cell = fixture.context.create_cell(
            CellOutput::new_builder()
                .capacity(100 * CKB)
                .lock(lock.clone())
                .type_(Some(fixture.vote_type.clone()).pack())
                .build(),
            Bytes::from(vote.encode().unwrap()),
        );
        vote_deps.push(cell_dep(vote_cell));
    }
    let data = CountingCellData {
        direction: 1,
        range_start: votes.first().map_or(0, |(lock, _)| script_hash(lock)[0]),
        range_end: votes.last().map_or(0, |(lock, _)| script_hash(lock)[0]),
        amount: vote_count as u128 * 10 * CKB as u128,
        vote_count: vote_count as u32,
    };
    let proposer_auth = fixture.context.create_cell(
        CellOutput::new_builder()
            .capacity(200 * CKB)
            .lock(fixture.proposer_lock.clone())
            .build(),
        Bytes::new(),
    );
    let tx = TransactionBuilder::default()
        .cell_dep(cell_dep(proposal_cell))
        .cell_dep(fixture.config_dep())
        .cell_deps(vote_deps)
        .input(input(proposer_auth))
        .output(
            CellOutput::new_builder()
                .capacity(200 * CKB)
                .lock(fixture.proposer_lock.clone())
                .type_(Some(fixture.counting_type.clone()).pack())
                .build(),
        )
        .output_data(Bytes::from(data.encode().unwrap()).pack())
        .build();
    let tx = fixture.context.complete_tx(tx);
    let transaction_bytes = tx.data().serialized_size_in_block();
    let cycles = fixture.context.verify_tx(&tx, 3_500_000_000).unwrap();
    (cycles, transaction_bytes)
}

#[test]
#[ignore = "cycle benchmark"]
fn benchmark_counting_cell_creation() {
    for vote_count in [1usize, 10, 100, 500, 1_000] {
        let (cycles, transaction_bytes) = benchmark_counting_batch(vote_count);
        println!(
            "votes={vote_count:4} cycles={cycles:10} tx={:.3} KB",
            transaction_bytes as f64 / 1_000.0
        );
    }
}
