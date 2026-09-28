/**
 * `e2e`: the whole voting lifecycle against a local devnet.
 *
 * The command starts the devnet itself - `ckb run` and `ckb miner` with the
 * working directory set to `devnet/` - and then walks through the "Quick start
 * on a local devnet" section of `sdk/README.md`:
 *
 *   deploy -> create-config -> dao-deposit -> create-proposal -> vote ->
 *   create-counting -> finalize-proposal -> pass-proposal
 */

import { execFileSync, spawn } from "node:child_process";
import {
  closeSync,
  existsSync,
  mkdirSync,
  openSync,
  writeFileSync,
} from "node:fs";
import { dirname, resolve } from "node:path";
import { Command } from "commander";
import { ccc } from "@ckb-ccc/shell";
import { buildClient } from "../../client.js";
import {
  loadConfig,
  patchConfigFile,
  type DeploymentConfig,
} from "../../config.js";
import { ProposalStatus } from "../../constants.js";
import { createConfigCell } from "../../config-cell.js";
import { createCountingCell } from "../../counting.js";
import { depositDao } from "../../dao.js";
import { deployScripts, readContractBinaries } from "../../deploy.js";
import { deriveKnownScripts } from "../../devnet.js";
import {
  createProposal,
  finalizeProposal,
  passProposal,
} from "../../proposal.js";
import { getProposalCell } from "../../query.js";
import { formatOutPoint, waitForTransaction } from "../../utils.js";
import { castVote } from "../../vote.js";

/** The private key of the first genesis account of the devnet. */
const DEVNET_PRIVATE_KEY =
  "0xd00c06bfd800d27397002dca6fb0993d5ba6399b4238b2f29ee9deb97593d2bc";

/** Config the run installs, as required by `docs/prompt/e2e.md`. */
const VOTE_DURATION = 5;
const VOTE_WINDOW = 5;
const CHALLENGE_TIME = 1;

/** Fee rate of every transaction, in shannons per KB. */
const FEE_RATE = 1500n;

const CKB = 100_000_000n;
/** "YES" shannons a proposal needs to be finalized. */
const YES_THRESHOLD = 100n * CKB;
/** Minimum bond of a proposal. */
const MINIMAL_PROPOSAL_CAPACITY = 200n * CKB;
/** Minimum `vote_amount` of a vote cell. */
const MINIMAL_VOTE_AMOUNT = 100n * CKB;
/** Capacity of the DAO deposit that backs the vote. */
const DEPOSIT_AMOUNT = 1_000n * CKB;
/** Grant requested by the proposal. */
const REQUESTED_AMOUNT = 50n * CKB;

/**
 * How long the command waits for a `ckb` process to come up.
 *
 * Strictly below ten seconds, as the prompt requires; the devnet loads its
 * chain and mines its first block well within that.
 */
const START_TIMEOUT_MS = 9_000;

/** The first try plus two restarts, after which the run reports an error. */
const START_ATTEMPTS = 3;

/** How long a restart waits for the old processes to exit. */
const STOP_GRACE_MS = 10_000;

/** Options of the `e2e` command. */
interface E2eOptions {
  config: string;
  rpcUrl: string;
  keepRunning?: boolean;
}

/** A `ckb` process of the devnet. */
interface DevnetProcess {
  pid: number;
  role: "run" | "miner";
}

/** Whether this run started (or took over) the devnet and must stop it. */
let ownsDevnet = false;

/** The root of the repository (`solution1/`), derived from this file. */
function repoRoot(): string {
  return resolve(import.meta.dir, "..", "..", "..", "..");
}

/** The devnet directory holding `ckb.toml` and the data. */
function devnetDirectory(): string {
  return process.env.CKB_DEVNET_DIR
    ? resolve(process.env.CKB_DEVNET_DIR)
    : resolve(repoRoot(), "devnet");
}

/** The `ckb` program of the devnet. */
function devnetBinary(): string {
  return process.env.CKB_DEVNET_BIN
    ? resolve(process.env.CKB_DEVNET_BIN)
    : resolve(devnetDirectory(), "ckb");
}

/** Sleeps for `ms`; used as a polling interval, never as a fixed wait. */
function delay(ms: number): Promise<void> {
  return new Promise((done) => setTimeout(done, ms));
}

/**
 * Lists the `ckb run` / `ckb miner` processes of the devnet.
 *
 * The command line is matched instead of a pid file: the binary name is what
 * identifies the process, whatever relative path it was started with.
 */
