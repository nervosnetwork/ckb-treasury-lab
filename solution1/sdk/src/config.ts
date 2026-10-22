/**
 * Deployment configuration of the voting system.
 *
 * The SDK talks to contracts that are already deployed, so it needs the
 * identity of four type scripts and of the `always success` lock. Since a
 * deployment is per chain, everything lives in a JSON file (see
 * `devnet.json.example`) that the CLI reads with `--config`; nothing is
 * hard-coded here.
 *
 * The current config *cell* is discovered on chain from the config type
 * script, or given explicitly through `configCell` when the node has no
 * indexer.
 */

import { readFileSync, existsSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { ccc } from "@ckb-ccc/shell";
import { HASH_TYPE_BYTES, MAX_HASH_TYPE_BYTE } from "./constants.js";
export { feeRateOf } from "./fee.js";

/** The five scripts a deployment is made of. */
export type ScriptKey =
  "config" | "proposal" | "vote" | "counting" | "alwaysSuccess";

/** {@link ScriptKey} in the order the `deploy` command publishes them. */
export const SCRIPT_KEYS: readonly ScriptKey[] = [
  "config",
  "proposal",
  "vote",
  "counting",
  "alwaysSuccess",
];

/** Where the code cell of a deployed script lives in `cell_deps`. */
export interface CellDepDeployment {
  txHash: ccc.HexLike;
  index: number;
  /** Defaults to `code`. */
  depType?: "code" | "depGroup";
}

/** A deployed script: its identity plus the code cell that provides it. */
export interface ScriptDeployment {
  /** The `code_hash` to write in a script referencing this one. */
  codeHash: ccc.HexLike;
  hashType: ccc.HashType;
  /** Default `args` of the script; empty for most of them. */
  args?: ccc.HexLike;
  /** The code cell to list in `cell_deps`. */
  cellDep: CellDepDeployment;
}

/** Everything the SDK needs to know about a deployment. */
export interface DeploymentConfig {
  rpcUrl: string;
  /** Fee rate in shannons per KB; defaults to 1500. */
  feeRate?: number | string | bigint;
  scripts: {
    /** The singleton config type script. */
    config: ScriptDeployment;
    /** The proposal / finalized / passed proposal type script. */
    proposal: ScriptDeployment;
    /** The vote type script. */
    vote: ScriptDeployment;
    /** The counting type script. */
    counting: ScriptDeployment;
    /** The `always success` lock used by a finalized proposal cell. */
    alwaysSuccess: ScriptDeployment;
  };
  /**
   * The current config cell. Optional: when omitted the SDK searches for a
   * cell whose type script has the configured config `code_hash`/`hash_type`.
   */
  configCell?: { txHash: ccc.HexLike; index: number };
  /**
   * Override the built-in known-script outpoints. Required on a devnet whose
   * genesis differs from the public testnets.
   */
  knownScripts?: Partial<Record<ccc.KnownScript, ccc.ScriptInfoLike>>;
}

/** Paths tried, in order, when no config path is given. */
export function defaultConfigPaths(): string[] {
  return [
    process.env.CKB_VOTE_CONFIG,
    resolve("devnet.config"),
    resolve("devnet.json"),
    resolve("deployment/devnet.json"),
    resolve("../deployment/devnet.json"),
  ].filter((path): path is string => !!path);
}

/** Returns the first config path that exists, or throws with the candidates. */
export function resolveConfigPath(explicit?: string): string {
  const candidates = explicit ? [resolve(explicit)] : defaultConfigPaths();
  for (const path of candidates) {
    if (existsSync(path)) {
      return path;
    }
  }
  throw new Error(
    `No deployment config found. Pass --config <path>, set CKB_VOTE_CONFIG, ` +
      `or create one of: ${candidates.join(", ")}. On a devnet, run ` +
      `"ckb-vote devnet-scripts --out <path>" then ` +
      `"ckb-vote deploy --config <path> --out <path>"; ` +
      `sdk/deployment.example.json shows the expected shape.`,
  );
}

function requireObject(value: unknown, label: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null) {
    throw new Error(`${label} must be an object`);
  }
  return value as Record<string, unknown>;
}

function requireString(value: unknown, label: string): string {
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`${label} must be a non-empty string`);
  }
  return value;
}

function requireBytes(
  value: unknown,
  byteLength: number,
  label: string,
): ccc.Hex {
  const hex = ccc.hexFrom(requireString(value, label));
  if (ccc.bytesFrom(hex).byteLength !== byteLength) {
    throw new Error(`${label} must be ${byteLength} bytes`);
  }
  return hex;
}

function parseCellDep(value: unknown, label: string): CellDepDeployment {
  const dep = requireObject(value, label);
  const depType = dep.depType ?? "code";
  if (depType !== "code" && depType !== "depGroup") {
    throw new Error(`${label}.depType must be "code" or "depGroup"`);
  }
  const index = Number(dep.index ?? 0);
  if (!Number.isInteger(index) || index < 0) {
    throw new Error(`${label}.index must be a non-negative integer`);
  }
  return {
    txHash: requireBytes(dep.txHash, 32, `${label}.txHash`),
    index,
    depType,
  };
}

