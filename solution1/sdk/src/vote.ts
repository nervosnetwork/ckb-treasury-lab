/**
 * Vote cells: a DAO deposit backed certificate of support.
 *
 * See `docs/vote-type-script-spec.md`.
 */

import { ccc } from "@ckb-ccc/shell";
import type { DeploymentConfig } from "./config.js";
import { feeRateOf } from "./config.js";
import { Direction, ProposalStatus, parseDirection } from "./constants.js";
import { encodeVote } from "./codec.js";
import { getProposalCell, loadConfigCell } from "./query.js";
import {
  addHeaderDeps,
  addUniqueCellDeps,
  cellDepFromDeployment,
  formatOutPoint,
  prioritizeCellDeps,
  requireCellHeader,
  requireLiveCell,
  resolveLock,
  scriptId,
  signerLock,
  voteTypeScript,
} from "./utils.js";

/** Parameters of a cast vote. */
export interface CastVoteParams {
  /** The open proposal to vote on. */
  proposalOutPoint: ccc.OutPointLike;
  /** `"yes"` / `"no"` (or `Direction`). */
  direction: Direction | "yes" | "no";
  /**
   * The DAO deposits backing the vote. When omitted, every deposit of the
   * voter that is older than the proposal is used.
   */
  deposits?: ccc.OutPointLike[];
  /** The voter's lock; defaults to the signer's lock. */
  lock?: string | ccc.ScriptLike;
}

/** The result of {@link castVote}. */
export interface CastVoteResult {
  txHash: ccc.Hex;
  voteCell: { txHash: ccc.Hex; index: number };
  voteTypeScript: ccc.Script;
  /** The amount certified, that is the sum of the backing deposits. */
  voteAmount: bigint;
  /** The deposits that were referenced. */
  deposits: ccc.OutPointLike[];
  /** Deposits that were skipped because they are newer than the proposal. */
  skipped: ccc.OutPointLike[];
}

/**
 * Casts a vote.
 *
 * A vote cell is backed by the voter's DAO deposits, which are referenced
 * through `cell_deps` together with the proposal cell and the block that
 * created each of them: the contract compares every deposit's age with the
 * proposal's.
 *
 * The deposits are moved in front of every other dependency before the
 * transaction is signed: the contract stops scanning `cell_deps` at the first
 * dependency group (`end_of_dao_deposit`), and the signer adds its lock's
 * dependency - a dep group on a public chain - while it completes the fee.
 */