function listDevnetProcesses(): DevnetProcess[] {
  let output: string;
  try {
    output = execFileSync("ps", ["-eo", "pid=,args="], { encoding: "utf8" });
  } catch {
    return [];
  }
  const pattern = /(?:^|[/\s])(?:\.\/)?ckb\s+(run|miner)(?:\s|$)/;
  const processes: DevnetProcess[] = [];
  for (const line of output.split("\n")) {
    const match = pattern.exec(line);
    if (!match) {
      continue;
    }
    const pid = Number.parseInt(line.trim().split(/\s+/)[0] ?? "", 10);
    if (Number.isInteger(pid)) {
      processes.push({ pid, role: match[1] as DevnetProcess["role"] });
    }
  }
  return processes;
}

/** Sends `signal` to `pid`, ignoring a process that already exited. */
function killQuietly(pid: number, signal: NodeJS.Signals): void {
  try {
    process.kill(pid, signal);
  } catch {
    // The process is gone already.
  }
}

/**
 * Starts `ckb run` or `ckb miner` from the devnet directory.
 *
 * The working directory matters: `ckb.toml` (and with it `specs/dev.toml` and
 * the `data` directory) is resolved relative to it, which is what keeps the
 * node on the local dev chain.
 */
function startCkb(role: DevnetProcess["role"]): void {
  const logDirectory = resolve(devnetDirectory(), "data", "logs");
  mkdirSync(logDirectory, { recursive: true });
  const logPath = resolve(logDirectory, `e2e-${role}.log`);
  const fd = openSync(logPath, "a");
  const child = spawn(devnetBinary(), [role], {
    cwd: devnetDirectory(),
    stdio: ["ignore", fd, fd],
  });
  closeSync(fd);
  // The processes outlive a step, so the parent must not be kept alive by their
  // handles; they are shut down explicitly on the way out.
  child.unref();
  child.on("error", (error) => {
    console.error(
      `Error: cannot run ${devnetBinary()} ${role}: ${error.message}`,
    );
  });
  console.log(
    `  starting ${devnetBinary()} ${role} (cwd ${devnetDirectory()})`,
  );
}

/** A synchronous best-effort shutdown, for the `exit` event. */
function stopDevnetSync(): void {
  if (!ownsDevnet) {
    return;
  }
  for (const { pid } of listDevnetProcesses()) {
    killQuietly(pid, "SIGTERM");
  }
}

/** Gracefully stops every `ckb` process of the devnet. */
async function stopDevnet(graceMs = STOP_GRACE_MS): Promise<void> {
  if (listDevnetProcesses().length === 0) {
    return;
  }
  for (const { pid } of listDevnetProcesses()) {
    killQuietly(pid, "SIGTERM");
  }
  const deadline = Date.now() + graceMs;
  while (listDevnetProcesses().length > 0 && Date.now() < deadline) {
    await delay(300);
  }
  for (const { pid } of listDevnetProcesses()) {
    killQuietly(pid, "SIGKILL");
  }
}

/**
 * Polls the node until it reports a consistent tip.
 *
 * The RPC socket answers before the chain service has finished loading, so the
 * tip is only trusted once the matching block can be read back.
 */
async function waitForReadyTip(
  client: ccc.Client,
  timeoutMs = START_TIMEOUT_MS,
): Promise<bigint> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    try {
      const tip = await client.getTip();
      const block = await client.getBlockByNumber(tip);
      if (block && block.header.number === tip) {
        return tip;
      }
    } catch {
      // The node is not ready yet.
    }
    if (Date.now() > deadline) {
      throw new Error(
        `the ckb node did not become ready within ${timeoutMs} ms`,
      );
    }
    await delay(250);
  }
}

/** Polls the tip until the chain has grown past `from`. */
async function waitForNewBlock(
  client: ccc.Client,
  from: bigint,
  timeoutMs = START_TIMEOUT_MS,
): Promise<bigint> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const tip = await client.getTip();
    if (tip > from) {
      return tip;
    }
    if (Date.now() > deadline) {
      throw new Error(
        `the ckb miner did not produce a block within ${timeoutMs} ms`,
      );
    }
    await delay(500);
  }
}

/** Starts a node and a miner and waits until the chain is moving. */
async function startDevnetOnce(client: ccc.Client): Promise<void> {
  if (!listDevnetProcesses().some(({ role }) => role === "run")) {
    startCkb("run");
  }
  // Wait for the node before starting the miner: the miner polls its RPC.
  const tipAtStart = await waitForReadyTip(client);

  if (!listDevnetProcesses().some(({ role }) => role === "miner")) {
    startCkb("miner");
  }
  const tip = await waitForNewBlock(client, tipAtStart);
  console.log(`  devnet ready, tip is block ${tip}`);
}

/**
 * Starts the devnet, restarting it when a process does not come up in time.
 *
 * The run needs both processes, so a failed attempt kills whatever was started
 * and tries again; after {@link START_ATTEMPTS} attempts the error is reported
 * to the caller, which quits.
 */
