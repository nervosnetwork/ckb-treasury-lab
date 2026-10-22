/**
 * Counting cells: the certificates that aggregate vote cells into a single
 * amount, one hash range at a time.
 *
 * See `docs/counting-type-script-spec.md`.
 */

import { ccc } from "@ckb-ccc/shell";
import type { DeploymentConfig } from "./config.js";
import { feeRateOf } from "./fee.js";
import { Direction, ProposalStatus, parseDirection } from "./constants.js";
import { encodeCounting } from "./codec.js";
import {
  findVoteCells,
  getProposalCell,
  getVoteCell,
  loadConfigCell,
  type VoteCellInfo,
} from "./query.js";
import {
  addHeaderDeps,
  addUniqueCellDeps,
  cellDepFromDeployment,
  countingTypeScript,
  formatOutPoint,
  hashPrefix,
  parseU16,
  requireCellHeader,
  requireLiveCell,
  resolveLock,
  scriptHash,
  scriptId,
  signerLock,
  waitForBlockNumber,
} from "./utils.js";

/** Parameters of a counting cell. */
export interface CreateCountingCellParams {
  /**
   * The proposal the votes belong to: the open cell for "YES", the open or
   * finalized cell for "NO".
   */
  proposalOutPoint: ccc.OutPointLike;
  /** `"yes"` collects a proposal's votes, `"no"` a challenge's. */
  direction: Direction | "yes" | "no";
  /** Lower bound of the hash range, included. */
  startHash: number | string;
  /** Upper bound of the hash range, included. */
  endHash: number | string;
  /**
   * The vote cells to aggregate. When omitted, every vote cell of the
   * proposal inside the range is collected.
   */
  voteOutPoints?: ccc.OutPointLike[];
  /** Lock of the counting cell; defaults to the signer's lock. */
  lock?: string | ccc.ScriptLike;
  /** Wait until `config.vote_duration` elapsed before sending. */
  wait?: boolean;
}

/** The result of {@link createCountingCell}. */
export interface CreateCountingCellResult {
  txHash: ccc.Hex;
  countingCell: { txHash: ccc.Hex; index: number };
  countingTypeScript: ccc.Script;
  /** The certified amount, the sum of the collected vote cells. */
  voteAmount: bigint;
  /** The vote cells that were aggregated. */
  voteCells: ccc.OutPointLike[];
  /** Vote cells of another voter that carries a vote already counted. */
  skippedDuplicates: ccc.OutPointLike[];
}

/**
 * Creates a counting cell.
 *
 * The "YES" votes of a proposal are collected by the initiator, the "NO" votes
 * of a challenge by the challenger; both reference the vote cells through
 * `cell_deps`, and the transaction has to carry the creating block of the
 * proposal and of every vote cell.
 */
