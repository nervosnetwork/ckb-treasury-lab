/**
 * The config cell: the singleton that holds every parameter of the system.
 *
 * See `docs/config-type-script-spec.md`.
 */

import { ccc } from "@ckb-ccc/shell";
import type { DeploymentConfig } from "./config.js";
import { SCRIPT_KEYS, feeRateOf } from "./config.js";
import { HASH_TYPE_BYTES, MAX_HASH_TYPE_BYTE } from "./constants.js";
import { encodeVotingConfig, type VotingConfig } from "./codec.js";
import { loadConfigCell } from "./query.js";
import {
  addUniqueCellDeps,
  cellDepFromDeployment,
  ckbHash,
  formatOutPoint,
  requireLiveCell,
  resolveLock,
  scriptFromDeployment,
  scriptHash,
  scriptId,
  signerLock,
  typeId,
} from "./utils.js";

/** Parameters of a freshly minted config cell. */
export interface CreateConfigCellParams {
  /** Lock of the config cell; defaults to the signer's lock. */
  lock?: string | ccc.ScriptLike;
  /** Halts every voting script when true. Defaults to false. */
  emergentHalt?: boolean;
  /** Shannons of "YES" votes required to finalize a proposal. */
  yesThreshold?: bigint | number;
  /** Minimum bond of a proposal, in shannons. */
  minimalProposalCapacity?: bigint | number;
  /** Blocks a proposal waits before it can be finalized. */
  voteDuration?: bigint | number;
  /** Blocks after the proposal in which a vote stays valid. */
  voteWindow?: bigint | number;
  /** Blocks a finalized proposal waits before it can pass. */
  challengeTime?: bigint | number;
  /** Lock allowed to veto a finalized proposal; defaults to the signer's. */
  vetoLock?: string | ccc.ScriptLike;
  /** The 32 byte hash of the veto lock, used instead of `vetoLock`. */
  vetoLockScriptHash?: ccc.HexLike;
  /** Minimum `vote_amount` of a vote cell, in shannons. */
  minimalVoteAmount?: bigint | number;
  /** Capacity of the config cell, in shannons; defaults to the minimum. */
  capacity?: bigint | number;
}

/** A config cell that was just created. */
export interface CreateConfigCellResult {
  txHash: ccc.Hex;
  configCell: { txHash: ccc.Hex; index: number };
  configTypeScript: ccc.Script;
  /** `blake160(config type script)`, to copy into the deployment config. */
  configId: ccc.Hex;
  /** The Type ID `args` of the config type script. */
  configArgs: ccc.Hex;
}

/** Parameters of a config cell update. */
export interface UpdateConfigCellParams {
  emergentHalt?: boolean;
  yesThreshold?: bigint | number;
  minimalProposalCapacity?: bigint | number;
  voteDuration?: bigint | number;
  voteWindow?: bigint | number;
  challengeTime?: bigint | number;
  vetoLock?: string | ccc.ScriptLike;
  vetoLockScriptHash?: ccc.HexLike;
  minimalVoteAmount?: bigint | number;
  capacity?: bigint | number;
}

