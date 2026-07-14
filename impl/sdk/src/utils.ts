import { readFileSync } from "node:fs";
import { ccc } from "@ckb-ccc/shell";
import { DEVNET_CONFIG, type ScriptInfo, type NetworkConfig, type KnownScriptInfo } from "./config.js";

/**
 * Compute blake160: CKB blake2b-256 hash truncated to 20 bytes.
 * Matches ckb_hash::new_blake2b() (32-byte output) taking the first 20 bytes.
 */
export function blake160(data: ccc.BytesLike): ccc.Hex {
  const hasher = new ccc.HasherCkb();
  hasher.update(data);
  return ccc.hexFrom(ccc.bytesFrom(hasher.digest()).slice(0, 20));
}

/**
 * Compute the blake160 Type ID (20 bytes) from the first input and output index.
 * blake160(CellInput.as_bytes || output_index_u64_le) — first 20 bytes of the
 * blake2b-256 hash, matching the standard Type ID construction.
 */
export function computeBlake160TypeId(
  firstInput: ccc.CellInput,
  outputIndex: number,
): ccc.Hex {
  const indexBuf = new Uint8Array(8);
  new DataView(indexBuf.buffer).setBigUint64(0, BigInt(outputIndex), true);
  const hash = ccc.bytesFrom(
    ccc.hashCkb(ccc.CellInput.encode(firstInput), indexBuf),
  );
  return ccc.hexFrom(hash.slice(0, 20));
}

/**
 * Compute the proposal type script args:
 *   blake160 Type ID (20 bytes)
 */
export function buildProposalTypeScriptArgs(
  firstInput: ccc.CellInput,
  outputIndex: number,
): ccc.Hex {
  return computeBlake160TypeId(firstInput, outputIndex);
}

/**
 * Build a Script from a ScriptInfo (used for lock/type fields on cells).
 */
export function scriptFromInfo(
  info: ScriptInfo,
  args: ccc.HexLike = "0x",
): ccc.Script {
  return ccc.Script.from({
    codeHash: info.codeHash,
    hashType: info.hashType,
    args: ccc.hexFrom(args),
  });
}

/**
 * Build a CellDep that loads a script contract cell.
 */
export function cellDepFromInfo(info: ScriptInfo): ccc.CellDep {
  return ccc.CellDep.from({
    outPoint: ccc.OutPoint.from({
      txHash: info.outPoint.txHash,
      index: info.outPoint.index,
    }),
    depType: "code",
  });
}

/**
 * Build a CellDep from an OutPointLike (for non-script cells like proposal/DAO cells).
 */
export function cellDepFromOutPoint(op: {
  txHash: string;
  index: number | bigint;
}): ccc.CellDep {
  return ccc.CellDep.from({
    outPoint: ccc.OutPoint.from({ txHash: op.txHash, index: op.index }),
    depType: "code",
  });
}

/**
 * Merge caller-supplied config overrides with the base config.
 */
export function mergeConfig(
  base: NetworkConfig,
  overrides?: Partial<NetworkConfig>,
): NetworkConfig {
  if (!overrides) return base;
  return { ...base, ...overrides };
}

/**
 * Load a NetworkConfig from an optional JSON file path, merged on top of the
 * default devnet config. Callers can also supply runtime overrides (e.g. --rpc-url).
 */
export function loadNetworkConfig(
  configPath?: string,
  cliOverrides?: Partial<NetworkConfig>,
): NetworkConfig {
  let base = DEVNET_CONFIG;

  if (configPath) {
    let raw: unknown;
    try {
      raw = JSON.parse(readFileSync(configPath, "utf8"));
    } catch (err) {
      throw new Error(
        `Failed to parse config file "${configPath}": ${err instanceof Error ? err.message : String(err)}`,
      );
    }
    if (raw === null || typeof raw !== "object" || Array.isArray(raw)) {
      throw new Error(`Config file "${configPath}" must be a JSON object`);
    }
    const json = raw as Record<string, unknown>;

    base = {
      ckbRpcUrl:
        typeof json.ckbRpcUrl === "string"
          ? json.ckbRpcUrl
          : base.ckbRpcUrl,
      alwaysSuccess: json.alwaysSuccess
        ? (json.alwaysSuccess as ScriptInfo)
        : base.alwaysSuccess,
      proposalTypeScript: json.proposalTypeScript
        ? (json.proposalTypeScript as ScriptInfo)
        : base.proposalTypeScript,
      voteTypeScript: json.voteTypeScript
        ? (json.voteTypeScript as ScriptInfo)
        : base.voteTypeScript,
      sp1VerifyingKeyHash:
        typeof json.sp1VerifyingKeyHash === "string"
          ? json.sp1VerifyingKeyHash
          : base.sp1VerifyingKeyHash,
      feeRate:
        typeof json.feeRate === "number"
          ? BigInt(json.feeRate)
          : typeof json.feeRate === "string"
            ? BigInt(json.feeRate)
            : base.feeRate,
      knownScripts:
        typeof json.knownScripts === "object" && json.knownScripts !== null
          ? (json.knownScripts as Record<string, KnownScriptInfo>)
          : base.knownScripts,
    };
  }

  if (cliOverrides) {
    base = { ...base, ...cliOverrides };
  }

  return base;
}

/**
 * Build a CCC client connected to the given RPC URL.
 * Uses ClientPublicTestnet (same `ckt` address prefix as devnet).
 * When the config supplies `knownScripts`, those entries are merged on top of
 * the testnet defaults so only the overridden scripts change; all other
 * known scripts (AnyoneCanPay, xUDT, etc.) remain available.
 */
export function buildClient(
  rpcUrl: string,
  knownScripts?: Record<string, KnownScriptInfo>,
): ccc.ClientPublicTestnet {
  if (!knownScripts) {
    return new ccc.ClientPublicTestnet({ url: rpcUrl });
  }
  // Build a temporary client to read the full testnet script map, then merge.
  const base = new ccc.ClientPublicTestnet();
  const merged = { ...base.scripts, ...knownScripts };
  return new ccc.ClientPublicTestnet({ url: rpcUrl, scripts: merged as never });
}

/**
 * Convert a hash_type string to the byte value used in molecule encoding.
 * CKB spec: data=0, type=1, data1=2, data2=3
 */
export function hashTypeToByte(hashType: string): number {
  switch (hashType) {
    case "data":
      return 0;
    case "type":
      return 1;
    case "data1":
      return 2;
    case "data2":
      return 3;
    default:
      throw new Error(`Unknown hash type: ${hashType}`);
  }
}

/**
 * Fetch a live cell by outpoint. Throws if the cell is not found.
 */
export async function getRequiredCell(
  client: ccc.Client,
  outPoint: { txHash: string; index: number },
): Promise<ccc.Cell> {
  const cell = await client.getCell(
    ccc.OutPoint.from({ txHash: outPoint.txHash, index: outPoint.index }),
  );
  if (!cell) {
    throw new Error(`Cell not found: ${outPoint.txHash}:${outPoint.index}`);
  }
  return cell;
}

/**
 * Get the signer's recommended lock script (secp256k1 by default).
 */
export async function getSignerLock(signer: ccc.Signer): Promise<ccc.Script> {
  const addr = await signer.getRecommendedAddressObj();
  return addr.script;
}