async function ensureDevnet(client: ccc.Client): Promise<void> {
  ownsDevnet = true;
  let lastError: unknown;
  for (let attempt = 1; attempt <= START_ATTEMPTS; attempt++) {
    try {
      await startDevnetOnce(client);
      return;
    } catch (error) {
      lastError = error;
      const message = error instanceof Error ? error.message : String(error);
      if (attempt === START_ATTEMPTS) {
        break;
      }
      console.log(
        `  the devnet did not start (attempt ${attempt}/${START_ATTEMPTS}): ` +
          `${message}`,
      );
      console.log("  restarting the ckb processes...");
      await stopDevnet();
    }
  }
  throw new Error(
    `cannot start the devnet after ${START_ATTEMPTS} attempts: ` +
      `${lastError instanceof Error ? lastError.message : String(lastError)}`,
  );
}

/** Writes a deployment config, formatting big integers as numbers. */
function writeConfig(path: string, config: DeploymentConfig): void {
  mkdirSync(dirname(path), { recursive: true });
  const json = JSON.stringify(
    config,
    (_key, value) => (typeof value === "bigint" ? Number(value) : value),
    2,
  );
  writeFileSync(path, `${json}\n`);
}

/** Prints `[i/n] title` for each lifecycle step. */
class Steps {
  private index = 0;

  constructor(private readonly total: number) {}

  start(title: string): void {
    this.index += 1;
    console.log(`\n[${this.index}/${this.total}] ${title}`);
  }

  info(message: string): void {
    console.log(`      ${message}`);
  }
}

const STEP_COUNT = 9;

/** Runs the whole lifecycle; throws on the first failure. */
async function runE2e(options: E2eOptions): Promise<void> {
  const configPath = resolve(options.config);
  const binariesDirectory = resolve(repoRoot(), "build", "release");
  const steps = new Steps(STEP_COUNT);

  console.log("CKB voting system end-to-end run");
  console.log(`  config:  ${configPath}`);
  console.log(`  rpc:     ${options.rpcUrl}`);

  if (!existsSync(devnetBinary())) {
    throw new Error(
      `the ckb program was not found at ${devnetBinary()}; the e2e run needs ` +
        `the devnet binaries under ${devnetDirectory()}`,
    );
  }

  const rpcClient = buildClient({ rpcUrl: options.rpcUrl });
  await ensureDevnet(rpcClient);

  steps.start("derive the devnet known scripts from block 0");
  const knownScripts = await deriveKnownScripts(rpcClient);
  const client = buildClient({
    rpcUrl: options.rpcUrl,
    knownScripts,
  });
  for (const name of [
    ccc.KnownScript.Secp256k1Blake160,
    ccc.KnownScript.NervosDao,
  ]) {
    if (!knownScripts[name]) {
      throw new Error(
        `the genesis block does not declare the ${name} script; the devnet ` +
          `was not started from ${devnetDirectory()}/ckb.toml`,
      );
    }
  }
  steps.info(`found ${Object.keys(knownScripts).length} genesis scripts`);

  if (!existsSync(binariesDirectory)) {
    throw new Error(
      `the compiled contracts were not found at ${binariesDirectory}; run ` +
        `"make build" in the repository root first`,
    );
  }

  steps.start("deploy the five contracts (hash_type data2)");
  const signer = new ccc.SignerCkbPrivateKey(client, DEVNET_PRIVATE_KEY);
  const deployed = await deployScripts(
    signer,
    readContractBinaries(binariesDirectory),
    { feeRate: FEE_RATE },
  );
  await waitForTransaction(client, deployed.txHash);
  const deployment: DeploymentConfig = {
    rpcUrl: options.rpcUrl,
    feeRate: Number(FEE_RATE),
    knownScripts,
    scripts: deployed.scripts,
  };
  writeConfig(configPath, deployment);
  for (const [name, script] of Object.entries(deployed.scripts)) {
    steps.info(
      `${name.padEnd(13)} code hash ${script.codeHash} (${script.hashType})`,
    );
  }

  steps.start(
    `create the config cell (vote-duration ${VOTE_DURATION}, ` +
      `vote-window ${VOTE_WINDOW}, challenge-time ${CHALLENGE_TIME})`,
  );
  const configCell = await createConfigCell(signer, deployment, {
    yesThreshold: YES_THRESHOLD,
    minimalProposalCapacity: MINIMAL_PROPOSAL_CAPACITY,
    voteDuration: VOTE_DURATION,
    voteWindow: VOTE_WINDOW,
    challengeTime: CHALLENGE_TIME,
    minimalVoteAmount: MINIMAL_VOTE_AMOUNT,
  });
  await waitForTransaction(client, configCell.txHash);
  patchConfigFile(configPath, {
    configArgs: configCell.configArgs,
    configCell: configCell.configCell,
  });
  const active = loadConfig(configPath);
  steps.info(`config cell ${formatOutPoint(configCell.configCell)}`);

  steps.start(`dao-deposit ${DEPOSIT_AMOUNT / CKB} CKB`);
  const deposit = await depositDao(signer, active, {
    amount: DEPOSIT_AMOUNT,
  });
  await waitForTransaction(client, deposit.txHash);
  steps.info(`deposit cell ${formatOutPoint(deposit.depositCell)}`);

  steps.start("create a proposal");
  const proposal = await createProposal(signer, active, {
    description: "Fund the docs (e2e)",
    requestedAmount: REQUESTED_AMOUNT,
  });
  await waitForTransaction(client, proposal.txHash);
  steps.info(`proposal cell ${formatOutPoint(proposal.proposalCell)}`);

  steps.start('cast a "YES" vote backed by the deposit');
  const vote = await castVote(signer, active, {
    proposalOutPoint: proposal.proposalCell,
    direction: "yes",
    deposits: [deposit.depositCell],
  });
  await waitForTransaction(client, vote.txHash);
  steps.info(
    `vote cell ${formatOutPoint(vote.voteCell)} certifying ` +
      `${vote.voteAmount / CKB} CKB`,
  );

  steps.start(
    'create a "YES" counting cell for the whole hash range (waits for vote_duration)',
  );
  const counting = await createCountingCell(signer, active, {
    proposalOutPoint: proposal.proposalCell,
    direction: "yes",
    startHash: 0,
    endHash: 0xffff,
    voteOutPoints: [vote.voteCell],
    wait: true,
  });
  await waitForTransaction(client, counting.txHash);
  steps.info(
    `counting cell ${formatOutPoint(counting.countingCell)} certifying ` +
      `${counting.voteAmount / CKB} CKB`,
  );

  steps.start("finalize the proposal");
  const finalizeTx = await finalizeProposal(signer, active, {
    proposalOutPoint: proposal.proposalCell,
    countingOutPoints: [counting.countingCell],
    wait: true,
  });
  await waitForTransaction(client, finalizeTx);
  const finalized = { txHash: finalizeTx, index: 0 };
  steps.info(`finalized proposal ${formatOutPoint(finalized)}`);

  steps.start("pass the proposal (waits for challenge_time)");
  const passTx = await passProposal(signer, active, {
    proposalOutPoint: finalized,
    wait: true,
  });
  await waitForTransaction(client, passTx);
  const passed = { txHash: passTx, index: 0 };

  const state = await getProposalCell(client, active, passed);
  if (state.data.status !== ProposalStatus.Passed) {
    throw new Error(
      `the proposal is in status ${state.data.status} instead of passed`,
    );
  }
  steps.info(`passed proposal ${formatOutPoint(passed)}`);
  steps.info(
    `total_yes ${state.data.totalYes / CKB} CKB, requested ` +
      `${state.data.requestedAmount / CKB} CKB (receive-grant is out of scope)`,
  );
  console.log("\nEnd-to-end run completed successfully.");
}