function parseScript(value: unknown, label: string): ScriptDeployment {
  const script = requireObject(value, label);
  const hashType = requireString(script.hashType, `${label}.hashType`);
  if (!(hashType in HASH_TYPE_BYTES)) {
    throw new Error(
      `${label}.hashType must be data, type, data1 or data2 (got ${hashType})`,
    );
  }
  return {
    codeHash: requireBytes(script.codeHash, 32, `${label}.codeHash`),
    hashType: hashType as ccc.HashType,
    args:
      script.args == null ? undefined : ccc.hexFrom(script.args as ccc.HexLike),
    cellDep: parseCellDep(script.cellDep, `${label}.cellDep`),
  };
}

/** Parses and validates a deployment config from JSON. */
export function parseConfig(
  input: unknown,
  source = "config",
): DeploymentConfig {
  const root = requireObject(input, source);
  const scripts = requireObject(root.scripts, `${source}.scripts`);

  const parsed = {} as DeploymentConfig["scripts"];
  for (const key of SCRIPT_KEYS) {
    parsed[key] = parseScript(scripts[key], `${source}.scripts.${key}`);
  }

  // The config stores the hash types of the vote, counting and always success
  // scripts as one byte each; anything larger is rejected on chain.
  for (const key of ["vote", "counting", "alwaysSuccess"] as const) {
    if (HASH_TYPE_BYTES[parsed[key].hashType] > MAX_HASH_TYPE_BYTE) {
      throw new Error(
        `${source}.scripts.${key}.hashType is larger than the largest hash type ` +
          `the contracts accept (data2)`,
      );
    }
  }

  let configCell: DeploymentConfig["configCell"];
  if (root.configCell != null) {
    const cell = requireObject(root.configCell, `${source}.configCell`);
    configCell = {
      txHash: requireBytes(cell.txHash, 32, `${source}.configCell.txHash`),
      index: Number(cell.index ?? 0),
    };
  }

  return {
    rpcUrl: requireString(root.rpcUrl, `${source}.rpcUrl`),
    feeRate: root.feeRate as DeploymentConfig["feeRate"],
    scripts: parsed,
    configCell,
    knownScripts: root.knownScripts as DeploymentConfig["knownScripts"],
  };
}

/** Reads and validates a deployment config file. */
export function loadConfig(path?: string): DeploymentConfig {
  const resolved = resolveConfigPath(path);
  return parseConfig(JSON.parse(readFileSync(resolved, "utf8")), resolved);
}

/** What is needed before the voting contracts exist: a node and its genesis. */
export interface BootstrapConfig {
  rpcUrl: string;
  feeRate?: number | string | bigint;
  knownScripts?: Partial<Record<ccc.KnownScript, ccc.ScriptInfoLike>>;
}

/** Parses a config that may not describe the voting scripts yet. */
export function parseBootstrapConfig(
  input: unknown,
  source = "config",
): BootstrapConfig {
  const root = requireObject(input, source);
  return {
    rpcUrl: requireString(root.rpcUrl, `${source}.rpcUrl`),
    feeRate: root.feeRate as BootstrapConfig["feeRate"],
    knownScripts: root.knownScripts as BootstrapConfig["knownScripts"],
  };
}

/** Loads a bootstrap config; without one, a plain local devnet is assumed. */
export function loadBootstrapConfig(path?: string): BootstrapConfig {
  const resolved = firstConfigPath(path);
  if (!resolved) {
    return { rpcUrl: "http://127.0.0.1:8114" };
  }
  return parseBootstrapConfig(
    JSON.parse(readFileSync(resolved, "utf8")),
    resolved,
  );
}

/** Like {@link resolveConfigPath} but returns `undefined` instead of throwing. */
export function firstConfigPath(explicit?: string): string | undefined {
  const candidates = explicit ? [resolve(explicit)] : defaultConfigPaths();
  return candidates.find((path) => existsSync(path));
}

/**
 * Writes the config cell coordinates back into a deployment config file.
 *
 * `create-config` produces the config Type ID, so the file has to be updated
 * before a proposal can be created.
 */
export function patchConfigFile(
  path: string,
  patch: {
    configArgs?: ccc.HexLike;
    configCell?: { txHash: ccc.HexLike; index: number };
  },
): void {
  const resolved = resolve(path);
  const json = JSON.parse(readFileSync(resolved, "utf8")) as Record<
    string,
    unknown
  >;
  const scripts = requireObject(json.scripts, `${resolved}.scripts`);
  if (patch.configArgs !== undefined) {
    const config = requireObject(scripts.config, `${resolved}.scripts.config`);
    config.args = ccc.hexFrom(patch.configArgs);
  }
  if (patch.configCell !== undefined) {
    json.configCell = {
      txHash: ccc.hexFrom(patch.configCell.txHash),
      index: patch.configCell.index,
    };
  }
  writeFileSync(resolved, `${JSON.stringify(json, null, 2)}\n`);
}
