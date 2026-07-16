import { Command } from "commander";
import { ccc } from "@ckb-ccc/shell";
import { ProposalCodec, VoteCodec } from "../../codec.js";
import { die, resolveNetworkConfig } from "../shared.js";
import { blake160, buildClient } from "../../utils.js";
import type { NetworkConfig } from "../../config.js";

// ─── Raw RPC helpers ─────────────────────────────────────────────────────────

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

// ─── RPC types ───────────────────────────────────────────────────────────────

interface RpcScript {
  code_hash: string;
  hash_type: string;
  args: string;
}

/** One row returned by get_transactions (group_by_transaction: false). */
interface RpcTxWithCell {
  tx_hash: string;
  block_number: string; // hex
  tx_index: string; // hex
  io_type: "input" | "output";
  io_index: string; // hex
}

interface RpcGetTransactionsResult {
  last_cursor: string;
  objects: RpcTxWithCell[];
}

/** Minimal shape from get_transaction needed for cell reconstruction. */
interface RpcFullTx {
  transaction: {
    inputs: Array<{ previous_output: { tx_hash: string; index: string } }>;
    outputs: Array<{
      capacity: string;
      lock: RpcScript;
      type?: RpcScript | null;
    }>;
    outputs_data: string[];
  };
}

interface RpcBlockHeader {
  header: { timestamp: string };
}

// ─── Paginated get_transactions ───────────────────────────────────────────────

async function getAllTxEntries(
  rpcUrl: string,
  codeHash: string,
  hashType: string,
  args: string,
  order: "asc" | "desc" = "asc",
): Promise<RpcTxWithCell[]> {
  const all: RpcTxWithCell[] = [];
  let cursor: string | null = null;
  const searchMode = args === "0x" ? "prefix" : "exact";
  const pageSize = 100;

  while (true) {
    const result: RpcGetTransactionsResult =
      await rpcCall<RpcGetTransactionsResult>(rpcUrl, "get_transactions", [
        {
          script: { code_hash: codeHash, hash_type: hashType, args },
          script_type: "type",
          script_search_mode: searchMode,
        },
        order,
        "0x" + pageSize.toString(16),
        cursor,
      ]);
    all.push(...result.objects);
    if (result.objects.length < pageSize) break;
    cursor = result.last_cursor;
  }
  return all;
}

// ─── Cached fetchers ──────────────────────────────────────────────────────────

async function prefetchTxs(
  rpcUrl: string,
  txHashes: string[],
  cache: Map<string, RpcFullTx>,
): Promise<void> {
  const missing = [...new Set(txHashes)].filter((h) => !cache.has(h));
  await Promise.all(
    missing.map(async (h) => {
      cache.set(h, await rpcCall<RpcFullTx>(rpcUrl, "get_transaction", [h]));
    }),
  );
}

async function getTimestampForBlock(
  rpcUrl: string,
  blockNumberHex: string,
  cache: Map<string, string>,
): Promise<string> {
  if (!cache.has(blockNumberHex)) {
    const block = await rpcCall<RpcBlockHeader>(rpcUrl, "get_block_by_number", [
      blockNumberHex,
      "0x2",
    ]);
    const ms = parseInt(block.header.timestamp, 16);
    const ts = new Date(ms)
      .toLocaleString("sv-SE", { timeZone: "Asia/Shanghai" })
      .replace("T", " ");
    cache.set(blockNumberHex, ts + " +08:00");
  }
  return cache.get(blockNumberHex)!;
}

// ─── Cell construction ────────────────────────────────────────────────────────

function makeScript(s: RpcScript): ccc.Script {
  return ccc.Script.from({
    codeHash: s.code_hash,
    hashType: s.hash_type,
    args: s.args,
  });
}

function buildCellFromTx(
  txHash: string,
  outputIdx: number,
  fullTx: RpcFullTx,
): { cell: ccc.Cell; outputData: string } | null {
  const output = fullTx.transaction.outputs[outputIdx];
  const outputData = fullTx.transaction.outputs_data[outputIdx] ?? "0x";
  if (!output) return null;
  const cell = ccc.Cell.from({
    outPoint: ccc.OutPoint.from({ txHash, index: outputIdx }),
    cellOutput: ccc.CellOutput.from({
      capacity: BigInt(output.capacity),
      lock: makeScript(output.lock),
      type: output.type ? makeScript(output.type) : undefined,
    }),
    outputData,
  });
  return { cell, outputData };
}

