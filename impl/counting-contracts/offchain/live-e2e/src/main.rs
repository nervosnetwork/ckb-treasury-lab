use std::{
    collections::BTreeMap,
    env,
    error::Error,
    fmt::Write as _,
    fs::{self, File},
    io,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use ckb_gen_types::{bytes::Bytes, packed, prelude::*};
use ckb_jsonrpc_types::{Byte32 as JsonByte32, JsonBytes, Transaction as JsonTransaction};
use counting_common::{
    CodecError, CountingCellData, CountingConfig, Hash, OutPoint, ProposalData, ProposalOutcome,
    ProposalPhase, ResultData, VoteData, blake2b_256,
};
use reqwest::{Url, blocking::Client};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

const CKB: u64 = 100_000_000;
const DATA_HASH_TYPE: u8 = 0;
const TYPE_HASH_TYPE: u8 = 1;
const DATA1_HASH_TYPE: u8 = 2;
const ALWAYS_SUCCESS_HASH: &str =
    "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5";

type AnyResult<T> = Result<T, Box<dyn Error>>;

#[derive(Clone)]
struct CellRef {
    out_point: packed::OutPoint,
    output: packed::CellOutput,
    data: Bytes,
}

#[derive(Clone)]
struct CodeCells {
    always: packed::OutPoint,
    dao: packed::OutPoint,
    proposal: packed::OutPoint,
    vote: packed::OutPoint,
    counting: packed::OutPoint,
    policy: packed::OutPoint,
    treasury: packed::OutPoint,
}

struct NodeGuard(Child);

impl Drop for NodeGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Rpc {
    client: Client,
    endpoint: Url,
    next_id: u64,
}

struct RpcBlock {
    header: packed::Header,
    transactions: Vec<packed::Transaction>,
}

#[derive(Debug)]
struct Commit {
    hash: Hash,
    block_number: u64,
    tx_index: u32,
}

#[derive(Deserialize)]
struct JsonRpcResponse<T> {
    result: Option<T>,
    error: Option<JsonRpcError>,
}

#[derive(Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

impl Rpc {
    fn new(endpoint: &str) -> AnyResult<Self> {
        Ok(Self {
            client: Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(30))
                .build()?,
            endpoint: Url::parse(endpoint)?,
            next_id: 1,
        })
    }

    fn call<T: DeserializeOwned>(&mut self, method: &str, params: Value) -> AnyResult<Option<T>> {
        let id = self.next_id;
        self.next_id += 1;
        let response = self
            .client
            .post(self.endpoint.clone())
            .json(&json!({
                "id": id,
                "jsonrpc": "2.0",
                "method": method,
                "params": params,
            }))
            .send()?
            .error_for_status()?
            .json::<JsonRpcResponse<T>>()?;
        if let Some(error) = response.error {
            return Err(other(format!(
                "RPC {method} failed with {}: {}",
                error.code, error.message
            )));
        }
        Ok(response.result)
    }

    fn wait_ready(&mut self) -> AnyResult<()> {
        let mut last_error = None;
        for _ in 0..100 {
            match self.tip_number() {
                Ok(_) => return Ok(()),
                Err(error) => last_error = Some(error.to_string()),
            }
            thread::sleep(Duration::from_millis(100));
        }
        Err(other(format!(
            "CKB RPC did not become ready; last error: {}",
            last_error.unwrap_or_else(|| "unknown".to_owned())
        )))
    }

    fn tip_number(&mut self) -> AnyResult<u64> {
        let value = self
            .call::<String>("get_tip_block_number", json!([]))?
            .ok_or_else(|| other("missing tip number"))?;
        Ok(u64::from_str_radix(
            value.strip_prefix("0x").unwrap_or(&value),
            16,
        )?)
    }

    fn generate_block(&mut self) -> AnyResult<u64> {
        self.call::<JsonByte32>("generate_block", json!([]))?
            .ok_or_else(|| other("generate_block returned no hash"))?;
        self.tip_number()
    }

    fn mine_to(&mut self, target: u64) -> AnyResult<()> {
        while self.tip_number()? < target {
            self.generate_block()?;
        }
        Ok(())
    }

    fn packed_block(&mut self, number: u64) -> AnyResult<RpcBlock> {
        let bytes = self
            .call::<JsonBytes>(
                "get_block_by_number",
                json!([format!("0x{number:x}"), "0x0", false]),
            )?
            .ok_or_else(|| other(format!("block {number} was not found")))?
            .into_bytes();
        if let Ok(block) = packed::BlockV1::from_slice(&bytes) {
            return Ok(RpcBlock {
                header: block.header(),
                transactions: block.transactions().into_iter().collect(),
            });
        }
        let block = packed::Block::from_slice(&bytes)
            .map_err(|_| other(format!("block {number} is invalid Molecule data")))?;
        Ok(RpcBlock {
            header: block.header(),
            transactions: block.transactions().into_iter().collect(),
        })
    }

    fn block_hash(&mut self, number: u64) -> AnyResult<Hash> {
        Ok(packed_hash(
            &self.packed_block(number)?.header.calc_header_hash(),
        ))
    }

    fn submit(&mut self, transaction: packed::Transaction) -> AnyResult<Hash> {
        let expected = packed_hash(&transaction.raw().calc_tx_hash());
        let returned = self
            .call::<JsonByte32>(
                "send_transaction",
                json!([JsonTransaction::from(transaction), "passthrough"]),
            )?
            .ok_or_else(|| other("send_transaction returned no hash"))?;
        let returned: Hash = returned.0;
        if returned != expected {
            return Err(other("send_transaction returned an unexpected hash"));
        }
        Ok(expected)
    }

    fn commit(&mut self, label: &str, transaction: packed::Transaction) -> AnyResult<Commit> {
        let hash = self.submit(transaction)?;
        for _ in 0..30 {
            let number = self.generate_block()?;
            let block = self.packed_block(number)?;
            for (index, transaction) in block.transactions.into_iter().enumerate() {
                if packed_hash(&transaction.raw().calc_tx_hash()) == hash {
                    println!(
                        "  {label:<24} block={number:<4} tx={} index={index}",
                        hex_hash(hash)
                    );
                    return Ok(Commit {
                        hash,
                        block_number: number,
                        tx_index: index as u32,
                    });
                }
            }
        }
        Err(other(format!("transaction {label} was not committed")))
    }

    fn live_status(&mut self, out_point: &packed::OutPoint) -> AnyResult<String> {
        let tx_hash = packed_hash(&out_point.tx_hash());
        let index: u32 = out_point.index().unpack();
        let value = self
            .call::<Value>(
                "get_live_cell",
                json!([{"tx_hash": hex_hash(tx_hash), "index": format!("0x{index:x}")}, false]),
            )?
            .ok_or_else(|| other("get_live_cell returned no result"))?;
        Ok(value["status"].as_str().unwrap_or("unknown").to_owned())
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Counting Cell live E2E failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> AnyResult<()> {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| other("cannot locate counting-contracts directory"))?
        .to_path_buf();
    let ckb_repo = env::var_os("CKB_REPO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/Users/yukang/code/ckb"));
    let ckb_bin = env::var_os("CKB_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| ckb_repo.join("target/debug/ckb"));
    require_file(&ckb_bin)?;

    let binaries = contract_binaries(&workspace, &ckb_repo)?;
    let code_hashes = binaries
        .iter()
        .map(|(name, path)| Ok((name.clone(), blake2b_256(&fs::read(path)?))))
        .collect::<AnyResult<BTreeMap<_, _>>>()?;
    if hex_hash(*code_hashes.get("always").unwrap()) != ALWAYS_SUCCESS_HASH {
        return Err(other("unexpected always-success binary hash"));
    }

    let always_hash = *code_hashes.get("always").unwrap();
    let proposal_lock = script(always_hash, DATA_HASH_TYPE, &[0x23]);
    let proposer_lock = script(always_hash, DATA_HASH_TYPE, &[0x01]);
    let yes_voter_lock = script(always_hash, DATA_HASH_TYPE, &[0x11]);
    let no_voter_lock = script(always_hash, DATA_HASH_TYPE, &[0x12]);
    let challenger_lock = script(always_hash, DATA_HASH_TYPE, &[0x22]);
    let guardian_lock = script(always_hash, DATA_HASH_TYPE, &[0x24]);
    let burn_lock = script(always_hash, DATA_HASH_TYPE, &[0]);
    let receiver_lock = script(always_hash, DATA_HASH_TYPE, &[0x55]);
    let treasury_config_type = script(always_hash, DATA_HASH_TYPE, &[0x42]);
    let treasury_config_type_hash = packed_hash(&treasury_config_type.calc_script_hash());
    let treasury_lock = script(
        *code_hashes.get("treasury").unwrap(),
        DATA1_HASH_TYPE,
        &treasury_config_type_hash,
    );
    let config_type = script(
        *code_hashes.get("config").unwrap(),
        DATA1_HASH_TYPE,
        &[0x41; 32],
    );
    let policy_type = script(
        *code_hashes.get("policy").unwrap(),
        DATA1_HASH_TYPE,
        &packed_hash(&config_type.calc_script_hash()),
    );
    let genesis_input = packed::CellInput::new_cellbase_input(0);
    let mut type_id_code_hash = [0; 32];
    type_id_code_hash[25..].copy_from_slice(b"TYPE_ID");
    let dao_code_type = script(
        type_id_code_hash,
        TYPE_HASH_TYPE,
        &type_id(&genesis_input, 2),
    );
    let dao_type = script(
        packed_hash(&dao_code_type.calc_script_hash()),
        TYPE_HASH_TYPE,
        &[],
    );
    let config = CountingConfig {
        approval_bps: 6_000,
        minimum_total_votes: 100 * CKB as u128,
        maximum_proposal_amount: 1_000 * CKB,
        minimum_challenge_period: 5,
        max_votes_per_counting_cell: 1_000,
        minimum_proposal_bond: 1_500 * CKB,
        treasury_lock_hash: packed_hash(&treasury_lock.calc_script_hash()),
        proposal_lock_hash: packed_hash(&proposal_lock.calc_script_hash()),
        guardian_lock_hash: packed_hash(&guardian_lock.calc_script_hash()),
        proposal_bond_burn_lock_hash: packed_hash(&burn_lock.calc_script_hash()),
        dao_code_hash: packed_hash(&dao_type.code_hash()),
        dao_hash_type: TYPE_HASH_TYPE,
        proposal_code_hash: *code_hashes.get("proposal").unwrap(),
        proposal_hash_type: DATA1_HASH_TYPE,
        vote_code_hash: *code_hashes.get("vote").unwrap(),
        vote_hash_type: DATA1_HASH_TYPE,
        counting_code_hash: *code_hashes.get("counting").unwrap(),
        counting_hash_type: DATA1_HASH_TYPE,
        policy_type_hash: packed_hash(&policy_type.calc_script_hash()),
    };
    let treasury_config = encode_treasury_config(
        packed_hash(&policy_type.calc_script_hash()),
        packed_hash(&burn_lock.calc_script_hash()),
    );

    let run_id = format!(
        "{}-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        std::process::id()
    );
    let run_dir = workspace.join("target/live-e2e").join(run_id);
    fs::create_dir_all(&run_dir)?;
    let spec_path = run_dir.join("source.toml");
    write_chain_spec(
        &spec_path,
        &binaries,
        &config_type,
        &config,
        &treasury_config_type,
        &treasury_config,
        &treasury_lock,
        always_hash,
    )?;
    let rpc_port = free_port()?;
    let p2p_port = free_port()?;
    initialize_node(&ckb_bin, &run_dir, &spec_path, rpc_port, p2p_port)?;
    enable_integration_rpc(&run_dir.join("ckb.toml"))?;
    let log_path = run_dir.join("node-process.log");
    let log = File::create(&log_path)?;
    let child = Command::new(&ckb_bin)
        .args(["run", "-C"])
        .arg(&run_dir)
        .arg("--ba-advanced")
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .spawn()?;
    let _guard = NodeGuard(child);
    let endpoint = format!("http://127.0.0.1:{rpc_port}");
    let mut rpc = Rpc::new(&endpoint)?;
    rpc.wait_ready()?;
    println!("Hash-range Counting Cell live E2E");
    println!("  chain directory          {}", run_dir.display());

    let genesis = rpc.packed_block(0)?;
    let genesis_tx = genesis
        .transactions
        .first()
        .ok_or_else(|| other("genesis has no Cellbase transaction"))?;
    let cells = genesis_cells(genesis_tx);
    let code = locate_code_cells(&cells, &code_hashes)?;
    let config_cell = cells
        .iter()
        .find(|cell| cell.output.type_().to_opt().as_ref() == Some(&config_type))
        .cloned()
        .ok_or_else(|| other("CountingConfig Cell is missing"))?;
    let treasury_config_cell = cells
        .iter()
        .find(|cell| cell.output.type_().to_opt().as_ref() == Some(&treasury_config_type))
        .cloned()
        .ok_or_else(|| other("TreasuryConfig Cell is missing"))?;
    let treasury_cell = find_cell(&cells, &treasury_lock, 5_000 * CKB)?;
    let proposal_funding = find_cell(&cells, &proposer_lock, 1_500 * CKB)?;
    let proposer_counting_funding = find_cell(&cells, &proposer_lock, 200 * CKB)?;
    let yes_dao_funding = find_cell(&cells, &yes_voter_lock, 1_000 * CKB)?;
    let yes_vote_funding = find_cell(&cells, &yes_voter_lock, 500 * CKB)?;
    let no_dao_funding = find_cell(&cells, &no_voter_lock, 800 * CKB)?;
    let no_vote_funding = find_cell(&cells, &no_voter_lock, 500 * CKB)?;
    let challenger_counting_funding = find_cell(&cells, &challenger_lock, 200 * CKB)?;
    let passed_proposal_funding = find_cell(&cells, &proposer_lock, 1_600 * CKB)?;
    let passed_counting_funding = find_cell(&cells, &proposer_lock, 210 * CKB)?;
    let passed_dao_funding = find_cell(&cells, &yes_voter_lock, 1_100 * CKB)?;
    let passed_vote_funding = find_cell(&cells, &yes_voter_lock, 510 * CKB)?;

    let yes_dao = rpc.commit(
        "YES DAO deposit",
        simple_transfer(
            &yes_dao_funding,
            &yes_voter_lock,
            Some(dao_type.clone()),
            Bytes::from(vec![0; 8]),
            vec![code_dep(&code.always), code_dep(&code.dao)],
        ),
    )?;
    let yes_dao_cell = output_cell(
        &yes_dao,
        0,
        1_000 * CKB,
        &yes_voter_lock,
        Some(&dao_type),
        vec![0; 8],
    );
    let no_dao = rpc.commit(
        "NO DAO deposit",
        simple_transfer(
            &no_dao_funding,
            &no_voter_lock,
            Some(dao_type.clone()),
            Bytes::from(vec![0; 8]),
            vec![code_dep(&code.always), code_dep(&code.dao)],
        ),
    )?;
    let no_dao_cell = output_cell(
        &no_dao,
        0,
        800 * CKB,
        &no_voter_lock,
        Some(&dao_type),
        vec![0; 8],
    );

    let start_block = rpc.tip_number()? + 8;
    let end_block = start_block + 20;
    let proposal_input = input(&proposal_funding.out_point);
    let proposal_type = script(
        *code_hashes.get("proposal").unwrap(),
        DATA1_HASH_TYPE,
        &type_id(&proposal_input, 0),
    );
    let proposal_id = packed_hash(&proposal_type.calc_script_hash());
    let open = ProposalData {
        phase: ProposalPhase::Open,
        start_block,
        end_block,
        challenge_period: 5,
        minimum_vote_capacity: 100 * CKB,
        requested_amount: 100 * CKB,
        yes_amount: 0,
        yes_vote_count: 0,
        receiver_lock_hash: [0x55; 32],
        proposer_lock_hash: packed_hash(&proposer_lock.calc_script_hash()),
        config_type_hash: packed_hash(&config_type.calc_script_hash()),
        metadata_hash: blake2b_256(b"Counting Cell live E2E"),
    };
    let proposal_commit = rpc.commit(
        "create Proposal",
        transaction(
            vec![proposal_input],
            vec![
                code_dep(&code.always),
                code_dep(&code.proposal),
                code_dep(&config_cell.out_point),
            ],
            vec![],
            vec![output(
                1_500 * CKB,
                &proposal_lock,
                Some(proposal_type.clone()),
            )],
            vec![Bytes::from(encoded(open.encode())?)],
        ),
    )?;
    let open_cell = output_cell(
        &proposal_commit,
        0,
        1_500 * CKB,
        &proposal_lock,
        Some(&proposal_type),
        encoded(open.encode())?,
    );
    rpc.mine_to(start_block)?;
    let vote_type = script(
        *code_hashes.get("vote").unwrap(),
        DATA1_HASH_TYPE,
        &proposal_id,
    );
    let proposal_header = rpc.block_hash(proposal_commit.block_number)?;
    let yes_dao_header = rpc.block_hash(yes_dao.block_number)?;
    let yes_vote = submit_vote(
        &mut rpc,
        "submit YES vote",
        &code,
        &yes_vote_funding,
        &yes_voter_lock,
        &vote_type,
        &open_cell,
        &config_cell,
        &yes_dao_cell,
        1,
        1_000 * CKB,
        vec![proposal_header, yes_dao_header],
    )?;
    let no_dao_header = rpc.block_hash(no_dao.block_number)?;
    let no_vote = submit_vote(
        &mut rpc,
        "submit NO vote",
        &code,
        &no_vote_funding,
        &no_voter_lock,
        &vote_type,
        &open_cell,
        &config_cell,
        &no_dao_cell,
        0,
        800 * CKB,
        vec![proposal_header, no_dao_header],
    )?;
    if yes_vote.block_number > end_block || no_vote.block_number > end_block {
        return Err(other("a vote was committed after the planned close height"));
    }
    rpc.mine_to(end_block)?;

    let mut closed = open.clone();
    closed.phase = ProposalPhase::Closed;
    let end_header = rpc.block_hash(end_block)?;
    let close_commit = rpc.commit(
        "close Proposal",
        transaction(
            vec![input(&open_cell.out_point)],
            vec![
                code_dep(&code.always),
                code_dep(&code.proposal),
                code_dep(&config_cell.out_point),
            ],
            vec![end_header],
            vec![output(
                1_500 * CKB,
                &proposal_lock,
                Some(proposal_type.clone()),
            )],
            vec![Bytes::from(encoded(closed.encode())?)],
        ),
    )?;
    let closed_cell = output_cell(
        &close_commit,
        0,
        1_500 * CKB,
        &proposal_lock,
        Some(&proposal_type),
        encoded(closed.encode())?,
    );
    let counting_type = script(
        *code_hashes.get("counting").unwrap(),
        DATA1_HASH_TYPE,
        &proposal_id,
    );
    let yes_lock_hash = packed_hash(&yes_voter_lock.calc_script_hash());
    let yes_count = CountingCellData {
        direction: 1,
        range_start: yes_lock_hash[0],
        range_end: yes_lock_hash[0],
        amount: 1_000 * CKB as u128,
        vote_count: 1,
    };
    let yes_count_commit = rpc.commit(
        "create YES Counting Cell",
        transaction(
            vec![input(&proposer_counting_funding.out_point)],
            vec![
                code_dep(&code.always),
                code_dep(&code.counting),
                code_dep(&closed_cell.out_point),
                code_dep(&config_cell.out_point),
                code_dep(&out_point(yes_vote.hash, 0)),
            ],
            vec![],
            vec![output(
                200 * CKB,
                &proposer_lock,
                Some(counting_type.clone()),
            )],
            vec![Bytes::from(encoded(yes_count.encode())?)],
        ),
    )?;
    let yes_count_cell = output_cell(
        &yes_count_commit,
        0,
        200 * CKB,
        &proposer_lock,
        Some(&counting_type),
        encoded(yes_count.encode())?,
    );
    let mut finalized = closed.clone();
    finalized.phase = ProposalPhase::Finalized;
    finalized.yes_amount = yes_count.amount;
    finalized.yes_vote_count = 1;
    let finalized_commit = rpc.commit(
        "finalize YES candidate",
        transaction(
            vec![
                input(&closed_cell.out_point),
                input(&yes_count_cell.out_point),
            ],
            vec![
                code_dep(&code.always),
                code_dep(&code.proposal),
                code_dep(&code.counting),
                code_dep(&config_cell.out_point),
            ],
            vec![],
            vec![
                output(1_500 * CKB, &proposal_lock, Some(proposal_type.clone())),
                output(200 * CKB, &proposer_lock, None),
            ],
            vec![Bytes::from(encoded(finalized.encode())?), Bytes::new()],
        ),
    )?;
    let finalized_cell = output_cell(
        &finalized_commit,
        0,
        1_500 * CKB,
        &proposal_lock,
        Some(&proposal_type),
        encoded(finalized.encode())?,
    );

    let no_lock_hash = packed_hash(&no_voter_lock.calc_script_hash());
    let no_count = CountingCellData {
        direction: 0,
        range_start: no_lock_hash[0],
        range_end: no_lock_hash[0],
        amount: 800 * CKB as u128,
        vote_count: 1,
    };
    let no_count_commit = rpc.commit(
        "create NO Counting Cell",
        transaction(
            vec![input(&challenger_counting_funding.out_point)],
            vec![
                code_dep(&code.always),
                code_dep(&code.counting),
                code_dep(&finalized_cell.out_point),
                code_dep(&config_cell.out_point),
                code_dep(&out_point(no_vote.hash, 0)),
            ],
            vec![],
            vec![output(
                200 * CKB,
                &challenger_lock,
                Some(counting_type.clone()),
            )],
            vec![Bytes::from(encoded(no_count.encode())?)],
        ),
    )?;
    let no_count_cell = output_cell(
        &no_count_commit,
        0,
        200 * CKB,
        &challenger_lock,
        Some(&counting_type),
        encoded(no_count.encode())?,
    );
    let result = ResultData {
        outcome: ProposalOutcome::RejectedByVote,
        proposal_id,
        requested_amount: finalized.requested_amount,
        receiver_lock_hash: finalized.receiver_lock_hash,
        yes: finalized.yes_amount,
        no: no_count.amount,
        final_state_hash: blake2b_256(&encoded(finalized.encode())?),
        proposal_config_data_hash: blake2b_256(&encoded(config.encode())?),
        veto_reason_hash: [0; 32],
    };
    let challenge_commit = rpc.commit(
        "challenge candidate",
        transaction(
            vec![
                input(&finalized_cell.out_point),
                input(&no_count_cell.out_point),
            ],
            vec![
                code_dep(&code.always),
                code_dep(&code.proposal),
                code_dep(&code.counting),
                code_dep(&code.policy),
                code_dep(&config_cell.out_point),
            ],
            vec![],
            vec![
                output(1_500 * CKB, &challenger_lock, Some(policy_type.clone())),
                output(200 * CKB, &challenger_lock, None),
            ],
            vec![Bytes::from(encoded(result.encode())?), Bytes::new()],
        ),
    )?;
    let result_out_point = out_point(challenge_commit.hash, 0);
    for (label, out_point, live) in [
        ("Finalized Proposal", &finalized_cell.out_point, false),
        ("NO Counting Cell", &no_count_cell.out_point, false),
        ("Rejected Result", &result_out_point, true),
    ] {
        let status = rpc.live_status(out_point)?;
        if (status == "live") != live {
            return Err(other(format!(
                "{label} status is {status}, expected live={live}"
            )));
        }
    }
    let claim_bond_commit = rpc.commit(
        "claim slashed Proposal bond",
        transaction(
            vec![input(&result_out_point)],
            vec![
                code_dep(&code.always),
                code_dep(&code.policy),
                code_dep(&config_cell.out_point),
            ],
            vec![],
            vec![output(1_500 * CKB, &challenger_lock, None)],
            vec![Bytes::new()],
        ),
    )?;
    let claimed_bond_out_point = out_point(claim_bond_commit.hash, 0);
    for (label, out_point, live) in [
        ("Claimed Rejected Result", &result_out_point, false),
        ("Challenger bond reward", &claimed_bond_out_point, true),
    ] {
        let status = rpc.live_status(out_point)?;
        if (status == "live") != live {
            return Err(other(format!(
                "{label} status is {status}, expected live={live}"
            )));
        }
    }
    let passed_report = run_passed_payout(
        &mut rpc,
        &code,
        &config,
        &config_cell,
        &treasury_config_cell,
        &treasury_cell,
        &proposal_lock,
        &proposer_lock,
        &yes_voter_lock,
        &receiver_lock,
        &dao_type,
        &policy_type,
        passed_proposal_funding,
        passed_counting_funding,
        passed_dao_funding,
        passed_vote_funding,
    )?;
    let report = json!({
        "status": "passed",
        "chain_directory": run_dir,
        "proposal_id": hex_hash(proposal_id),
        "yes": result.yes.to_string(),
        "no": result.no.to_string(),
        "outcome": "RejectedByVote",
        "transactions": {
            "proposal": commit_json(&proposal_commit),
            "yes_vote": commit_json(&yes_vote),
            "no_vote": commit_json(&no_vote),
            "yes_counting": commit_json(&yes_count_commit),
            "finalized_candidate": commit_json(&finalized_commit),
            "no_counting": commit_json(&no_count_commit),
            "challenge": commit_json(&challenge_commit),
            "claim_bond": commit_json(&claim_bond_commit),
        },
        "passed_payout": passed_report,
    });
    let report_path = run_dir.join("report.json");
    fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;
    println!("  report                   {}", report_path.display());
    println!("Hash-range Counting Cell live E2E PASSED");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_passed_payout(
    rpc: &mut Rpc,
    code: &CodeCells,
    config: &CountingConfig,
    config_cell: &CellRef,
    treasury_config_cell: &CellRef,
    treasury_cell: &CellRef,
    proposal_lock: &packed::Script,
    proposer_lock: &packed::Script,
    yes_voter_lock: &packed::Script,
    receiver_lock: &packed::Script,
    dao_type: &packed::Script,
    policy_type: &packed::Script,
    proposal_funding: CellRef,
    counting_funding: CellRef,
    dao_funding: CellRef,
    vote_funding: CellRef,
) -> AnyResult<Value> {
    let dao_commit = rpc.commit(
        "Passed YES DAO deposit",
        simple_transfer(
            &dao_funding,
            yes_voter_lock,
            Some(dao_type.clone()),
            Bytes::from(vec![0; 8]),
            vec![code_dep(&code.always), code_dep(&code.dao)],
        ),
    )?;
    let dao_cell = output_cell(
        &dao_commit,
        0,
        capacity(&dao_funding.output),
        yes_voter_lock,
        Some(dao_type),
        vec![0; 8],
    );
    let start_block = rpc.tip_number()? + 8;
    let end_block = start_block + 20;
    let proposal_input = input(&proposal_funding.out_point);
    let proposal_type = script(
        config.proposal_code_hash,
        config.proposal_hash_type,
        &type_id(&proposal_input, 0),
    );
    let proposal_id = packed_hash(&proposal_type.calc_script_hash());
    let open = ProposalData {
        phase: ProposalPhase::Open,
        start_block,
        end_block,
        challenge_period: config.minimum_challenge_period,
        minimum_vote_capacity: 100 * CKB,
        requested_amount: 100 * CKB,
        yes_amount: 0,
        yes_vote_count: 0,
        receiver_lock_hash: packed_hash(&receiver_lock.calc_script_hash()),
        proposer_lock_hash: packed_hash(&proposer_lock.calc_script_hash()),
        config_type_hash: packed_hash(
            &config_cell
                .output
                .type_()
                .to_opt()
                .ok_or_else(|| other("CountingConfig Cell has no type script"))?
                .calc_script_hash(),
        ),
        metadata_hash: blake2b_256(b"Passed Treasury payout live E2E"),
    };
    let proposal_commit = rpc.commit(
        "create Passed Proposal",
        transaction(
            vec![proposal_input],
            vec![
                code_dep(&code.always),
                code_dep(&code.proposal),
                code_dep(&config_cell.out_point),
            ],
            vec![],
            vec![output(
                capacity(&proposal_funding.output),
                proposal_lock,
                Some(proposal_type.clone()),
            )],
            vec![Bytes::from(encoded(open.encode())?)],
        ),
    )?;
    let open_cell = output_cell(
        &proposal_commit,
        0,
        capacity(&proposal_funding.output),
        proposal_lock,
        Some(&proposal_type),
        encoded(open.encode())?,
    );
    rpc.mine_to(start_block)?;
    let vote_type = script(config.vote_code_hash, config.vote_hash_type, &proposal_id);
    let proposal_header = rpc.block_hash(proposal_commit.block_number)?;
    let dao_header = rpc.block_hash(dao_commit.block_number)?;
    let vote_commit = submit_vote(
        rpc,
        "submit Passed YES vote",
        code,
        &vote_funding,
        yes_voter_lock,
        &vote_type,
        &open_cell,
        config_cell,
        &dao_cell,
        1,
        capacity(&dao_funding.output),
        vec![proposal_header, dao_header],
    )?;
    rpc.mine_to(end_block)?;

    let mut closed = open.clone();
    closed.phase = ProposalPhase::Closed;
    let end_header = rpc.block_hash(end_block)?;
    let close_commit = rpc.commit(
        "close Passed Proposal",
        transaction(
            vec![input(&open_cell.out_point)],
            vec![
                code_dep(&code.always),
                code_dep(&code.proposal),
                code_dep(&config_cell.out_point),
            ],
            vec![end_header],
            vec![output(
                capacity(&proposal_funding.output),
                proposal_lock,
                Some(proposal_type.clone()),
            )],
            vec![Bytes::from(encoded(closed.encode())?)],
        ),
    )?;
    let closed_cell = output_cell(
        &close_commit,
        0,
        capacity(&proposal_funding.output),
        proposal_lock,
        Some(&proposal_type),
        encoded(closed.encode())?,
    );
    let counting_type = script(
        config.counting_code_hash,
        config.counting_hash_type,
        &proposal_id,
    );
    let voter_lock_hash = packed_hash(&yes_voter_lock.calc_script_hash());
    let counting = CountingCellData {
        direction: 1,
        range_start: voter_lock_hash[0],
        range_end: voter_lock_hash[0],
        amount: capacity(&dao_funding.output) as u128,
        vote_count: 1,
    };
    let counting_commit = rpc.commit(
        "create Passed Counting Cell",
        transaction(
            vec![input(&counting_funding.out_point)],
            vec![
                code_dep(&code.always),
                code_dep(&code.counting),
                code_dep(&closed_cell.out_point),
                code_dep(&config_cell.out_point),
                code_dep(&out_point(vote_commit.hash, 0)),
            ],
            vec![],
            vec![output(
                capacity(&counting_funding.output),
                proposer_lock,
                Some(counting_type.clone()),
            )],
            vec![Bytes::from(encoded(counting.encode())?)],
        ),
    )?;
    let counting_cell = output_cell(
        &counting_commit,
        0,
        capacity(&counting_funding.output),
        proposer_lock,
        Some(&counting_type),
        encoded(counting.encode())?,
    );
    let mut finalized = closed.clone();
    finalized.phase = ProposalPhase::Finalized;
    finalized.yes_amount = counting.amount;
    finalized.yes_vote_count = 1;
    let finalized_commit = rpc.commit(
        "finalize Passed candidate",
        transaction(
            vec![
                input(&closed_cell.out_point),
                input(&counting_cell.out_point),
            ],
            vec![
                code_dep(&code.always),
                code_dep(&code.proposal),
                code_dep(&code.counting),
                code_dep(&config_cell.out_point),
            ],
            vec![],
            vec![
                output(
                    capacity(&proposal_funding.output),
                    proposal_lock,
                    Some(proposal_type.clone()),
                ),
                output(capacity(&counting_funding.output), proposer_lock, None),
            ],
            vec![Bytes::from(encoded(finalized.encode())?), Bytes::new()],
        ),
    )?;
    let finalized_cell = output_cell(
        &finalized_commit,
        0,
        capacity(&proposal_funding.output),
        proposal_lock,
        Some(&proposal_type),
        encoded(finalized.encode())?,
    );
    rpc.mine_to(finalized_commit.block_number + finalized.challenge_period)?;

    let result = ResultData {
        outcome: ProposalOutcome::Passed,
        proposal_id,
        requested_amount: finalized.requested_amount,
        receiver_lock_hash: finalized.receiver_lock_hash,
        yes: finalized.yes_amount,
        no: 0,
        final_state_hash: blake2b_256(&encoded(finalized.encode())?),
        proposal_config_data_hash: blake2b_256(&encoded(config.encode())?),
        veto_reason_hash: [0; 32],
    };
    let result_commit = rpc.commit(
        "settle Passed Proposal",
        transaction(
            vec![input_since(
                &finalized_cell.out_point,
                relative_block_since(finalized.challenge_period),
            )],
            vec![
                code_dep(&code.always),
                code_dep(&code.proposal),
                code_dep(&code.policy),
                code_dep(&config_cell.out_point),
            ],
            vec![],
            vec![output(
                capacity(&proposal_funding.output),
                proposer_lock,
                Some(policy_type.clone()),
            )],
            vec![Bytes::from(encoded(result.encode())?)],
        ),
    )?;
    let result_cell = output_cell(
        &result_commit,
        0,
        capacity(&proposal_funding.output),
        proposer_lock,
        Some(policy_type),
        encoded(result.encode())?,
    );
    let treasury_change = capacity(&treasury_cell.output)
        .checked_sub(result.requested_amount)
        .ok_or_else(|| other("Treasury Cell cannot cover Passed Proposal"))?;
    let payout_commit = rpc.commit(
        "pay Passed Proposal",
        transaction_with_witnesses(
            vec![
                input(&result_cell.out_point),
                input(&treasury_cell.out_point),
            ],
            vec![
                code_dep(&code.always),
                code_dep(&code.policy),
                code_dep(&code.treasury),
                code_dep(&config_cell.out_point),
                code_dep(&treasury_config_cell.out_point),
            ],
            vec![],
            vec![
                output(result.requested_amount, receiver_lock, None),
                output(treasury_change, &treasury_cell.output.lock(), None),
                output(capacity(&result_cell.output), proposer_lock, None),
            ],
            vec![Bytes::new(), Bytes::new(), Bytes::new()],
            vec![Bytes::new(), treasury_action_witness()],
        ),
    )?;
    let receiver_cell = out_point(payout_commit.hash, 0);
    let treasury_change_cell = out_point(payout_commit.hash, 1);
    for (label, out_point, live) in [
        ("Passed Result", &result_cell.out_point, false),
        ("Treasury input", &treasury_cell.out_point, false),
        ("Receiver payout", &receiver_cell, true),
        ("Treasury change", &treasury_change_cell, true),
    ] {
        let status = rpc.live_status(out_point)?;
        if (status == "live") != live {
            return Err(other(format!(
                "{label} status is {status}, expected live={live}"
            )));
        }
    }
    Ok(json!({
        "proposal_id": hex_hash(proposal_id),
        "requested_amount": result.requested_amount,
        "result_wire_version": encoded(result.encode())?[0],
        "transactions": {
            "proposal": commit_json(&proposal_commit),
            "vote": commit_json(&vote_commit),
            "counting": commit_json(&counting_commit),
            "candidate": commit_json(&finalized_commit),
            "result": commit_json(&result_commit),
            "treasury_payout": commit_json(&payout_commit),
        }
    }))
}

fn contract_binaries(workspace: &Path, ckb_repo: &Path) -> AnyResult<BTreeMap<String, PathBuf>> {
    let mut paths = BTreeMap::new();
    paths.insert(
        "always".to_owned(),
        ckb_repo.join("test/template/specs/cells/always_success"),
    );
    for (name, file) in [
        ("config", "counting-config-type-script"),
        ("proposal", "counting-proposal-type-script"),
        ("vote", "counting-vote-type-script"),
        ("counting", "counting-type-script"),
        ("policy", "counting-policy-type-script"),
    ] {
        paths.insert(name.to_owned(), workspace.join("build/release").join(file));
    }
    paths.insert(
        "treasury".to_owned(),
        workspace
            .parent()
            .ok_or_else(|| other("cannot locate impl directory"))?
            .join("build/release/treasury-lock-script"),
    );
    for path in paths.values() {
        require_file(path)?;
    }
    Ok(paths)
}

fn write_chain_spec(
    path: &Path,
    binaries: &BTreeMap<String, PathBuf>,
    config_type: &packed::Script,
    config: &CountingConfig,
    treasury_config_type: &packed::Script,
    treasury_config: &[u8],
    treasury_lock: &packed::Script,
    always_hash: Hash,
) -> AnyResult<()> {
    let mut spec = String::new();
    writeln!(spec, "name = \"counting_cell_live_e2e\"\n")?;
    spec.push_str(
        "[genesis]\n\
         version = 0\n\
         parent_hash = \"0x0000000000000000000000000000000000000000000000000000000000000000\"\n\
         timestamp = 0\n\
         compact_target = 0x20010000\n\
         uncles_hash = \"0x0000000000000000000000000000000000000000000000000000000000000000\"\n\
         nonce = \"0x0\"\n\n\
         [genesis.genesis_cell]\n\
         message = \"Counting Cell live E2E\"\n\n\
         [genesis.genesis_cell.lock]\n\
         code_hash = \"0xb35557e7e9854206f7bc13e3c3a7fa4cf8892c84a09237fb0aab40aab3771eee\"\n\
         args = \"0x\"\n\
         hash_type = \"data\"\n\n",
    );
    for (resource, create_type_id) in [
        ("specs/cells/secp256k1_blake160_sighash_all", true),
        ("specs/cells/dao", true),
        ("specs/cells/secp256k1_data", false),
        ("specs/cells/secp256k1_blake160_multisig_all", true),
    ] {
        writeln!(
            spec,
            "[[genesis.system_cells]]\nfile = {{ bundled = \"{resource}\" }}\ncreate_type_id = {create_type_id}"
        )?;
    }
    for name in [
        "always", "config", "proposal", "vote", "counting", "policy", "treasury",
    ] {
        writeln!(
            spec,
            "[[genesis.system_cells]]\nfile = {{ file = \"{}\" }}\ncreate_type_id = false",
            toml_path(binaries.get(name).unwrap())
        )?;
    }
    spec.push_str(
        "\n[genesis.system_cells_lock]\n\
         code_hash = \"0xb35557e7e9854206f7bc13e3c3a7fa4cf8892c84a09237fb0aab40aab3771eee\"\n\
         args = \"0x\"\n\
         hash_type = \"data\"\n\n\
         [[genesis.dep_groups]]\n\
         name = \"secp256k1_blake160_sighash_all\"\n\
         files = [\n\
           { bundled = \"specs/cells/secp256k1_data\" },\n\
           { bundled = \"specs/cells/secp256k1_blake160_sighash_all\" }\n\
         ]\n\n\
         [[genesis.dep_groups]]\n\
         name = \"secp256k1_blake160_multisig_all\"\n\
         files = [\n\
           { bundled = \"specs/cells/secp256k1_data\" },\n\
           { bundled = \"specs/cells/secp256k1_blake160_multisig_all\" }\n\
         ]\n\n\
         [genesis.bootstrap_lock]\n\
         code_hash = \"0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5\"\n\
         args = \"0x\"\n\
         hash_type = \"data\"\n\n",
    );
    let config_lock = script(always_hash, DATA_HASH_TYPE, &[0x40]);
    append_issued_typed(
        &mut spec,
        1_000 * CKB,
        &config_lock,
        config_type,
        &encoded(config.encode())?,
    )?;
    append_issued_typed(
        &mut spec,
        1_000 * CKB,
        &config_lock,
        treasury_config_type,
        treasury_config,
    )?;
    append_issued_plain(&mut spec, 5_000 * CKB, treasury_lock)?;
    for (capacity, args) in [
        (1_500 * CKB, 0x01),
        (200 * CKB, 0x01),
        (1_000 * CKB, 0x11),
        (500 * CKB, 0x11),
        (800 * CKB, 0x12),
        (500 * CKB, 0x12),
        (200 * CKB, 0x22),
        (1_600 * CKB, 0x01),
        (210 * CKB, 0x01),
        (1_100 * CKB, 0x11),
        (510 * CKB, 0x11),
    ] {
        append_issued_plain(
            &mut spec,
            capacity,
            &script(always_hash, DATA_HASH_TYPE, &[args]),
        )?;
    }
    spec.push_str(
        "\n[params]\n\
         initial_primary_epoch_reward = 1_917_808_21917808\n\
         secondary_epoch_reward = 613_698_63013698\n\
         max_block_cycles = 3_500_000_000\n\
         max_block_bytes = 597_000\n\
         cellbase_maturity = 0\n\
         primary_epoch_reward_halving_interval = 8760\n\
         epoch_duration_target = 14400\n\
         genesis_epoch_length = 10\n\n\
         [pow]\n\
         func = \"Dummy\"\n",
    );
    fs::write(path, spec)?;
    Ok(())
}

fn append_issued_plain(spec: &mut String, capacity: u64, lock: &packed::Script) -> AnyResult<()> {
    writeln!(spec, "[[genesis.issued_cells]]\ncapacity = {capacity}")?;
    append_script_fields(spec, "lock", lock)
}

fn append_issued_typed(
    spec: &mut String,
    capacity: u64,
    lock: &packed::Script,
    type_script: &packed::Script,
    data: &[u8],
) -> AnyResult<()> {
    writeln!(
        spec,
        "[[genesis.issued_cells]]\ncapacity = {capacity}\ndata = \"{}\"",
        hex_bytes(data)
    )?;
    append_script_fields(spec, "lock", lock)?;
    append_script_fields(spec, "type", type_script)
}

fn append_script_fields(spec: &mut String, prefix: &str, script: &packed::Script) -> AnyResult<()> {
    let hash_type = match script.hash_type().as_slice()[0] {
        DATA_HASH_TYPE => "data",
        TYPE_HASH_TYPE => "type",
        DATA1_HASH_TYPE => "data1",
        value => return Err(other(format!("unsupported script hash type {value}"))),
    };
    writeln!(
        spec,
        "{prefix}.code_hash = \"{}\"\n{prefix}.args = \"{}\"\n{prefix}.hash_type = \"{hash_type}\"",
        hex_bytes(script.code_hash().as_slice()),
        hex_bytes(&script.args().raw_data()),
    )?;
    Ok(())
}

fn initialize_node(
    ckb_bin: &Path,
    run_dir: &Path,
    spec: &Path,
    rpc_port: u16,
    p2p_port: u16,
) -> AnyResult<()> {
    let output = Command::new(ckb_bin)
        .args(["init", "-C"])
        .arg(run_dir)
        .args(["--chain", "dev", "--import-spec"])
        .arg(spec)
        .args([
            "--rpc-port",
            &rpc_port.to_string(),
            "--p2p-port",
            &p2p_port.to_string(),
            "--ba-arg",
            "0x",
            "--log-to",
            "file",
            "--force",
        ])
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(other(format!(
            "ckb init failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )))
    }
}

fn enable_integration_rpc(path: &Path) -> AnyResult<()> {
    let config = fs::read_to_string(path)?;
    let mut output = String::with_capacity(config.len());
    for line in config.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("modules = ") {
            output.push_str("modules = [\"Net\", \"Pool\", \"Miner\", \"Chain\", \"Experiment\", \"Stats\", \"IntegrationTest\"]\n");
        } else if trimmed.starts_with("min_fee_rate = ") {
            output.push_str("min_fee_rate = 0\n");
        } else {
            output.push_str(line);
            output.push('\n');
        }
    }
    fs::write(path, output)?;
    Ok(())
}

fn genesis_cells(transaction: &packed::Transaction) -> Vec<CellRef> {
    let tx_hash = packed_hash(&transaction.raw().calc_tx_hash());
    transaction
        .raw()
        .outputs()
        .into_iter()
        .zip(transaction.raw().outputs_data())
        .enumerate()
        .map(|(index, (output, data))| CellRef {
            out_point: out_point(tx_hash, index as u32),
            output,
            data: data.raw_data(),
        })
        .collect()
}

fn locate_code_cells(cells: &[CellRef], hashes: &BTreeMap<String, Hash>) -> AnyResult<CodeCells> {
    let find = |name: &str| -> AnyResult<packed::OutPoint> {
        cells
            .iter()
            .find(|cell| blake2b_256(&cell.data) == *hashes.get(name).unwrap())
            .map(|cell| cell.out_point.clone())
            .ok_or_else(|| other(format!("missing {name} code cell")))
    };
    Ok(CodeCells {
        always: find("always")?,
        dao: cells
            .get(2)
            .ok_or_else(|| other("missing DAO code cell"))?
            .out_point
            .clone(),
        proposal: find("proposal")?,
        vote: find("vote")?,
        counting: find("counting")?,
        policy: find("policy")?,
        treasury: find("treasury")?,
    })
}

fn find_cell(cells: &[CellRef], lock: &packed::Script, wanted: u64) -> AnyResult<CellRef> {
    cells
        .iter()
        .find(|cell| {
            cell.output.lock().as_slice() == lock.as_slice()
                && capacity(&cell.output) == wanted
                && cell.output.type_().to_opt().is_none()
                && cell.data.is_empty()
        })
        .cloned()
        .ok_or_else(|| other(format!("funding cell with capacity {wanted} is missing")))
}

#[allow(clippy::too_many_arguments)]
fn submit_vote(
    rpc: &mut Rpc,
    label: &str,
    code: &CodeCells,
    funding: &CellRef,
    voter_lock: &packed::Script,
    vote_type: &packed::Script,
    proposal: &CellRef,
    config: &CellRef,
    dao: &CellRef,
    direction: u8,
    amount: u64,
    headers: Vec<Hash>,
) -> AnyResult<Commit> {
    let vote = VoteData {
        direction,
        amount,
        dao_out_points: vec![common_out_point(&dao.out_point)],
    };
    rpc.commit(
        label,
        transaction(
            vec![input(&funding.out_point)],
            vec![
                code_dep(&code.always),
                code_dep(&code.vote),
                code_dep(&proposal.out_point),
                code_dep(&config.out_point),
                code_dep(&dao.out_point),
            ],
            headers,
            vec![output(
                capacity(&funding.output),
                voter_lock,
                Some(vote_type.clone()),
            )],
            vec![Bytes::from(encoded(vote.encode())?)],
        ),
    )
}

fn simple_transfer(
    funding: &CellRef,
    lock: &packed::Script,
    type_script: Option<packed::Script>,
    data: Bytes,
    deps: Vec<packed::CellDep>,
) -> packed::Transaction {
    transaction(
        vec![input(&funding.out_point)],
        deps,
        vec![],
        vec![output(capacity(&funding.output), lock, type_script)],
        vec![data],
    )
}

fn transaction(
    inputs: Vec<packed::CellInput>,
    deps: Vec<packed::CellDep>,
    headers: Vec<Hash>,
    outputs: Vec<packed::CellOutput>,
    outputs_data: Vec<Bytes>,
) -> packed::Transaction {
    let raw = packed::RawTransaction::new_builder()
        .cell_deps(deps.pack())
        .header_deps(headers.into_iter().map(|hash| hash.pack()).pack())
        .inputs(inputs.pack())
        .outputs(outputs.pack())
        .outputs_data(outputs_data.pack())
        .build();
    packed::Transaction::new_builder().raw(raw).build()
}

fn transaction_with_witnesses(
    inputs: Vec<packed::CellInput>,
    deps: Vec<packed::CellDep>,
    headers: Vec<Hash>,
    outputs: Vec<packed::CellOutput>,
    outputs_data: Vec<Bytes>,
    witnesses: Vec<Bytes>,
) -> packed::Transaction {
    transaction(inputs, deps, headers, outputs, outputs_data)
        .as_builder()
        .witnesses(witnesses.pack())
        .build()
}

fn output(
    capacity: u64,
    lock: &packed::Script,
    type_script: Option<packed::Script>,
) -> packed::CellOutput {
    packed::CellOutput::new_builder()
        .capacity(capacity)
        .lock(lock.clone())
        .type_(type_script.pack())
        .build()
}

fn output_cell(
    commit: &Commit,
    index: u32,
    capacity: u64,
    lock: &packed::Script,
    type_script: Option<&packed::Script>,
    data: Vec<u8>,
) -> CellRef {
    CellRef {
        out_point: out_point(commit.hash, index),
        output: output(capacity, lock, type_script.cloned()),
        data: Bytes::from(data),
    }
}

fn script(code_hash: Hash, hash_type: u8, args: &[u8]) -> packed::Script {
    packed::Script::new_builder()
        .code_hash(code_hash.pack())
        .hash_type(hash_type)
        .args(Bytes::copy_from_slice(args).pack())
        .build()
}

fn input(out_point: &packed::OutPoint) -> packed::CellInput {
    input_since(out_point, 0)
}

fn input_since(out_point: &packed::OutPoint, since: u64) -> packed::CellInput {
    packed::CellInput::new_builder()
        .previous_output(out_point.clone())
        .since(since)
        .build()
}

fn relative_block_since(blocks: u64) -> u64 {
    (1u64 << 63) | blocks
}

fn treasury_action_witness() -> Bytes {
    packed::WitnessArgs::new_builder()
        .lock(Some(Bytes::from(vec![1])).pack())
        .build()
        .as_bytes()
}

fn code_dep(out_point: &packed::OutPoint) -> packed::CellDep {
    packed::CellDep::new_builder()
        .out_point(out_point.clone())
        .build()
}

fn out_point(hash: Hash, index: u32) -> packed::OutPoint {
    packed::OutPoint::new_builder()
        .tx_hash(hash.pack())
        .index(index)
        .build()
}

fn common_out_point(out_point: &packed::OutPoint) -> OutPoint {
    OutPoint {
        tx_hash: packed_hash(&out_point.tx_hash()),
        index: out_point.index().unpack(),
    }
}

fn type_id(first_input: &packed::CellInput, output_index: u64) -> Hash {
    let mut preimage = first_input.as_slice().to_vec();
    preimage.extend_from_slice(&output_index.to_le_bytes());
    blake2b_256(&preimage)
}

fn encode_treasury_config(result_type_hash: Hash, zero_lock_hash: Hash) -> Vec<u8> {
    let mut output = Vec::with_capacity(97);
    output.push(2);
    output.extend_from_slice(&100u64.to_le_bytes());
    output.extend_from_slice(&CKB.to_le_bytes());
    output.extend_from_slice(&0u64.to_le_bytes());
    output.extend_from_slice(&CKB.to_le_bytes());
    output.extend_from_slice(&result_type_hash);
    output.extend_from_slice(&zero_lock_hash);
    output
}

fn capacity(output: &packed::CellOutput) -> u64 {
    output.capacity().unpack()
}

fn packed_hash(value: &packed::Byte32) -> Hash {
    value.as_slice().try_into().expect("packed hash length")
}

fn commit_json(commit: &Commit) -> Value {
    json!({
        "hash": hex_hash(commit.hash),
        "block_number": commit.block_number,
        "tx_index": commit.tx_index,
    })
}

fn free_port() -> AnyResult<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

fn hex_hash(hash: Hash) -> String {
    hex_bytes(&hash)
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(2 + bytes.len() * 2);
    output.push_str("0x");
    for byte in bytes {
        write!(output, "{byte:02x}").expect("writing to String");
    }
    output
}

fn toml_path(path: &Path) -> String {
    path.display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn require_file(path: &Path) -> AnyResult<()> {
    if path.is_file() {
        Ok(())
    } else {
        Err(other(format!(
            "required file is missing: {}",
            path.display()
        )))
    }
}

fn encoded(result: Result<Vec<u8>, CodecError>) -> AnyResult<Vec<u8>> {
    result.map_err(|error| other(format!("encode protocol data: {error:?}")))
}

fn other(message: impl Into<String>) -> Box<dyn Error> {
    Box::new(io::Error::other(message.into()))
}