export async function castVote(
  signer: ccc.Signer,
  config: DeploymentConfig,
  params: CastVoteParams,
): Promise<CastVoteResult> {
  const client = signer.client;
  const direction = parseDirection(params.direction);
  const info = await loadConfigCell(client, config);
  const proposal = await getProposalCell(
    client,
    config,
    params.proposalOutPoint,
  );
  if (proposal.data.status !== ProposalStatus.Open) {
    throw new Error(
      `the proposal is not open (status ${proposal.data.status}); a vote can only ` +
        `be cast before the proposal is finalized`,
    );
  }

  const lock = params.lock
    ? await resolveLock(client, params.lock)
    : await signerLock(signer);

  // The proposal's creating block bounds every deposit: a vote may only be
  // backed by assets that existed before the proposal did.
  const proposalHeader = await requireCellHeader(
    client,
    params.proposalOutPoint,
  );

  const daoType = await ccc.Script.fromKnownScript(
    client,
    ccc.KnownScript.NervosDao,
    "0x",
  );

  let candidates: ccc.OutPointLike[];
  if (params.deposits) {
    candidates = params.deposits;
  } else {
    const found: ccc.OutPointLike[] = [];
    for await (const cell of client.findCellsByLock(lock, daoType, true)) {
      if (await cell.isNervosDao(client, "deposited")) {
        found.push(cell.outPoint);
      }
    }
    candidates = found;
  }

  const deposits: ccc.OutPointLike[] = [];
  const skipped: ccc.OutPointLike[] = [];
  const seen = new Set<string>();
  let voteAmount = 0n;
  for (const outPoint of candidates) {
    // `cell_deps` may not reach the same cell twice, and summing it twice
    // would not match the amount the contract computes.
    const key = formatOutPoint(outPoint).toLowerCase();
    if (seen.has(key)) {
      throw new Error(`deposit ${key} is listed twice`);
    }
    seen.add(key);
    const cell = await requireLiveCell(client, outPoint);
    const header = await requireCellHeader(client, outPoint);
    if (header.number >= proposalHeader.number) {
      skipped.push(outPoint);
      continue;
    }
    if (!cell.cellOutput.type?.eq(daoType)) {
      throw new Error(
        `cell ${formatOutPoint(outPoint)} is not a Nervos DAO deposit`,
      );
    }
    if (!cell.cellOutput.lock.eq(lock)) {
      throw new Error(
        `deposit ${formatOutPoint(outPoint)} is not locked by the voter lock`,
      );
    }
    deposits.push(outPoint);
    voteAmount += cell.cellOutput.capacity;
  }

  if (deposits.length === 0) {
    throw new Error(
      skipped.length > 0
        ? "every DAO deposit of the voter was created after the proposal"
        : "the voter has no DAO deposit to back the vote with",
    );
  }
  if (voteAmount < info.data.minimalVoteAmount) {
    throw new Error(
      `the vote amount ${voteAmount} is below config.minimal_vote_amount ` +
        `${info.data.minimalVoteAmount}`,
    );
  }

  const proposalId = scriptId(proposal.typeScript);
  const typeScript = voteTypeScript(config, proposalId);
  const data = encodeVote({ voteAmount, direction });

  const tx = ccc.Transaction.from({
    outputs: [
      ccc.CellOutput.from({ capacity: 0n, lock, type: typeScript }, data),
    ],
    outputsData: [data],
  });
  addUniqueCellDeps(tx, [
    cellDepFromDeployment(config.scripts.vote),
    ccc.CellDep.from({ outPoint: info.cell.outPoint, depType: "code" }),
    ccc.CellDep.from({
      outPoint: proposal.cell.outPoint,
      depType: "code",
    }),
    ...deposits.map((outPoint) =>
      ccc.CellDep.from({ outPoint, depType: "code" }),
    ),
  ]);
  await addHeaderDeps(tx, client, [params.proposalOutPoint, ...deposits]);

  await tx.completeInputsByCapacity(signer);
  if (!tx.inputs.some((input) => input.cellOutput?.lock.eq(lock))) {
    throw new Error(
      "the transaction has no input locked by the voter lock, so the vote " +
        "would be rejected: sign with the owner of the DAO deposits",
    );
  }

  await tx.completeFeeBy(signer, feeRateOf(config));
  // Completing the fee may have appended the signer's lock dependency, which
  // is a dependency group on a public chain. The contract only counts the DAO
  // deposits written before the first group, so put them back in front.
  prioritizeCellDeps(tx, deposits);
  const txHash = await signer.sendTransaction(tx);

  return {
    txHash,
    voteCell: { txHash, index: 0 },
    voteTypeScript: typeScript,
    voteAmount,
    deposits,
    skipped,
  };
}

/**
 * Withdraws a vote: the vote cell is consumed and its capacity recycled.
 *
 * Withdrawing is always possible, even while the proposal is still open.
 */
export async function withdrawVote(
  signer: ccc.Signer,
  config: DeploymentConfig,
  params: { voteOutPoint: ccc.OutPointLike; lock?: string | ccc.ScriptLike },
): Promise<ccc.Hex> {
  const client = signer.client;
  const vote = await requireLiveCell(client, params.voteOutPoint);
  const lock = params.lock
    ? await resolveLock(client, params.lock)
    : await signerLock(signer);

  const tx = ccc.Transaction.from({
    inputs: [
      {
        previousOutput: vote.outPoint,
        cellOutput: vote.cellOutput,
        outputData: vote.outputData,
      },
    ],
    outputs: [{ capacity: vote.cellOutput.capacity, lock }],
    outputsData: ["0x"],
  });
  addUniqueCellDeps(tx, [cellDepFromDeployment(config.scripts.vote)]);

  await tx.completeFeeBy(signer, feeRateOf(config));
  return signer.sendTransaction(tx);
}