// ─── Query helpers ────────────────────────────────────────────────────────────

interface ProposalInfo {
  cell: ccc.Cell;
  decoded: ccc.mol.DecodedType<typeof ProposalCodec>;
  blockNumber: number;
  blockNumberHex: string;
  isLive: boolean;
}

interface VoteInfo {
  cell: ccc.Cell;
  decoded: ccc.mol.DecodedType<typeof VoteCodec>;
  blockNumber: number;
  blockNumberHex: string;
  isLive: boolean;
}

/**
 * Use get_transactions to find all proposal cells (live and consumed).
 */
async function collectProposals(
  rpcUrl: string,
  config: NetworkConfig,
  txCache: Map<string, RpcFullTx>,
): Promise<ProposalInfo[]> {
  const entries = await getAllTxEntries(
    rpcUrl,
    config.proposalTypeScript.codeHash,
    config.proposalTypeScript.hashType,
    "0x",
    "asc",
  );

  await prefetchTxs(
    rpcUrl,
    entries.map((e) => e.tx_hash),
    txCache,
  );

  const consumedSet = new Set<string>();
  for (const entry of entries) {
    if (entry.io_type !== "input") continue;
    const fullTx = txCache.get(entry.tx_hash)!;
    const inputIdx = parseInt(entry.io_index, 16);
    const prevOut = fullTx.transaction.inputs[inputIdx]?.previous_output;
    if (prevOut) {
      consumedSet.add(`${prevOut.tx_hash}:${parseInt(prevOut.index, 16)}`);
    }
  }

  const proposals: ProposalInfo[] = [];
  for (const entry of entries) {
    if (entry.io_type !== "output") continue;
    const fullTx = txCache.get(entry.tx_hash)!;
    const outputIdx = parseInt(entry.io_index, 16);
    const built = buildCellFromTx(entry.tx_hash, outputIdx, fullTx);
    if (!built || !built.outputData || built.outputData === "0x") continue;

    try {
      const decoded = ProposalCodec.decode(built.outputData);
      const outKey = `${entry.tx_hash}:${outputIdx}`;
      proposals.push({
        cell: built.cell,
        decoded,
        blockNumber: parseInt(entry.block_number, 16),
        blockNumberHex: entry.block_number,
        isLive: !consumedSet.has(outKey),
      });
    } catch {
      // skip malformed cells
    }
  }
  return proposals;
}

/**
 * Use get_transactions to find all vote cells for a given proposal (live and consumed).
 */
async function collectVotesForProposal(
  rpcUrl: string,
  config: NetworkConfig,
  voteTypeArgs: string,
  txCache: Map<string, RpcFullTx>,
): Promise<VoteInfo[]> {
  const entries = await getAllTxEntries(
    rpcUrl,
    config.voteTypeScript.codeHash,
    config.voteTypeScript.hashType,
    voteTypeArgs,
    "asc",
  );

  await prefetchTxs(
    rpcUrl,
    entries.map((e) => e.tx_hash),
    txCache,
  );

  const consumedSet = new Set<string>();
  for (const entry of entries) {
    if (entry.io_type !== "input") continue;
    const fullTx = txCache.get(entry.tx_hash)!;
    const inputIdx = parseInt(entry.io_index, 16);
    const prevOut = fullTx.transaction.inputs[inputIdx]?.previous_output;
    if (prevOut) {
      consumedSet.add(`${prevOut.tx_hash}:${parseInt(prevOut.index, 16)}`);
    }
  }

  const votes: VoteInfo[] = [];
  for (const entry of entries) {
    if (entry.io_type !== "output") continue;
    const fullTx = txCache.get(entry.tx_hash)!;
    const outputIdx = parseInt(entry.io_index, 16);
    const built = buildCellFromTx(entry.tx_hash, outputIdx, fullTx);
    if (!built || !built.outputData || built.outputData === "0x") continue;

    try {
      const decoded = VoteCodec.decode(built.outputData);
      const outKey = `${entry.tx_hash}:${outputIdx}`;
      votes.push({
        cell: built.cell,
        decoded,
        blockNumber: parseInt(entry.block_number, 16),
        blockNumberHex: entry.block_number,
        isLive: !consumedSet.has(outKey),
      });
    } catch {
      // skip malformed cells
    }
  }
  return votes;
}

// ─── Command ─────────────────────────────────────────────────────────────────