/** The `VotingConfig` implied by a deployment plus explicit parameters. */
function buildConfigData(
  config: DeploymentConfig,
  params: CreateConfigCellParams,
  vetoLockScriptHash: ccc.Hex,
): VotingConfig {
  for (const key of ["vote", "counting", "alwaysSuccess"] as const) {
    if (HASH_TYPE_BYTES[config.scripts[key].hashType] > MAX_HASH_TYPE_BYTE) {
      throw new Error(
        `scripts.${key} uses a hash type larger than data2, which the config ` +
          `cell cannot record`,
      );
    }
  }
  return {
    emergentHalt: params.emergentHalt ? 1 : 0,
    voteCodeHash: ccc.hexFrom(config.scripts.vote.codeHash),
    voteHashType: HASH_TYPE_BYTES[config.scripts.vote.hashType],
    countingCodeHash: ccc.hexFrom(config.scripts.counting.codeHash),
    countingHashType: HASH_TYPE_BYTES[config.scripts.counting.hashType],
    alwaysSuccessCodeHash: ccc.hexFrom(config.scripts.alwaysSuccess.codeHash),
    alwaysSuccessHashType:
      HASH_TYPE_BYTES[config.scripts.alwaysSuccess.hashType],
    yesThreshold: BigInt(params.yesThreshold ?? 0),
    minimalProposalCapacity: BigInt(params.minimalProposalCapacity ?? 0),
    voteDuration: BigInt(params.voteDuration ?? 0),
    voteWindow: BigInt(params.voteWindow ?? 0),
    challengeTime: BigInt(params.challengeTime ?? 0),
    vetoLockScriptHash,
    minimalVoteAmount: BigInt(params.minimalVoteAmount ?? 0),
  };
}

/**
 * Mints the config cell.
 *
 * The config type script is a Type ID, so its `args` are only known once the
 * first input is collected: the output type script is patched in a second pass.
 */
export async function createConfigCell(
  signer: ccc.Signer,
  config: DeploymentConfig,
  params: CreateConfigCellParams = {},
): Promise<CreateConfigCellResult> {
  const client = signer.client;
  const lock = params.lock
    ? await resolveLock(client, params.lock)
    : await signerLock(signer);
  const vetoLockScriptHash = params.vetoLockScriptHash
    ? ccc.hexFrom(params.vetoLockScriptHash)
    : scriptHash(
        params.vetoLock ? await resolveLock(client, params.vetoLock) : lock,
      );

  const data = encodeVotingConfig(
    buildConfigData(config, params, vetoLockScriptHash),
  );
  const placeholder = ccc.Script.from({
    codeHash: config.scripts.config.codeHash,
    hashType: config.scripts.config.hashType,
    args: `0x${"00".repeat(20)}`,
  });

  const tx = ccc.Transaction.from({
    outputs: [
      ccc.CellOutput.from(
        { capacity: BigInt(params.capacity ?? 0), lock, type: placeholder },
        data,
      ),
    ],
    outputsData: [data],
  });
  addUniqueCellDeps(tx, [cellDepFromDeployment(config.scripts.config)]);

  await tx.completeInputsByCapacity(signer);
  const first = tx.inputs[0];
  if (!first) {
    throw new Error(
      "no input was collected; the signer has no spendable cells",
    );
  }

  const configArgs = typeId(first, 0);
  const configTypeScript = scriptFromDeployment(
    config.scripts.config,
    configArgs,
  );
  tx.outputs[0].type = configTypeScript;

  await tx.completeFeeBy(signer, feeRateOf(config));
  const txHash = await signer.sendTransaction(tx);

  return {
    txHash,
    configCell: { txHash, index: 0 },
    configTypeScript,
    configId: scriptId(configTypeScript),
    configArgs,
  };
}

/**
 * Updates the config cell in place: the `args` (and therefore the Type ID) are
 * preserved, only the payload changes.
 */
