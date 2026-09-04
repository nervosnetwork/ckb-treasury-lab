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
use reqwest::{Url, blocking::Client};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tally_builder::{
    ChainBlock, ChainSource, CkbRpcClient, TallyBuilder, prove_block_transactions,
    prove_transaction,
};
use treasury_common::{
    Hash, OutPoint, ProposalConfig, ProposalData, ProposalOutcome, ProposalPhase, ResultData,
    TallyWitness, TreasuryConfig, VoteData, blake2b_256,
};

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
    tally: packed::OutPoint,
    policy: packed::OutPoint,
    treasury: packed::OutPoint,
}

struct NodeGuard {
    child: Child,
}

impl Drop for NodeGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
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
        parse_hex_u64(&value)
    }

    fn generate_block(&mut self) -> AnyResult<u64> {
        self.call::<JsonByte32>("generate_block", json!([]))?
            .ok_or_else(|| other("generate_block returned no hash"))?;
        self.tip_number()
    }

    fn packed_block(&mut self, number: u64) -> AnyResult<RpcBlock> {
        let bytes = self
            .call::<JsonBytes>(
                "get_block_by_number",
                json!([format!("0x{number:x}"), "0x0", false]),
            )?
            .ok_or_else(|| other(format!("block {number} was not found")))?;
        let bytes = bytes.into_bytes();
        if let Ok(block) = packed::BlockV1::from_slice(&bytes) {
            return Ok(RpcBlock {
                header: block.header(),
                transactions: block.transactions().into_iter().collect(),
            });
        }
        let block = packed::Block::from_slice(&bytes)
            .map_err(|_| other(format!("block {number} is not valid Molecule data")))?;
        Ok(RpcBlock {
            header: block.header(),
            transactions: block.transactions().into_iter().collect(),
        })
    }

    fn block_hash(&mut self, number: u64) -> AnyResult<Hash> {
        Ok(self
            .packed_block(number)?
            .header
            .calc_header_hash()
            .as_slice()
            .try_into()
            .expect("header hash length"))
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
                        "  {label:<28} block={number:<4} tx={} index={index}",
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
        let status = self.call::<Value>("get_transaction", json!([hex_hash(hash)]))?;
        Err(other(format!(
            "transaction {label} was not committed after 30 blocks; status={status:?}"
        )))
    }

    fn mine_to(&mut self, target: u64) -> AnyResult<()> {
        while self.tip_number()? < target {
            self.generate_block()?;
        }
        Ok(())
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

fn main() {
    if let Err(error) = run() {
        eprintln!("live E2E failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> AnyResult<()> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let impl_dir = manifest_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| other("cannot locate impl directory"))?
        .to_path_buf();
    let ckb_repo = env::var_os("CKB_REPO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/Users/yukang/code/ckb"));
    let ckb_bin = env::var_os("CKB_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| ckb_repo.join("target/debug/ckb"));
    require_file(&ckb_bin)?;

    let binaries = contract_binaries(&impl_dir, &ckb_repo)?;
    let code_hashes = binaries
        .iter()
        .map(|(name, path)| Ok((name.clone(), blake2b_256(&fs::read(path)?))))
        .collect::<AnyResult<BTreeMap<_, _>>>()?;
    if hex_hash(*code_hashes.get("always").expect("always binary")) != ALWAYS_SUCCESS_HASH {
        return Err(other("unexpected always-success binary hash"));
    }

    let always_code_hash = *code_hashes.get("always").unwrap();
    let config_code_hash = *code_hashes.get("config").unwrap();
    let policy_code_hash = *code_hashes.get("policy").unwrap();
    let treasury_code_hash = *code_hashes.get("treasury").unwrap();
    let proposal_config_type = script(config_code_hash, DATA1_HASH_TYPE, &[0x41; 32]);
    let treasury_config_type = script(config_code_hash, DATA1_HASH_TYPE, &[0x42; 32]);
    let policy_script = script(
        policy_code_hash,
        DATA1_HASH_TYPE,
        &packed_hash(&proposal_config_type.calc_script_hash()),
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
    let treasury_lock = script(
        treasury_code_hash,
        DATA1_HASH_TYPE,
        &packed_hash(&treasury_config_type.calc_script_hash()),
    );
    let zero_lock = script(always_code_hash, DATA_HASH_TYPE, &[0]);
    let candidate_lock = script(always_code_hash, DATA_HASH_TYPE, &[0x22]);
    let proposal_lock = script(always_code_hash, DATA_HASH_TYPE, &[0x23]);
    let guardian_lock = script(always_code_hash, DATA_HASH_TYPE, &[0x24]);
    let proposal_config = ProposalConfig {
        approval_bps: 6_000,
        minimum_total_votes: 2_000 * CKB as u128,
        maximum_proposal_amount: 1_000 * CKB,
        minimum_challenge_period: 5,
        minimum_tally_bond: 5_000 * CKB,
        treasury_lock_hash: packed_hash(&treasury_lock.calc_script_hash()),
        proposal_lock_hash: packed_hash(&proposal_lock.calc_script_hash()),
        guardian_lock_hash: packed_hash(&guardian_lock.calc_script_hash()),
        proposal_bond_burn_lock_hash: packed_hash(&zero_lock.calc_script_hash()),
        dao_code_hash: packed_hash(&dao_type.code_hash()),
        dao_hash_type: TYPE_HASH_TYPE,
        proposal_code_hash: *code_hashes.get("proposal").unwrap(),
        proposal_hash_type: DATA1_HASH_TYPE,
        vote_code_hash: *code_hashes.get("vote").unwrap(),
        vote_hash_type: DATA1_HASH_TYPE,
        tally_code_hash: *code_hashes.get("tally").unwrap(),
        tally_hash_type: DATA1_HASH_TYPE,
        candidate_lock_hash: packed_hash(&candidate_lock.calc_script_hash()),
        policy_type_hash: packed_hash(&policy_script.calc_script_hash()),
    };
    let treasury_config = TreasuryConfig {
        burn_expiry_blocks: 100,
        base_burn_incentive: 10 * CKB,
        burn_incentive_rate: 0,
        maximum_burn_incentive: 10 * CKB,
        result_type_hash: packed_hash(&policy_script.calc_script_hash()),
        zero_lock_hash: packed_hash(&zero_lock.calc_script_hash()),
    };

    let run_id = format!(
        "{}-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        std::process::id()
    );
    let run_dir = impl_dir.join("target/live-e2e").join(run_id);
    fs::create_dir_all(&run_dir)?;
    let source_spec = run_dir.join("source.toml");
    write_chain_spec(
        &source_spec,
        &binaries,
        &treasury_lock,
        &proposal_config_type,
        &proposal_config,
        &treasury_config_type,
        &treasury_config,
        always_code_hash,
    )?;

    let rpc_port = free_port()?;
    let p2p_port = free_port()?;
    initialize_node(&ckb_bin, &run_dir, &source_spec, rpc_port, p2p_port)?;
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
    let _guard = NodeGuard { child };
    let endpoint = format!("http://127.0.0.1:{rpc_port}");
    let mut rpc = Rpc::new(&endpoint)?;
    rpc.wait_ready()?;
    println!("V7 live-chain E2E");
    println!("  chain directory             {}", run_dir.display());
    println!("  RPC                         {endpoint}");

    let genesis = rpc.packed_block(0)?;
    let genesis_tx = genesis
        .transactions
        .first()
        .ok_or_else(|| other("genesis has no Cellbase transaction"))?;
    let genesis_hash = packed_hash(&genesis_tx.raw().calc_tx_hash());
    let cells = genesis_cells(genesis_tx);
    let code_cells = locate_code_cells(&cells, &code_hashes)?;
    let dao_code_cell = cells
        .get(2)
        .ok_or_else(|| other("genesis DAO code cell is missing"))?;
    let dao_code_type = dao_code_cell
        .output
        .type_()
        .to_opt()
        .ok_or_else(|| other("genesis DAO code cell has no Type ID"))?;
    let deployed_dao_type = script(
        packed_hash(&dao_code_type.calc_script_hash()),
        TYPE_HASH_TYPE,
        &[],
    );
    if deployed_dao_type != dao_type {
        return Err(other("genesis DAO Type Script differs from ProposalConfig"));
    }

    let proposer_lock = script(always_code_hash, DATA_HASH_TYPE, &[0x01]);
    let veto_proposer_lock = script(always_code_hash, DATA_HASH_TYPE, &[0x02]);
    let proposal_lock = script(always_code_hash, DATA_HASH_TYPE, &[0x23]);
    let guardian_lock = script(always_code_hash, DATA_HASH_TYPE, &[0x24]);
    let veto_operator1_lock = script(always_code_hash, DATA_HASH_TYPE, &[0x25]);
    let veto_operator2_lock = script(always_code_hash, DATA_HASH_TYPE, &[0x26]);
    let voter1_lock = script(always_code_hash, DATA_HASH_TYPE, &[0x11]);
    let voter2_lock = script(always_code_hash, DATA_HASH_TYPE, &[0x12]);
    let late_voter_lock = script(always_code_hash, DATA_HASH_TYPE, &[0x13]);
    let operator_lock = script(always_code_hash, DATA_HASH_TYPE, &[0x21]);
    let challenger_lock = script(always_code_hash, DATA_HASH_TYPE, &[0x22]);
    let receiver_lock = script(always_code_hash, DATA_HASH_TYPE, &[0x31]);

    let proposal_funding = find_cell(&cells, &proposer_lock, 1_500 * CKB)?;
    let veto_proposal_funding = find_cell(&cells, &veto_proposer_lock, 1_500 * CKB)?;
    let guardian_funding = find_cell(&cells, &guardian_lock, 100 * CKB)?;
    let veto_operator1_funding = find_cell(&cells, &veto_operator1_lock, 5_200 * CKB)?;
    let veto_operator2_funding = find_cell(&cells, &veto_operator2_lock, 5_300 * CKB)?;
    let dao1_funding = find_cell(&cells, &voter1_lock, 1_000 * CKB)?;
    let vote1_funding = find_cell(&cells, &voter1_lock, 500 * CKB)?;
    let dao2_funding = find_cell(&cells, &voter2_lock, 1_100 * CKB)?;
    let vote2_funding = find_cell(&cells, &voter2_lock, 500 * CKB)?;
    let late_dao_funding = find_cell(&cells, &late_voter_lock, 600 * CKB)?;
    let late_vote_funding = find_cell(&cells, &late_voter_lock, 550 * CKB)?;
    let omitted_bond_funding = find_cell(&cells, &operator_lock, 5_000 * CKB)?;
    let complete_bond_funding = find_cell(&cells, &operator_lock, 5_100 * CKB)?;
    let operator_auth_funding = find_cell(&cells, &operator_lock, 100 * CKB)?;
    let challenger_funding = find_cell(&cells, &challenger_lock, 100 * CKB)?;
    let proposal_config_cell = cells
        .iter()
        .find(|cell| cell.output.type_().to_opt().as_ref() == Some(&proposal_config_type))
        .cloned()
        .ok_or_else(|| other("policy config cell is missing"))?;
    let treasury_config_cell = cells
        .iter()
        .find(|cell| cell.output.type_().to_opt().as_ref() == Some(&treasury_config_type))
        .cloned()
        .ok_or_else(|| other("treasury config cell is missing"))?;

    rpc.mine_to(16)?;
    let treasury_cell = find_treasury_cell(&mut rpc, &treasury_lock, 100 * CKB)?;
    let treasury_index: u32 = treasury_cell.0.out_point.index().unpack();
    println!(
        "  consensus Treasury Cell     block={} capacity={} CKB out_point={}:{}",
        treasury_cell.1,
        capacity(&treasury_cell.0.output) / CKB,
        hex_hash(packed_hash(&treasury_cell.0.out_point.tx_hash())),
        treasury_index
    );

    let dao1 = rpc.commit(
        "DAO deposit voter 1",
        simple_transfer(
            &dao1_funding,
            &voter1_lock,
            Some(dao_type.clone()),
            Bytes::from(vec![0; 8]),
            vec![code_dep(&code_cells.always), code_dep(&code_cells.dao)],
        ),
    )?;
    let dao1_cell = output_cell(
        &dao1,
        0,
        1_000 * CKB,
        &voter1_lock,
        Some(&dao_type),
        vec![0; 8],
    );
    let dao2 = rpc.commit(
        "DAO deposit voter 2",
        simple_transfer(
            &dao2_funding,
            &voter2_lock,
            Some(dao_type.clone()),
            Bytes::from(vec![0; 8]),
            vec![code_dep(&code_cells.always), code_dep(&code_cells.dao)],
        ),
    )?;
    let dao2_cell = output_cell(
        &dao2,
        0,
        1_100 * CKB,
        &voter2_lock,
        Some(&dao_type),
        vec![0; 8],
    );

    let start_block = rpc.tip_number()? + 8;
    let end_block = start_block + 12;
    let proposal_input = input(&proposal_funding.out_point);
    let proposal_type_id = type_id(&proposal_input, 0);
    let proposal_type = script(
        *code_hashes.get("proposal").unwrap(),
        DATA1_HASH_TYPE,
        &proposal_type_id,
    );
    let proposal_id = packed_hash(&proposal_type.calc_script_hash());
    let open_proposal = ProposalData {
        phase: ProposalPhase::Open,
        start_block,
        end_block,
        challenge_period: 5,
        max_events_per_batch: 100,
        max_dao_deps_per_vote: 64,
        max_state_keys_per_batch: 4096,
        max_batch_sequence: 128,
        max_batch_witness_bytes: 500_000,
        minimum_vote_capacity: 500 * CKB,
        requested_amount: 100 * CKB,
        receiver_lock_hash: packed_hash(&receiver_lock.calc_script_hash()),
        proposer_lock_hash: packed_hash(&proposer_lock.calc_script_hash()),
        proposal_config_type_hash: packed_hash(&proposal_config_type.calc_script_hash()),
        metadata_hash: blake2b_256(b"live E2E proposal"),
    };
    let proposal_tx = transaction(
        vec![proposal_input],
        vec![
            code_dep(&code_cells.always),
            code_dep(&code_cells.proposal),
            code_dep(&proposal_config_cell.out_point),
        ],
        vec![],
        vec![output(
            1_500 * CKB,
            &proposal_lock,
            Some(proposal_type.clone()),
        )],
        vec![Bytes::from(open_proposal.encode())],
        vec![],
    );
    let proposal_commit = rpc.commit("create proposal", proposal_tx)?;
    let open_proposal_cell = output_cell(
        &proposal_commit,
        0,
        1_500 * CKB,
        &proposal_lock,
        Some(&proposal_type),
        open_proposal.encode(),
    );

    let late_dao = rpc.commit(
        "DAO deposit after proposal",
        simple_transfer(
            &late_dao_funding,
            &late_voter_lock,
            Some(dao_type.clone()),
            Bytes::from(vec![0; 8]),
            vec![code_dep(&code_cells.always), code_dep(&code_cells.dao)],
        ),
    )?;
    let late_dao_cell = output_cell(
        &late_dao,
        0,
        600 * CKB,
        &late_voter_lock,
        Some(&dao_type),
        vec![0; 8],
    );

    rpc.mine_to(start_block)?;
    let vote_type = script(
        *code_hashes.get("vote").unwrap(),
        DATA1_HASH_TYPE,
        &proposal_id,
    );
    let proposal_creation_header = rpc.block_hash(proposal_commit.block_number)?;
    let late_dao_header = rpc.block_hash(late_dao.block_number)?;
    let late_vote_tx = vote_transaction(
        &code_cells,
        &late_vote_funding,
        &late_voter_lock,
        &vote_type,
        &open_proposal_cell,
        &proposal_config_cell,
        &late_dao_cell,
        600 * CKB,
        vec![proposal_creation_header, late_dao_header],
    )?;
    let late_vote_rejection = rpc
        .submit(late_vote_tx)
        .expect_err("a DAO deposit created after its proposal must not vote");
    if !late_vote_rejection.to_string().contains("error code 20") {
        return Err(other(format!(
            "late DAO vote failed for an unexpected reason: {late_vote_rejection}"
        )));
    }
    println!("  late DAO vote              rejected with error code 20");

    let dao1_header = rpc.block_hash(dao1.block_number)?;
    let vote1 = submit_vote(
        &mut rpc,
        "vote yes voter 1",
        &code_cells,
        &vote1_funding,
        &voter1_lock,
        &vote_type,
        &open_proposal_cell,
        &proposal_config_cell,
        &dao1_cell,
        1_000 * CKB,
        vec![proposal_creation_header, dao1_header],
    )?;
    let dao2_header = rpc.block_hash(dao2.block_number)?;
    let vote2 = submit_vote(
        &mut rpc,
        "vote yes voter 2",
        &code_cells,
        &vote2_funding,
        &voter2_lock,
        &vote_type,
        &open_proposal_cell,
        &proposal_config_cell,
        &dao2_cell,
        1_100 * CKB,
        vec![proposal_creation_header, dao2_header],
    )?;
    if vote1.block_number > end_block || vote2.block_number > end_block {
        return Err(other("a vote was committed after the voting window"));
    }
    let dao1_spend = rpc.commit(
        "spend DAO deposit after vote",
        transaction(
            vec![input(&dao1_cell.out_point)],
            vec![code_dep(&code_cells.always), code_dep(&code_cells.dao)],
            vec![dao1_header],
            vec![output(1_000 * CKB, &voter1_lock, Some(dao_type.clone()))],
            vec![Bytes::from(dao1.block_number.to_le_bytes().to_vec())],
            vec![],
        ),
    )?;
    if dao1_spend.block_number > end_block {
        return Err(other("the DAO spend was committed after the voting window"));
    }
    rpc.mine_to(end_block)?;

    let mut closed_proposal = open_proposal.clone();
    closed_proposal.phase = ProposalPhase::Closed;
    let close_tx = transaction(
        vec![input(&open_proposal_cell.out_point)],
        vec![code_dep(&code_cells.always), code_dep(&code_cells.proposal)],
        vec![rpc.block_hash(end_block)?],
        vec![output(
            1_500 * CKB,
            &proposal_lock,
            Some(proposal_type.clone()),
        )],
        vec![Bytes::from(closed_proposal.encode())],
        vec![],
    );
    let close_commit = rpc.commit("close proposal", close_tx)?;
    let closed_proposal_cell = output_cell(
        &close_commit,
        0,
        1_500 * CKB,
        &proposal_lock,
        Some(&proposal_type),
        closed_proposal.encode(),
    );

    let chain_source = CkbRpcClient::new_direct(&endpoint)?;
    let voting_blocks = (start_block..=end_block)
        .map(|number| chain_source.block_by_number(number))
        .collect::<Result<Vec<_>, _>>()?;
    let omitted_session = create_tally_session(
        &mut rpc,
        "create omitted tally",
        &code_cells,
        *code_hashes.get("tally").unwrap(),
        &omitted_bond_funding,
        &operator_lock,
        &closed_proposal_cell,
        &closed_proposal,
        proposal_id,
        &proposal_config_cell,
        proposal_config,
    )?;
    let (omitted_tally_type, omitted_tally_cell, mut omitted_builder) = omitted_session;
    let (omitted_blocks, omitted_headers) =
        prove_selected_votes(&voting_blocks, &[vote1.hash], end_block)?;
    let (omitted_candidate, omitted_batch) = map_builder(
        omitted_builder.build_batch(
            &closed_proposal,
            omitted_blocks,
            end_block + 1,
            0,
            end_block,
        ),
        "build omitted tally batch",
    )?;
    let omitted_advance = tally_advance_tx(
        &code_cells,
        &closed_proposal_cell,
        &proposal_config_cell,
        &omitted_tally_cell,
        &candidate_lock,
        &omitted_tally_type,
        omitted_candidate.encode(),
        omitted_headers,
        TallyWitness::Advance(omitted_batch),
    )?;
    let omitted_candidate_commit = rpc.commit("submit omitted candidate", omitted_advance)?;
    let omitted_candidate_cell = output_cell(
        &omitted_candidate_commit,
        0,
        capacity(&omitted_tally_cell.output),
        &candidate_lock,
        Some(&omitted_tally_type),
        omitted_candidate.encode(),
    );
    drop(omitted_builder);

    let omitted_vote_block = voting_blocks
        .iter()
        .find(|block| block.block_number == vote2.block_number)
        .ok_or_else(|| other("omitted vote block was not fetched"))?;
    let omitted_proof = prove_transaction(
        vote2.block_number,
        0,
        &omitted_vote_block.raw_transactions,
        omitted_vote_block.witnesses_root,
        vote2.tx_index,
    )
    .map_err(|error| other(format!("build omitted vote proof: {error:?}")))?;
    let mut challenge_witness = map_builder(
        TallyBuilder::build_omitted_vote_challenge_from_candidate(
            proposal_id,
            &closed_proposal,
            proposal_config,
            &chain_source,
            OutPoint {
                tx_hash: omitted_candidate_commit.hash,
                index: 0,
            },
            omitted_proof,
        ),
        "replay candidate and build omitted-vote challenge",
    )?;
    let mut challenge_deps = vec![
        code_dep(&code_cells.always),
        code_dep(&code_cells.tally),
        code_dep(&closed_proposal_cell.out_point),
        code_dep(&proposal_config_cell.out_point),
    ];
    challenge_deps.extend(vote_event_deps(&mut challenge_witness, 4)?);
    let challenge_tx = transaction(
        vec![
            input(&omitted_candidate_cell.out_point),
            input(&challenger_funding.out_point),
        ],
        challenge_deps,
        vec![omitted_vote_block.block_hash],
        vec![
            output(
                capacity(&omitted_candidate_cell.output),
                &challenger_lock,
                None,
            ),
            output(capacity(&challenger_funding.output), &challenger_lock, None),
        ],
        vec![Bytes::new(), Bytes::new()],
        vec![input_type_witness(challenge_witness.encode().map_err(
            |error| other(format!("encode challenge witness: {error:?}")),
        )?)],
    );
    let challenge_commit = rpc.commit("challenge omitted vote", challenge_tx)?;
    let challenged_status = rpc.live_status(&omitted_candidate_cell.out_point)?;
    if challenged_status == "live" {
        return Err(other("challenged candidate remained live"));
    }

    let complete_session = create_tally_session(
        &mut rpc,
        "create complete tally",
        &code_cells,
        *code_hashes.get("tally").unwrap(),
        &complete_bond_funding,
        &operator_lock,
        &closed_proposal_cell,
        &closed_proposal,
        proposal_id,
        &proposal_config_cell,
        proposal_config,
    )?;
    let (complete_tally_type, complete_tally_cell, mut complete_builder) = complete_session;
    let scanned = map_builder(
        complete_builder.scan_blocks(&closed_proposal, &voting_blocks),
        "scan complete voting window",
    )?;
    let (complete_candidate, complete_batch) = map_builder(
        complete_builder.build_batch(
            &closed_proposal,
            scanned.blocks,
            scanned.end_block,
            scanned.end_tx_index,
            scanned.candidate_since,
        ),
        "build complete tally batch",
    )?;
    if complete_candidate.yes != 2_100 * CKB as u128 || complete_candidate.no != 0 {
        return Err(other(format!(
            "unexpected complete tally yes={} no={}",
            complete_candidate.yes, complete_candidate.no
        )));
    }
    let complete_advance = tally_advance_tx(
        &code_cells,
        &closed_proposal_cell,
        &proposal_config_cell,
        &complete_tally_cell,
        &candidate_lock,
        &complete_tally_type,
        complete_candidate.encode(),
        scanned.header_deps,
        TallyWitness::Advance(complete_batch),
    )?;
    let complete_candidate_commit = rpc.commit("submit complete candidate", complete_advance)?;
    let complete_candidate_cell = output_cell(
        &complete_candidate_commit,
        0,
        capacity(&complete_tally_cell.output),
        &candidate_lock,
        Some(&complete_tally_type),
        complete_candidate.encode(),
    );

    let challenge_deadline = complete_candidate_commit
        .block_number
        .checked_add(closed_proposal.challenge_period)
        .ok_or_else(|| other("challenge deadline overflow"))?;
    rpc.mine_to(challenge_deadline)?;
    let passed = proposal_config.passes(
        complete_candidate.yes,
        complete_candidate.no,
        closed_proposal.requested_amount,
    );
    if !passed {
        return Err(other(
            "the complete tally did not pass the configured policy",
        ));
    }
    let result_data = ResultData {
        outcome: if passed {
            ProposalOutcome::Passed
        } else {
            ProposalOutcome::RejectedByVote
        },
        proposal_id,
        requested_amount: closed_proposal.requested_amount,
        receiver_lock_hash: closed_proposal.receiver_lock_hash,
        yes: complete_candidate.yes,
        no: complete_candidate.no,
        final_state_hash: blake2b_256(&complete_candidate.encode()),
        proposal_config_data_hash: blake2b_256(&proposal_config.encode()),
        veto_reason_hash: [0; 32],
    };
    let finalize_tx = transaction(
        vec![
            input(&closed_proposal_cell.out_point),
            input_since(
                &complete_candidate_cell.out_point,
                0x8000_0000_0000_0000 | closed_proposal.challenge_period,
            ),
            input(&operator_auth_funding.out_point),
        ],
        vec![
            code_dep(&code_cells.always),
            code_dep(&code_cells.proposal),
            code_dep(&code_cells.tally),
            code_dep(&code_cells.policy),
            code_dep(&proposal_config_cell.out_point),
        ],
        vec![],
        vec![
            output(1_500 * CKB, &proposer_lock, Some(policy_script.clone())),
            output(
                capacity(&complete_candidate_cell.output),
                &operator_lock,
                None,
            ),
            output(
                capacity(&operator_auth_funding.output),
                &operator_lock,
                None,
            ),
        ],
        vec![
            Bytes::from(result_data.encode()),
            Bytes::new(),
            Bytes::new(),
        ],
        vec![
            Bytes::new(),
            input_type_witness(
                TallyWitness::Finalize
                    .encode()
                    .map_err(|error| other(format!("encode finalize witness: {error:?}")))?,
            ),
        ],
    );
    let finalize_commit = rpc.commit("finalize passed proposal", finalize_tx)?;
    let result_cell = output_cell(
        &finalize_commit,
        0,
        1_500 * CKB,
        &proposer_lock,
        Some(&policy_script),
        result_data.encode(),
    );

    let treasury_capacity = capacity(&treasury_cell.0.output);
    let treasury_change = treasury_capacity - closed_proposal.requested_amount;
    let payout_tx = transaction(
        vec![
            input(&treasury_cell.0.out_point),
            input(&result_cell.out_point),
        ],
        vec![
            code_dep(&code_cells.always),
            code_dep(&code_cells.treasury),
            code_dep(&code_cells.policy),
            code_dep(&treasury_config_cell.out_point),
            code_dep(&proposal_config_cell.out_point),
        ],
        vec![],
        vec![
            output(closed_proposal.requested_amount, &receiver_lock, None),
            output(treasury_change, &treasury_lock, None),
            output(1_500 * CKB, &proposer_lock, None),
        ],
        vec![Bytes::new(), Bytes::new(), Bytes::new()],
        vec![lock_witness(vec![1]), Bytes::new()],
    );
    let payout_commit = rpc.commit("pay Treasury grant", payout_tx)?;
    let receiver_out_point = out_point(payout_commit.hash, 0);
    let change_out_point = out_point(payout_commit.hash, 1);
    for (label, point, expected_live) in [
        ("old Treasury Cell", &treasury_cell.0.out_point, false),
        ("Result Cell", &result_cell.out_point, false),
        ("receiver payout", &receiver_out_point, true),
        ("Treasury change", &change_out_point, true),
    ] {
        let actual = rpc.live_status(point)?;
        if (actual == "live") != expected_live {
            return Err(other(format!(
                "{label} status is {actual}, expected live={expected_live}"
            )));
        }
    }

    let veto_start_block = rpc.tip_number()? + 3;
    let veto_end_block = veto_start_block + 3;
    let veto_proposal_input = input(&veto_proposal_funding.out_point);
    let veto_proposal_type = script(
        *code_hashes.get("proposal").unwrap(),
        DATA1_HASH_TYPE,
        &type_id(&veto_proposal_input, 0),
    );
    let veto_proposal_id = packed_hash(&veto_proposal_type.calc_script_hash());
    let veto_open_proposal = ProposalData {
        phase: ProposalPhase::Open,
        start_block: veto_start_block,
        end_block: veto_end_block,
        challenge_period: 5,
        max_events_per_batch: 100,
        max_dao_deps_per_vote: 64,
        max_state_keys_per_batch: 4096,
        max_batch_sequence: 128,
        max_batch_witness_bytes: 500_000,
        minimum_vote_capacity: 500 * CKB,
        requested_amount: 100 * CKB,
        receiver_lock_hash: packed_hash(&receiver_lock.calc_script_hash()),
        proposer_lock_hash: packed_hash(&veto_proposer_lock.calc_script_hash()),
        proposal_config_type_hash: packed_hash(&proposal_config_type.calc_script_hash()),
        metadata_hash: blake2b_256(b"live E2E guardian-veto proposal"),
    };
    let veto_proposal_tx = transaction(
        vec![veto_proposal_input],
        vec![
            code_dep(&code_cells.always),
            code_dep(&code_cells.proposal),
            code_dep(&proposal_config_cell.out_point),
        ],
        vec![],
        vec![output(
            1_500 * CKB,
            &proposal_lock,
            Some(veto_proposal_type.clone()),
        )],
        vec![Bytes::from(veto_open_proposal.encode())],
        vec![],
    );
    let veto_proposal_commit = rpc.commit("create veto proposal", veto_proposal_tx)?;
    let veto_open_proposal_cell = output_cell(
        &veto_proposal_commit,
        0,
        1_500 * CKB,
        &proposal_lock,
        Some(&veto_proposal_type),
        veto_open_proposal.encode(),
    );

    rpc.mine_to(veto_end_block)?;
    let mut veto_closed_proposal = veto_open_proposal.clone();
    veto_closed_proposal.phase = ProposalPhase::Closed;
    let veto_close_tx = transaction(
        vec![input(&veto_open_proposal_cell.out_point)],
        vec![code_dep(&code_cells.always), code_dep(&code_cells.proposal)],
        vec![rpc.block_hash(veto_end_block)?],
        vec![output(
            1_500 * CKB,
            &proposal_lock,
            Some(veto_proposal_type.clone()),
        )],
        vec![Bytes::from(veto_closed_proposal.encode())],
        vec![],
    );
    let veto_close_commit = rpc.commit("close veto proposal", veto_close_tx)?;
    let veto_closed_proposal_cell = output_cell(
        &veto_close_commit,
        0,
        1_500 * CKB,
        &proposal_lock,
        Some(&veto_proposal_type),
        veto_closed_proposal.encode(),
    );

    let (veto_tally1_type, veto_tally1_active, mut veto_tally1_builder) = create_tally_session(
        &mut rpc,
        "create veto tally 1",
        &code_cells,
        *code_hashes.get("tally").unwrap(),
        &veto_operator1_funding,
        &veto_operator1_lock,
        &veto_closed_proposal_cell,
        &veto_closed_proposal,
        veto_proposal_id,
        &proposal_config_cell,
        proposal_config,
    )?;
    let (_veto_tally2_type, veto_tally2_active, _veto_tally2_builder) = create_tally_session(
        &mut rpc,
        "create veto tally 2",
        &code_cells,
        *code_hashes.get("tally").unwrap(),
        &veto_operator2_funding,
        &veto_operator2_lock,
        &veto_closed_proposal_cell,
        &veto_closed_proposal,
        veto_proposal_id,
        &proposal_config_cell,
        proposal_config,
    )?;

    let veto_voting_blocks = (veto_start_block..=veto_end_block)
        .map(|number| chain_source.block_by_number(number))
        .collect::<Result<Vec<_>, _>>()?;
    let veto_scan = map_builder(
        veto_tally1_builder.scan_blocks(&veto_closed_proposal, &veto_voting_blocks),
        "scan veto proposal voting window",
    )?;
    let (veto_candidate, veto_batch) = map_builder(
        veto_tally1_builder.build_batch(
            &veto_closed_proposal,
            veto_scan.blocks,
            veto_scan.end_block,
            veto_scan.end_tx_index,
            veto_scan.candidate_since,
        ),
        "build veto proposal candidate",
    )?;
    let veto_candidate_tx = tally_advance_tx(
        &code_cells,
        &veto_closed_proposal_cell,
        &proposal_config_cell,
        &veto_tally1_active,
        &candidate_lock,
        &veto_tally1_type,
        veto_candidate.encode(),
        veto_scan.header_deps,
        TallyWitness::Advance(veto_batch),
    )?;
    let veto_candidate_commit = rpc.commit("submit veto candidate", veto_candidate_tx)?;
    let veto_candidate_cell = output_cell(
        &veto_candidate_commit,
        0,
        capacity(&veto_tally1_active.output),
        &candidate_lock,
        Some(&veto_tally1_type),
        veto_candidate.encode(),
    );
    let veto_challenge_deadline = veto_candidate_commit
        .block_number
        .checked_add(veto_closed_proposal.challenge_period)
        .ok_or_else(|| other("veto challenge deadline overflow"))?;
    rpc.mine_to(veto_challenge_deadline)?;

    let vetoed_result_data = ResultData {
        outcome: ProposalOutcome::Vetoed,
        proposal_id: veto_proposal_id,
        requested_amount: veto_closed_proposal.requested_amount,
        receiver_lock_hash: veto_closed_proposal.receiver_lock_hash,
        yes: 0,
        no: 0,
        final_state_hash: [0; 32],
        proposal_config_data_hash: blake2b_256(&proposal_config.encode()),
        veto_reason_hash: blake2b_256(b"live E2E malicious proposal"),
    };
    let veto_tx = transaction(
        vec![
            input(&veto_closed_proposal_cell.out_point),
            input(&guardian_funding.out_point),
        ],
        vec![
            code_dep(&code_cells.always),
            code_dep(&code_cells.proposal),
            code_dep(&code_cells.policy),
            code_dep(&proposal_config_cell.out_point),
        ],
        vec![],
        vec![
            output(1_500 * CKB, &zero_lock, Some(policy_script.clone())),
            output(capacity(&guardian_funding.output), &guardian_lock, None),
        ],
        vec![Bytes::from(vetoed_result_data.encode()), Bytes::new()],
        vec![],
    );
    let veto_commit = rpc.commit("guardian veto proposal", veto_tx)?;
    let vetoed_result_cell = output_cell(
        &veto_commit,
        0,
        1_500 * CKB,
        &zero_lock,
        Some(&policy_script),
        vetoed_result_data.encode(),
    );

    let stale_result_data = ResultData {
        outcome: ProposalOutcome::RejectedByVote,
        proposal_id: veto_proposal_id,
        requested_amount: veto_closed_proposal.requested_amount,
        receiver_lock_hash: veto_closed_proposal.receiver_lock_hash,
        yes: veto_candidate.yes,
        no: veto_candidate.no,
        final_state_hash: blake2b_256(&veto_candidate.encode()),
        proposal_config_data_hash: blake2b_256(&proposal_config.encode()),
        veto_reason_hash: [0; 32],
    };
    let stale_settlement_tx = transaction(
        vec![
            input(&veto_closed_proposal_cell.out_point),
            input_since(
                &veto_candidate_cell.out_point,
                0x8000_0000_0000_0000 | veto_closed_proposal.challenge_period,
            ),
        ],
        vec![
            code_dep(&code_cells.always),
            code_dep(&code_cells.proposal),
            code_dep(&code_cells.tally),
            code_dep(&code_cells.policy),
            code_dep(&proposal_config_cell.out_point),
        ],
        vec![],
        vec![
            output(
                1_500 * CKB,
                &veto_proposer_lock,
                Some(policy_script.clone()),
            ),
            output(
                capacity(&veto_candidate_cell.output),
                &veto_operator1_lock,
                None,
            ),
        ],
        vec![Bytes::from(stale_result_data.encode()), Bytes::new()],
        vec![
            Bytes::new(),
            input_type_witness(
                TallyWitness::Finalize
                    .encode()
                    .map_err(|error| other(format!("encode stale finalize witness: {error:?}")))?,
            ),
        ],
    );
    let stale_settlement_rejection = rpc
        .submit(stale_settlement_tx)
        .expect_err("settlement using a veto-consumed Proposal Cell must fail")
        .to_string();

    let consume_vetoed_result_tx = transaction(
        vec![input(&vetoed_result_cell.out_point)],
        vec![
            code_dep(&code_cells.always),
            code_dep(&code_cells.policy),
            code_dep(&proposal_config_cell.out_point),
        ],
        vec![],
        vec![output(1_500 * CKB, &guardian_lock, None)],
        vec![Bytes::new()],
        vec![],
    );
    let vetoed_result_spend_rejection = rpc
        .submit(consume_vetoed_result_tx)
        .expect_err("a Vetoed Result Cell must be immutable")
        .to_string();

    let veto_candidate_cleanup = rpc.commit(
        "refund veto tally 1",
        tally_cleanup_vetoed_tx(
            &code_cells,
            &proposal_config_cell,
            &vetoed_result_cell,
            &veto_candidate_cell,
            &veto_operator1_lock,
        )?,
    )?;
    let veto_active_cleanup = rpc.commit(
        "refund veto tally 2",
        tally_cleanup_vetoed_tx(
            &code_cells,
            &proposal_config_cell,
            &vetoed_result_cell,
            &veto_tally2_active,
            &veto_operator2_lock,
        )?,
    )?;
    let veto_operator1_refund = out_point(veto_candidate_cleanup.hash, 0);
    let veto_operator2_refund = out_point(veto_active_cleanup.hash, 0);
    for (label, point, expected_live) in [
        (
            "veto-consumed Proposal Cell",
            &veto_closed_proposal_cell.out_point,
            false,
        ),
        (
            "cleaned Candidate TallyChainCell",
            &veto_candidate_cell.out_point,
            false,
        ),
        (
            "cleaned Active TallyChainCell",
            &veto_tally2_active.out_point,
            false,
        ),
        ("veto operator 1 refund", &veto_operator1_refund, true),
        ("veto operator 2 refund", &veto_operator2_refund, true),
        (
            "immutable Vetoed Result",
            &vetoed_result_cell.out_point,
            true,
        ),
    ] {
        let actual = rpc.live_status(point)?;
        if (actual == "live") != expected_live {
            return Err(other(format!(
                "{label} status is {actual}, expected live={expected_live}"
            )));
        }
    }

    let report = json!({
        "status": "passed",
        "node_binary": ckb_bin,
        "chain_directory": run_dir,
        "rpc": endpoint,
        "genesis_tx": hex_hash(genesis_hash),
        "treasury": {
            "source_block": treasury_cell.1,
            "input_capacity": treasury_capacity,
            "requested_amount": closed_proposal.requested_amount,
            "change_capacity": treasury_change,
        },
        "voting_window": { "start": start_block, "end": end_block },
        "tally": {
            "yes": complete_candidate.yes.to_string(),
            "no": complete_candidate.no.to_string(),
            "passed": passed,
            "challenge_deadline": challenge_deadline,
        },
        "guardian_veto": {
            "voting_window": {
                "start": veto_start_block,
                "end": veto_end_block,
            },
            "candidate_challenge_deadline": veto_challenge_deadline,
            "proposal_bond_burned": 1_500 * CKB,
            "operator_1_refund": capacity(&veto_candidate_cell.output),
            "operator_2_refund": capacity(&veto_tally2_active.output),
            "stale_settlement_rejection": stale_settlement_rejection,
            "vetoed_result_spend_rejection": vetoed_result_spend_rejection,
        },
        "transactions": {
            "dao_deposit_1": commit_json(&dao1),
            "dao_deposit_2": commit_json(&dao2),
            "dao_spend_after_vote": commit_json(&dao1_spend),
            "late_dao_deposit": commit_json(&late_dao),
            "proposal": commit_json(&proposal_commit),
            "vote_1": commit_json(&vote1),
            "vote_2": commit_json(&vote2),
            "proposal_close": commit_json(&close_commit),
            "omitted_candidate": commit_json(&omitted_candidate_commit),
            "omitted_vote_challenge": commit_json(&challenge_commit),
            "complete_candidate": commit_json(&complete_candidate_commit),
            "finalize": commit_json(&finalize_commit),
            "payout": commit_json(&payout_commit),
            "veto_proposal": commit_json(&veto_proposal_commit),
            "veto_proposal_close": commit_json(&veto_close_commit),
            "veto_candidate": commit_json(&veto_candidate_commit),
            "guardian_veto": commit_json(&veto_commit),
            "veto_candidate_cleanup": commit_json(&veto_candidate_cleanup),
            "veto_active_cleanup": commit_json(&veto_active_cleanup),
        },
        "assertions": [
            "Treasury Cell was created by a mined Cellbase transaction",
            "a DAO deposit created after the Proposal Cell was rejected for voting",
            "spending a DAO deposit after voting did not revoke its vote",
            "omitted-vote candidate was accepted and then consumed by a valid challenge",
            "complete tally counted both DAO deposits",
            "policy produced a passed Result Cell",
            "payout consumed both Result and Treasury inputs",
            "receiver payout and Treasury change remain live",
            "Guardian veto consumed the singleton Proposal Cell after a Candidate matured",
            "a stale settlement using the consumed Proposal Cell was rejected",
            "the Vetoed Result Cell is immutable and keeps authorizing tally cleanup",
            "Candidate and Active tally bonds were refunded exactly to distinct operators",
        ],
    });
    let report_path = run_dir.join("report.json");
    fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;
    println!("  report                      {}", report_path.display());
    println!("V7 live-chain E2E PASSED");
    Ok(())
}

fn contract_binaries(impl_dir: &Path, ckb_repo: &Path) -> AnyResult<BTreeMap<String, PathBuf>> {
    let mut paths = BTreeMap::new();
    paths.insert(
        "always".to_owned(),
        ckb_repo.join("test/template/specs/cells/always_success"),
    );
    for (name, file) in [
        ("config", "config-type-script"),
        ("proposal", "proposal-type-script"),
        ("vote", "vote-type-script"),
        ("tally", "tally-type-script"),
        ("policy", "policy-type-script"),
        ("treasury", "treasury-lock-script"),
        ("grant", "grant-lock-script"),
    ] {
        paths.insert(name.to_owned(), impl_dir.join("build/release").join(file));
    }
    for path in paths.values() {
        require_file(path)?;
    }
    Ok(paths)
}

#[allow(clippy::too_many_arguments)]
fn write_chain_spec(
    path: &Path,
    binaries: &BTreeMap<String, PathBuf>,
    treasury_lock: &packed::Script,
    proposal_config_type: &packed::Script,
    proposal_config: &ProposalConfig,
    treasury_config_type: &packed::Script,
    treasury_config: &TreasuryConfig,
    always_code_hash: Hash,
) -> AnyResult<()> {
    let mut spec = String::new();
    writeln!(spec, "name = \"ckb_treasury_live_e2e\"\n")?;
    spec.push_str(
        "[genesis]\n\
         version = 0\n\
         parent_hash = \"0x0000000000000000000000000000000000000000000000000000000000000000\"\n\
         timestamp = 0\n\
         compact_target = 0x20010000\n\
         uncles_hash = \"0x0000000000000000000000000000000000000000000000000000000000000000\"\n\
         nonce = \"0x0\"\n\n\
         [genesis.genesis_cell]\n\
         message = \"CKB Treasury V7 live E2E\"\n\n\
         [genesis.genesis_cell.lock]\n\
         code_hash = \"0xb35557e7e9854206f7bc13e3c3a7fa4cf8892c84a09237fb0aab40aab3771eee\"\n\
         args = \"0x\"\n\
         hash_type = \"data\"\n\n",
    );
    for (resource, type_id) in [
        ("specs/cells/secp256k1_blake160_sighash_all", true),
        ("specs/cells/dao", true),
        ("specs/cells/secp256k1_data", false),
        ("specs/cells/secp256k1_blake160_multisig_all", true),
    ] {
        writeln!(
            spec,
            "[[genesis.system_cells]]\nfile = {{ bundled = \"{resource}\" }}\ncreate_type_id = {type_id}"
        )?;
    }
    for name in [
        "always", "config", "proposal", "vote", "tally", "policy", "treasury", "grant",
    ] {
        let binary = binaries.get(name).expect("known binary");
        writeln!(
            spec,
            "[[genesis.system_cells]]\nfile = {{ file = \"{}\" }}\ncreate_type_id = false",
            toml_path(binary)
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
    append_issued_typed(
        &mut spec,
        1_000 * CKB,
        &script(always_code_hash, DATA_HASH_TYPE, &[0x40]),
        proposal_config_type,
        &proposal_config.encode(),
    )?;
    append_issued_typed(
        &mut spec,
        1_000 * CKB,
        &script(always_code_hash, DATA_HASH_TYPE, &[0x40]),
        treasury_config_type,
        &treasury_config.encode(),
    )?;
    for (capacity, args) in [
        (1_500 * CKB, 0x01),
        (1_500 * CKB, 0x02),
        (1_000 * CKB, 0x11),
        (500 * CKB, 0x11),
        (1_100 * CKB, 0x12),
        (500 * CKB, 0x12),
        (600 * CKB, 0x13),
        (550 * CKB, 0x13),
        (5_000 * CKB, 0x21),
        (5_100 * CKB, 0x21),
        (100 * CKB, 0x21),
        (100 * CKB, 0x22),
        (100 * CKB, 0x24),
        (5_200 * CKB, 0x25),
        (5_300 * CKB, 0x26),
    ] {
        append_issued_plain(
            &mut spec,
            capacity,
            &script(always_code_hash, DATA_HASH_TYPE, &[args]),
        )?;
    }
    spec.push_str(
        "\n[params]\n\
         initial_primary_epoch_reward = 1_917_808_21917808\n\
         secondary_epoch_reward = 613_698_63013698\n\
         max_block_cycles = 10_000_000_000\n\
         max_block_bytes = 2_000_000\n\
         cellbase_maturity = 0\n\
         primary_epoch_reward_halving_interval = 8760\n\
         epoch_duration_target = 14400\n\
         genesis_epoch_length = 10\n\n\
         [params.treasury]\n\
         activation_block_number = 1\n\
         emission_interval = 1\n",
    );
    append_script_fields(&mut spec, "lock", treasury_lock)?;
    spec.push_str("\n[pow]\nfunc = \"Dummy\"\n");
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
    source_spec: &Path,
    rpc_port: u16,
    p2p_port: u16,
) -> AnyResult<()> {
    let output = Command::new(ckb_bin)
        .args(["init", "-C"])
        .arg(run_dir)
        .args(["--chain", "dev", "--import-spec"])
        .arg(source_spec)
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
    if !output.status.success() {
        return Err(other(format!(
            "ckb init failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
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
        let expected = hashes
            .get(name)
            .ok_or_else(|| other(format!("missing {name} code hash")))?;
        cells
            .iter()
            .find(|cell| blake2b_256(&cell.data) == *expected)
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
        tally: find("tally")?,
        policy: find("policy")?,
        treasury: find("treasury")?,
    })
}

fn find_cell(cells: &[CellRef], lock: &packed::Script, wanted_capacity: u64) -> AnyResult<CellRef> {
    cells
        .iter()
        .find(|cell| {
            cell.output.lock().as_slice() == lock.as_slice()
                && capacity(&cell.output) == wanted_capacity
                && cell.output.type_().to_opt().is_none()
                && cell.data.is_empty()
        })
        .cloned()
        .ok_or_else(|| {
            other(format!(
                "funding cell with capacity {wanted_capacity} is missing"
            ))
        })
}

fn find_treasury_cell(
    rpc: &mut Rpc,
    treasury_lock: &packed::Script,
    minimum_capacity: u64,
) -> AnyResult<(CellRef, u64)> {
    let tip = rpc.tip_number()?;
    for number in 1..=tip {
        let block = rpc.packed_block(number)?;
        let Some(cellbase) = block.transactions.first() else {
            continue;
        };
        let tx_hash = packed_hash(&cellbase.raw().calc_tx_hash());
        for (index, (output, data)) in cellbase
            .raw()
            .outputs()
            .into_iter()
            .zip(cellbase.raw().outputs_data())
            .enumerate()
        {
            if output.lock().as_slice() == treasury_lock.as_slice()
                && capacity(&output) >= minimum_capacity
                && output.type_().to_opt().is_none()
                && data.raw_data().is_empty()
            {
                return Ok((
                    CellRef {
                        out_point: out_point(tx_hash, index as u32),
                        output,
                        data: Bytes::new(),
                    },
                    number,
                ));
            }
        }
    }
    Err(other("no consensus-created Treasury Cell was found"))
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
    proposal_config: &CellRef,
    dao: &CellRef,
    amount: u64,
    headers: Vec<Hash>,
) -> AnyResult<Commit> {
    let tx = vote_transaction(
        code,
        funding,
        voter_lock,
        vote_type,
        proposal,
        proposal_config,
        dao,
        amount,
        headers,
    )?;
    rpc.commit(label, tx)
}

#[allow(clippy::too_many_arguments)]
fn vote_transaction(
    code: &CodeCells,
    funding: &CellRef,
    voter_lock: &packed::Script,
    vote_type: &packed::Script,
    proposal: &CellRef,
    proposal_config: &CellRef,
    dao: &CellRef,
    amount: u64,
    headers: Vec<Hash>,
) -> AnyResult<packed::Transaction> {
    let vote_data = VoteData {
        direction: 1,
        amount,
        dao_out_points: vec![common_out_point(&dao.out_point)],
    }
    .encode()
    .map_err(|error| other(format!("encode vote: {error:?}")))?;
    Ok(transaction(
        vec![input(&funding.out_point)],
        vec![
            code_dep(&code.always),
            code_dep(&code.vote),
            code_dep(&proposal.out_point),
            code_dep(&proposal_config.out_point),
            code_dep(&dao.out_point),
        ],
        headers,
        vec![output(
            capacity(&funding.output),
            voter_lock,
            Some(vote_type.clone()),
        )],
        vec![Bytes::from(vote_data)],
        vec![],
    ))
}

#[allow(clippy::too_many_arguments)]
fn create_tally_session(
    rpc: &mut Rpc,
    label: &str,
    code: &CodeCells,
    tally_code_hash: Hash,
    funding: &CellRef,
    operator_lock: &packed::Script,
    proposal_cell: &CellRef,
    proposal: &ProposalData,
    proposal_id: Hash,
    proposal_config_cell: &CellRef,
    proposal_config: ProposalConfig,
) -> AnyResult<(packed::Script, CellRef, TallyBuilder)> {
    let tally_input = input(&funding.out_point);
    let tally_type = script(tally_code_hash, DATA1_HASH_TYPE, &type_id(&tally_input, 0));
    let operator_hash = packed_hash(&operator_lock.calc_script_hash());
    let builder = TallyBuilder::new(
        proposal_id,
        operator_hash,
        proposal.start_block,
        proposal_config,
    );
    let initial = builder.state().clone();
    let tx = transaction(
        vec![tally_input],
        vec![
            code_dep(&code.always),
            code_dep(&code.tally),
            code_dep(&proposal_cell.out_point),
            code_dep(&proposal_config_cell.out_point),
        ],
        vec![],
        vec![output(
            capacity(&funding.output),
            operator_lock,
            Some(tally_type.clone()),
        )],
        vec![Bytes::from(initial.encode())],
        vec![],
    );
    let committed = rpc.commit(label, tx)?;
    let cell = output_cell(
        &committed,
        0,
        capacity(&funding.output),
        operator_lock,
        Some(&tally_type),
        initial.encode(),
    );
    Ok((tally_type, cell, builder))
}

fn prove_selected_votes(
    blocks: &[ChainBlock],
    included_hashes: &[Hash],
    anchor_block: u64,
) -> AnyResult<(Vec<treasury_common::ProvenBlock>, Vec<Hash>)> {
    let mut proven = Vec::new();
    let mut header_deps = Vec::new();
    for block in blocks {
        let indices = block
            .raw_transactions
            .iter()
            .enumerate()
            .filter_map(|(index, raw)| {
                let tx = packed::RawTransaction::from_slice(raw).ok()?;
                let hash = packed_hash(&tx.calc_tx_hash());
                included_hashes.contains(&hash).then_some(index as u32)
            })
            .collect::<Vec<_>>();
        if indices.is_empty() {
            continue;
        }
        let header_index = push_header(&mut header_deps, block.block_hash)?;
        proven.push(
            prove_block_transactions(
                block.block_number,
                header_index,
                &block.raw_transactions,
                block.witnesses_root,
                &indices,
            )
            .map_err(|error| other(format!("prove selected votes: {error:?}")))?,
        );
    }
    let anchor = blocks
        .iter()
        .find(|block| block.block_number == anchor_block)
        .ok_or_else(|| other("anchor block was not fetched"))?;
    push_header(&mut header_deps, anchor.block_hash)?;
    Ok((proven, header_deps))
}

fn push_header(headers: &mut Vec<Hash>, hash: Hash) -> AnyResult<u16> {
    if let Some(index) = headers.iter().position(|existing| *existing == hash) {
        return index
            .try_into()
            .map_err(|_| other("too many header dependencies"));
    }
    let index = headers
        .len()
        .try_into()
        .map_err(|_| other("too many header dependencies"))?;
    headers.push(hash);
    Ok(index)
}

#[allow(clippy::too_many_arguments)]
fn tally_advance_tx(
    code: &CodeCells,
    proposal: &CellRef,
    proposal_config: &CellRef,
    tally: &CellRef,
    output_lock: &packed::Script,
    tally_type: &packed::Script,
    output_data: Vec<u8>,
    header_deps: Vec<Hash>,
    mut witness: TallyWitness,
) -> AnyResult<packed::Transaction> {
    let mut deps = vec![
        code_dep(&code.always),
        code_dep(&code.tally),
        code_dep(&proposal.out_point),
        code_dep(&proposal_config.out_point),
    ];
    deps.extend(vote_event_deps(&mut witness, 4)?);
    Ok(transaction(
        vec![input(&tally.out_point)],
        deps,
        header_deps,
        vec![output(
            capacity(&tally.output),
            output_lock,
            Some(tally_type.clone()),
        )],
        vec![Bytes::from(output_data)],
        vec![input_type_witness(witness.encode().map_err(|error| {
            other(format!("encode tally witness: {error:?}"))
        })?)],
    ))
}

fn tally_cleanup_vetoed_tx(
    code: &CodeCells,
    proposal_config: &CellRef,
    vetoed_result: &CellRef,
    tally: &CellRef,
    operator_lock: &packed::Script,
) -> AnyResult<packed::Transaction> {
    Ok(transaction(
        vec![input(&tally.out_point)],
        vec![
            code_dep(&code.always),
            code_dep(&code.tally),
            code_dep(&proposal_config.out_point),
            code_dep(&vetoed_result.out_point),
        ],
        vec![],
        vec![output(capacity(&tally.output), operator_lock, None)],
        vec![Bytes::new()],
        vec![input_type_witness(
            TallyWitness::CleanupVetoed
                .encode()
                .map_err(|error| other(format!("encode veto cleanup witness: {error:?}")))?,
        )],
    ))
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
        vec![],
    )
}

fn transaction(
    inputs: Vec<packed::CellInput>,
    deps: Vec<packed::CellDep>,
    headers: Vec<Hash>,
    outputs: Vec<packed::CellOutput>,
    outputs_data: Vec<Bytes>,
    witnesses: Vec<Bytes>,
) -> packed::Transaction {
    let raw = packed::RawTransaction::new_builder()
        .cell_deps(deps.pack())
        .header_deps(headers.into_iter().map(|hash| hash.pack()).pack())
        .inputs(inputs.pack())
        .outputs(outputs.pack())
        .outputs_data(outputs_data.pack())
        .build();
    packed::Transaction::new_builder()
        .raw(raw)
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
    packed::CellInput::new_builder()
        .previous_output(out_point.clone())
        .build()
}

fn input_since(out_point: &packed::OutPoint, since: u64) -> packed::CellInput {
    packed::CellInput::new_builder()
        .since(since)
        .previous_output(out_point.clone())
        .build()
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

fn vote_event_deps(
    witness: &mut TallyWitness,
    first_dep_index: u16,
) -> AnyResult<Vec<packed::CellDep>> {
    let votes = match witness {
        TallyWitness::Advance(batch) => batch
            .blocks
            .iter_mut()
            .flat_map(|block| &mut block.events)
            .filter_map(|event| event.vote.as_mut())
            .collect::<Vec<_>>(),
        TallyWitness::ChallengeVote { omitted, .. } => omitted.vote.iter_mut().collect(),
        _ => Vec::new(),
    };
    votes
        .into_iter()
        .enumerate()
        .map(|(offset, vote)| {
            vote.cell_dep_index = first_dep_index
                .checked_add(
                    offset
                        .try_into()
                        .map_err(|_| other("too many VoteEventCell dependencies"))?,
                )
                .ok_or_else(|| other("VoteEventCell dependency index overflow"))?;
            Ok(code_dep(&out_point(
                vote.vote_cell.tx_hash,
                vote.vote_cell.index,
            )))
        })
        .collect()
}

fn type_id(first_input: &packed::CellInput, output_index: u64) -> Hash {
    let mut preimage = first_input.as_slice().to_vec();
    preimage.extend_from_slice(&output_index.to_le_bytes());
    blake2b_256(&preimage)
}

fn input_type_witness(data: Vec<u8>) -> Bytes {
    packed::WitnessArgs::new_builder()
        .input_type(Some(Bytes::from(data)).pack())
        .build()
        .as_bytes()
}

fn lock_witness(data: Vec<u8>) -> Bytes {
    packed::WitnessArgs::new_builder()
        .lock(Some(Bytes::from(data)).pack())
        .build()
        .as_bytes()
}

fn capacity(output: &packed::CellOutput) -> u64 {
    output.capacity().unpack()
}

fn packed_hash(value: &packed::Byte32) -> Hash {
    value.as_slice().try_into().expect("packed hash length")
}

fn map_builder<T, E: std::fmt::Debug>(result: Result<T, E>, context: &str) -> AnyResult<T> {
    result.map_err(|error| other(format!("{context}: {error:?}")))
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

fn parse_hex_u64(value: &str) -> AnyResult<u64> {
    u64::from_str_radix(value.strip_prefix("0x").unwrap_or(value), 16).map_err(Into::into)
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

fn other(message: impl Into<String>) -> Box<dyn Error> {
    Box::new(io::Error::other(message.into()))
}