export function registerQuery(program: Command): void {
  program
    .command("query")
    .description(
      "Query proposal and vote history on chain (includes consumed cells)",
    )
    .option("--rpc-url <url>", "CKB RPC endpoint")
    .option("--config <path>", "JSON NetworkConfig file (defaults to devnet)")
    .option(
      "--count <n>",
      "Number of latest proposals to show, sorted by block number",
      (v) => parseInt(v, 10),
      3,
    )
    .action(async (opts) => {
      try {
        const count = opts.count as number;
        const config = resolveNetworkConfig(opts.config, opts.rpcUrl);
        const client = buildClient(config.ckbRpcUrl, config.knownScripts);
        const txCache = new Map<string, RpcFullTx>();
        const tsCache = new Map<string, string>();

        const allProposals = await collectProposals(
          config.ckbRpcUrl,
          config,
          txCache,
        );

        // Sort newest-first, take top `count`.
        allProposals.sort((a, b) => b.blockNumber - a.blockNumber);
        const proposals = allProposals.slice(0, count);

        // Fetch votes for each selected proposal in parallel.
        const votesPerProposal = await Promise.all(
          proposals.map((p) => {
            const voteTypeArgs = blake160(p.cell.cellOutput.type!.toBytes());
            return collectVotesForProposal(
              config.ckbRpcUrl,
              config,
              voteTypeArgs,
              txCache,
            );
          }),
        );

        console.log(
          `=== CKB Vote Query Results (latest ${proposals.length}) ===\n`,
        );

        if (proposals.length === 0) {
          console.log("No proposal cells found on chain.");
        }

        for (let pi = 0; pi < proposals.length; pi++) {
          const {
            cell,
            decoded: proposalData,
            blockNumber,
            blockNumberHex,
            isLive,
          } = proposals[pi];
          const outPoint = cell.outPoint!;
          const status = isLive ? "ACTIVE" : "CONSUMED";
          const relatedVotes = votesPerProposal[pi];
          const timestamp = await getTimestampForBlock(
            config.ckbRpcUrl,
            blockNumberHex,
            tsCache,
          );

          console.log(
            `── Proposal ${pi + 1}/${proposals.length} [${status}] ──`,
          );
          console.log(`  cell:  ${outPoint.txHash}:${outPoint.index} (output)`);
          console.log(`  block: #${blockNumber}`);
          console.log(`  time:  ${timestamp}`);

          const descBytes = ccc.bytesFrom(proposalData.description);
          const description = new TextDecoder().decode(descBytes);
          console.log(`  description: ${description}`);
          console.log(`  duration: ${proposalData.duration} blocks`);
          console.log(
            `  amount: ${(Number(proposalData.amount) / 1e8).toFixed(2)} CKB`,
          );

          try {
            const receiverAddr = ccc.Address.fromScript(
              proposalData.receiver,
              client,
            );
            console.log(`  receiver: ${receiverAddr}`);
          } catch {
            console.log(
              `  receiver: (script) codeHash=${proposalData.receiver.codeHash}, args=${proposalData.receiver.args}`,
            );
          }
          console.log(
            `  minimal_requirement: ${Number(proposalData.minimalRequirement).toFixed(0)} CKB`,
          );

          if (relatedVotes.length > 0) {
            console.log(`  votes (${relatedVotes.length}):`);
            for (let vi = 0; vi < relatedVotes.length; vi++) {
              const {
                cell: voteCell,
                decoded: voteData,
                blockNumber: vBlockNum,
                isLive: vLive,
              } = relatedVotes[vi];
              const vOutPoint = voteCell.outPoint!;
              const voteText = voteData.vote === 1 ? "YES" : "NO";
              const voteAmount = (Number(voteData.amount) / 1e8).toFixed(2);
              const cellStatus = vLive ? "output" : "consumed";

              let line = `    ${vi + 1}. ${vOutPoint.txHash}:${vOutPoint.index} (${cellStatus})`;
              line += ` - ${voteText}`;
              line += `, ${voteAmount} CKB`;
              line += `, block #${vBlockNum}`;
              console.log(line);
            }
          } else {
            console.log(`  votes: (none)`);
          }
          console.log();
        }

        const totalVoteCount = votesPerProposal.reduce(
          (sum, vs) => sum + vs.length,
          0,
        );
        console.log(
          `=== Summary: ${proposals.length} proposal(s), ${totalVoteCount} vote(s) ===`,
        );
      } catch (err) {
        die(err);
      }
    });
}