export async function createCountingCell(
  signer: ccc.Signer,
  config: DeploymentConfig,
  params: CreateCountingCellParams,
): Promise<CreateCountingCellResult> {
  const client = signer.client;
  const direction = parseDirection(params.direction);
  const startHash = parseU16(params.startHash, "startHash");
  const endHash = parseU16(params.endHash, "endHash");
  if (startHash > endHash) {
    throw new Error(
      `the hash range is empty: startHash ${startHash} > endHash ${endHash}`,
    );
  }

  const info = await loadConfigCell(client, config);
  const proposal = await getProposalCell(
    client,
    config,
    params.proposalOutPoint,
  );
  const isFinalized = proposal.data.status === ProposalStatus.Finalized;
  const allowed =
    direction === Direction.Yes
      ? proposal.data.status === ProposalStatus.Open
      : proposal.data.status === ProposalStatus.Open || isFinalized;
  if (!allowed) {
    throw new Error(
      `a "${direction === Direction.Yes ? "YES" : "NO"}" counting cell cannot ` +
        `reference a proposal in status ${proposal.data.status}`,
    );
  }

  const proposalId = scriptId(proposal.typeScript);
  const proposalHeader = await requireCellHeader(
    client,
    params.proposalOutPoint,
  );
  // A finalized proposal is created after voting ended, so it records the
  // block that created the original open cell; that is the window's origin.
  const windowOrigin = isFinalized
    ? proposal.data.originBlockNumber
    : proposalHeader.number;

  // Collection only starts after the voting duration has elapsed.
  if (params.wait) {
    await waitForBlockNumber(
      client,
      windowOrigin + info.data.voteDuration + 1n,
    );
  }

  // Pick the vote cells to aggregate.
  let candidates: VoteCellInfo[];
  if (params.voteOutPoints) {
    candidates = await Promise.all(
      params.voteOutPoints.map((op) => getVoteCell(client, config, op)),
    );
  } else {
    candidates = [];
    for await (const vote of findVoteCells(client, config, {
      proposalId,
      direction,
    })) {
      if (
        hashPrefix(scriptHash(vote.cell.cellOutput.lock)) >= startHash &&
        hashPrefix(scriptHash(vote.cell.cellOutput.lock)) <= endHash
      ) {
        candidates.push(vote);
      }
    }
  }
  if (candidates.length === 0) {
    throw new Error(
      "no vote cell matches the hash range and direction of this counting cell",
    );
  }

  const skippedDuplicates: ccc.OutPointLike[] = [];
  const byLock = new Map<
    string,
    { outPoint: ccc.OutPointLike; amount: bigint }
  >();
  for (const vote of candidates) {
    if (vote.typeScript.args !== proposalId) {
      throw new Error(
        `vote cell ${formatOutPoint(vote.cell.outPoint)} is for another proposal`,
      );
    }
    if (vote.data.direction !== direction) {
      throw new Error(
        `vote cell ${formatOutPoint(vote.cell.outPoint)} has direction ` +
          `${vote.data.direction}, expected ${direction}`,
      );
    }
    const prefix = hashPrefix(scriptHash(vote.cell.cellOutput.lock));
    if (prefix < startHash || prefix > endHash) {
      throw new Error(
        `the lock of vote cell ${formatOutPoint(vote.cell.outPoint)} falls ` +
          `outside [${startHash}, ${endHash}]`,
      );
    }
    const header = await requireCellHeader(client, vote.cell.outPoint);
    const offset = header.number - windowOrigin;
    if (offset < 0n || offset >= info.data.voteWindow) {
      throw new Error(
        `vote cell ${formatOutPoint(vote.cell.outPoint)} was cast outside ` +
          `config.vote_window (offset ${offset} blocks)`,
      );
    }

    // A voter may only be counted once per counting cell: keep the largest
    // vote of each voter lock.
    const lockKey = scriptHash(vote.cell.cellOutput.lock);
    const previous = byLock.get(lockKey);
    if (previous) {
      skippedDuplicates.push(vote.cell.outPoint);
      if (previous.amount >= vote.data.voteAmount) {
        continue;
      }
    }
    byLock.set(lockKey, {
      outPoint: vote.cell.outPoint,
      amount: vote.data.voteAmount,
    });
  }

  const entries = [...byLock.values()];
  const voteCells = entries.map((entry) => entry.outPoint);
  const voteAmount = entries.reduce((sum, entry) => sum + entry.amount, 0n);

  const lock = params.lock
    ? await resolveLock(client, params.lock)
    : await signerLock(signer);
  const typeScript = countingTypeScript(config, proposalId);
  const data = encodeCounting({ startHash, endHash, direction, voteAmount });

  const tx = ccc.Transaction.from({
    outputs: [
      ccc.CellOutput.from({ capacity: 0n, lock, type: typeScript }, data),
    ],
    outputsData: [data],
  });
  addUniqueCellDeps(tx, [
    cellDepFromDeployment(config.scripts.counting),
    ccc.CellDep.from({ outPoint: info.cell.outPoint, depType: "code" }),
    ccc.CellDep.from({
      outPoint: proposal.cell.outPoint,
      depType: "code",
    }),
    ...voteCells.map((outPoint) =>
      ccc.CellDep.from({ outPoint, depType: "code" }),
    ),
  ]);
  await addHeaderDeps(tx, client, [params.proposalOutPoint, ...voteCells]);

  await tx.completeInputsByCapacity(signer);
  await tx.completeFeeBy(signer, feeRateOf(config));
  const txHash = await signer.sendTransaction(tx);

  return {
    txHash,
    countingCell: { txHash, index: 0 },
    countingTypeScript: typeScript,
    voteAmount,
    voteCells,
    skippedDuplicates,
  };
}

/** Consumes a counting cell and recycles its capacity. */
export async function consumeCountingCell(
  signer: ccc.Signer,
  config: DeploymentConfig,
  params: {
    countingOutPoint: ccc.OutPointLike;
    lock?: string | ccc.ScriptLike;
  },
): Promise<ccc.Hex> {
  const client = signer.client;
  const counting = await requireLiveCell(client, params.countingOutPoint);
  const lock = params.lock
    ? await resolveLock(client, params.lock)
    : await signerLock(signer);

  const tx = ccc.Transaction.from({
    inputs: [
      {
        previousOutput: counting.outPoint,
        cellOutput: counting.cellOutput,
        outputData: counting.outputData,
      },
    ],
    outputs: [{ capacity: counting.cellOutput.capacity, lock }],
    outputsData: ["0x"],
  });
  addUniqueCellDeps(tx, [cellDepFromDeployment(config.scripts.counting)]);

  await tx.completeFeeBy(signer, feeRateOf(config));
  return signer.sendTransaction(tx);
}
