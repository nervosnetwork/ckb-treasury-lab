import { Command } from "commander";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { resolveNetworkConfig } from "../shared.js";
import type { NetworkConfig, ScriptInfo } from "../../config.js";

function ok(msg: string): void {
  console.log(`  ok   ${msg}`);
}

function fail(msg: string): void {
  console.log(`  FAIL ${msg}`);
}

interface MigrationRecipe {
  name: string;
  tx_hash: string;
  index: number;
  type_id?: string;
}

interface Migration {
  cell_recipes: MigrationRecipe[];
}

function checkScriptVsDeployment(
  name: string,
  info: ScriptInfo,
  deploymentDir: string,
  subdir: string,
): boolean {
  const migDir = join(deploymentDir, subdir, "migrations");
  if (!existsSync(migDir)) {
    fail(`${name}: migration directory not found: ${migDir}`);
    return false;
  }

  const files = readdirSync(migDir)
    .filter((f) => f.endsWith(".json"))
    .sort();
  if (files.length === 0) {
    fail(`${name}: no migration JSON files in ${migDir}`);
    return false;
  }

  const migFile = join(migDir, files[files.length - 1]);
  let mig: Migration;
  try {
    mig = JSON.parse(readFileSync(migFile, "utf8")) as Migration;
  } catch {
    fail(`${name}: failed to parse ${migFile}`);
    return false;
  }

  const recipe = mig.cell_recipes?.[0];
  if (!recipe) {
    fail(`${name}: no cell_recipes in ${migFile}`);
    return false;
  }

  let allOk = true;

  if (recipe.type_id !== info.codeHash) {
    fail(
      `${name} codeHash: config=${info.codeHash}, deployment=${recipe.type_id ?? "(missing)"}`,
    );
    allOk = false;
  }

  if (recipe.tx_hash !== info.outPoint.txHash) {
    fail(
      `${name} outPoint.txHash: config=${info.outPoint.txHash}, deployment=${recipe.tx_hash}`,
    );
    allOk = false;
  }

  if (recipe.index !== info.outPoint.index) {
    fail(
      `${name} outPoint.index: config=${info.outPoint.index}, deployment=${recipe.index}`,
    );
    allOk = false;
  }

  if (allOk) {
    ok(
      `${name}: ${info.outPoint.txHash}:${info.outPoint.index} codeHash=${info.codeHash}`,
    );
  }
  return allOk;
}

interface RpcResponse<T> {
  result?: T;
  error?: { code: number; message: string };
}

async function rpcCall<T>(
  rpcUrl: string,
  method: string,
  params: unknown[],
): Promise<T> {
  const res = await fetch(rpcUrl, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ id: 1, jsonrpc: "2.0", method, params }),
  });
  const data = (await res.json()) as RpcResponse<T>;
  if (data.error) throw new Error(`RPC error: ${data.error.message}`);
  if (data.result === undefined) throw new Error(`No result from ${method}`);
  return data.result;
}

async function checkScriptCellAlive(
  name: string,
  outPoint: ScriptInfo["outPoint"],
  rpcUrl: string,
): Promise<boolean> {
  try {
    const result = await rpcCall<{ cell: unknown | null }>(
      rpcUrl,
      "get_live_cell",
      [
        {
          tx_hash: outPoint.txHash,
          index: `0x${outPoint.index.toString(16)}`,
        },
        false,
      ],
    );
    if (result.cell !== null) {
      ok(`${name} cell is live: ${outPoint.txHash}:${outPoint.index}`);
      return true;
    } else {
      fail(`${name} cell NOT live: ${outPoint.txHash}:${outPoint.index}`);
      return false;
    }
  } catch (err) {
    fail(
      `${name} cell check failed: ${err instanceof Error ? err.message : String(err)}`,
    );
    return false;
  }
}

interface RpcBlock {
  header: Record<string, unknown>;
  transactions: Array<{
    outputs: Array<{
      capacity: string;
      lock: { code_hash: string; hash_type: string; args: string };
      type?: { code_hash: string; hash_type: string; args: string } | null;
    }>;
  }>;
}

async function checkKnownScriptCellDeps(
  config: NetworkConfig,
): Promise<boolean> {
  if (!config.knownScripts) {
    ok("knownScripts: not configured (using testnet defaults)");
    return true;
  }
  let allOk = true;

  for (const [name, info] of Object.entries(config.knownScripts)) {
    for (const { cellDep } of info.cellDeps) {
      const { txHash, index: idx } = cellDep.outPoint;
      if (!(await checkScriptCellAlive(name, { txHash, index: Number(idx) }, config.ckbRpcUrl))) {
        allOk = false;
      }
    }
  }

  return allOk;
}

export function registerCheck(program: Command): void {
  program
    .command("check")
    .description(
      "Verify SDK configuration against deployment files and chain state",
    )
    .option("--rpc-url <url>", "CKB RPC endpoint")
    .option("--config <path>", "JSON NetworkConfig file (defaults to devnet)")
    .option(
      "--deployment-dir <path>",
      "Path to deployment/devnet directory",
      "../deployment/devnet",
    )
    .action(async (opts) => {
      const config = resolveNetworkConfig(opts.config, opts.rpcUrl);
      const deploymentDir = resolve(opts.deploymentDir as string);
      let allOk = true;

      // 1. Script configs vs deployment migration files
      console.log("Checking script configurations against deployment files...");
      const scriptChecks: Array<{
        name: string;
        dir: string;
        info: ScriptInfo;
      }> = [
        {
          name: "alwaysSuccess",
          dir: "always-success",
          info: config.alwaysSuccess,
        },
        {
          name: "voteTypeScript",
          dir: "vote-type-script",
          info: config.voteTypeScript,
        },
      ];
      for (const { name, dir, info } of scriptChecks) {
        if (!checkScriptVsDeployment(name, info, deploymentDir, dir)) {
          allOk = false;
        }
        if (
          !(await checkScriptCellAlive(name, info.outPoint, config.ckbRpcUrl))
        ) {
          allOk = false;
        }
      }

      // 2. knownScripts cellDeps in genesis block
      console.log("\nChecking knownScripts cellDeps are in genesis block...");
      if (!(await checkKnownScriptCellDeps(config))) {
        allOk = false;
      }

      console.log();
      if (allOk) {
        console.log("All checks passed.");
        process.exit(0);
      } else {
        console.log("Some checks failed.");
        process.exit(1);
      }
    });
}