export function registerE2eCommand(program: Command): void {
  program
    .command("e2e")
    .description("run the whole voting lifecycle against a local devnet")
    .option(
      "-c, --config <path>",
      "deployment config written by the run",
      "./devnet.config",
    )
    .option("--rpc-url <url>", "CKB RPC endpoint", "http://127.0.0.1:8114")
    .option("--keep-running", "leave the devnet processes running")
    .action(async (options: E2eOptions) => {
      let finished = false;
      const shutdown = async (): Promise<void> => {
        if (options.keepRunning) {
          console.log("The devnet is still running (--keep-running).");
          return;
        }
        if (ownsDevnet) {
          await stopDevnet();
          console.log("The devnet was shut down.");
        }
      };
      const onSignal = (code: number): void => {
        void (async () => {
          if (finished) {
            return;
          }
          finished = true;
          console.log("\nInterrupted, shutting the devnet down...");
          await shutdown();
          process.exit(code);
        })();
      };
      const onSigint = () => onSignal(130);
      const onSigterm = () => onSignal(143);
      const onFatal = (error: unknown) => {
        console.error(error);
        onSignal(1);
      };
      const onExit = () => stopDevnetSync();

      process.on("SIGINT", onSigint);
      process.on("SIGTERM", onSigterm);
      process.on("uncaughtException", onFatal);
      process.on("unhandledRejection", onFatal);
      process.on("exit", onExit);

      try {
        await runE2e(options);
      } catch (error) {
        console.error(
          `Error: ${error instanceof Error ? error.message : String(error)}`,
        );
        process.exitCode = 1;
      } finally {
        finished = true;
        process.off("SIGINT", onSigint);
        process.off("SIGTERM", onSigterm);
        process.off("uncaughtException", onFatal);
        process.off("unhandledRejection", onFatal);
        process.off("exit", onExit);
        await shutdown();
      }
    });
}