export async function updateConfigCell(
  signer: ccc.Signer,
  config: DeploymentConfig,
  params: UpdateConfigCellParams,
): Promise<{
  txHash: ccc.Hex;
  configCell: { txHash: ccc.Hex; index: number };
}> {
  const client = signer.client;
  const current = await loadConfigCell(client, config);
  const vetoLockScriptHash = params.vetoLockScriptHash
    ? ccc.hexFrom(params.vetoLockScriptHash)
    : params.vetoLock
      ? scriptHash(await resolveLock(client, params.vetoLock))
      : current.data.vetoLockScriptHash;

  const data = encodeVotingConfig({
    ...current.data,
    emergentHalt:
      params.emergentHalt === undefined
        ? current.data.emergentHalt
        : params.emergentHalt
          ? 1
          : 0,
    yesThreshold:
      params.yesThreshold === undefined
        ? current.data.yesThreshold
        : BigInt(params.yesThreshold),
    minimalProposalCapacity:
      params.minimalProposalCapacity === undefined
        ? current.data.minimalProposalCapacity
        : BigInt(params.minimalProposalCapacity),
    voteDuration:
      params.voteDuration === undefined
        ? current.data.voteDuration
        : BigInt(params.voteDuration),
    voteWindow:
      params.voteWindow === undefined
        ? current.data.voteWindow
        : BigInt(params.voteWindow),
    challengeTime:
      params.challengeTime === undefined
        ? current.data.challengeTime
        : BigInt(params.challengeTime),
    vetoLockScriptHash,
    minimalVoteAmount:
      params.minimalVoteAmount === undefined
        ? current.data.minimalVoteAmount
        : BigInt(params.minimalVoteAmount),
  });

  const capacity =
    params.capacity === undefined
      ? current.cell.cellOutput.capacity
      : BigInt(params.capacity);

  const tx = ccc.Transaction.from({
    inputs: [{ previousOutput: current.cell.outPoint }],
    outputs: [
      ccc.CellOutput.from(
        {
          capacity,
          lock: current.cell.cellOutput.lock,
          type: current.typeScript,
        },
        data,
      ),
    ],
    outputsData: [data],
  });
  addUniqueCellDeps(tx, [cellDepFromDeployment(config.scripts.config)]);

  await tx.completeFeeBy(signer, feeRateOf(config));
  const txHash = await signer.sendTransaction(tx);
  // The updated config cell is the first output; the change cell follows it.
  return { txHash, configCell: { txHash, index: 0 } };
}

/** One line of the deployment report produced by {@link checkDeployment}. */
export interface DeploymentCheck {
  script: string;
  ok: boolean;
  message: string;
}

/**
 * Verifies that every deployed script can be found on chain and that its
 * `code_hash` matches the code cell the config points at.
 *
 * For a `data` / `data1` / `data2` hash type the code hash is the ckb-hash of
 * the code cell data; for a `type` hash type it is the ckb-hash of the code
 * cell's own type script.
 */
export async function checkDeployment(
  client: ccc.Client,
  config: DeploymentConfig,
): Promise<DeploymentCheck[]> {
  const checks: DeploymentCheck[] = [];

  for (const key of SCRIPT_KEYS) {
    const deployment = config.scripts[key];
    let message = "";
    let ok = false;
    try {
      const code = await requireLiveCell(client, deployment.cellDep);
      const expected =
        deployment.hashType === "type"
          ? code.cellOutput.type
            ? ckbHash(code.cellOutput.type.toBytes())
            : undefined
          : ckbHash(code.outputData);
      if (!expected) {
        message = "hash type is `type` but the code cell has no type script";
      } else if (expected !== ccc.hexFrom(deployment.codeHash)) {
        ok = false;
        message = `code hash mismatch: the chain says ${expected}`;
      } else {
        ok = true;
        message = `ok, ${formatOutPoint(deployment.cellDep)}`;
      }
    } catch (error) {
      message = error instanceof Error ? error.message : String(error);
    }
    checks.push({ script: key, ok, message });
  }

  try {
    const info = await loadConfigCell(client, config);
    checks.push({
      script: "configCell",
      ok: true,
      message:
        `ok, ${formatOutPoint(info.cell.outPoint)}, id ${info.id}, ` +
        `vote_duration ${info.data.voteDuration}, ` +
        `challenge_time ${info.data.challengeTime}, ` +
        `yes_threshold ${info.data.yesThreshold}, ` +
        `vote_window ${info.data.voteWindow}, ` +
        `minimal_proposal_capacity ${info.data.minimalProposalCapacity}, ` +
        `minimal_vote_amount ${info.data.minimalVoteAmount}`,
    });
  } catch (error) {
    checks.push({
      script: "configCell",
      ok: false,
      message: error instanceof Error ? error.message : String(error),
    });
  }

  return checks;
}
