/**
 * The proposal cell lifecycle.
 *
 * One type script covers all three states (see
 * `docs/proposal-type-script-spec.md`): `open -> finalized -> passed`, plus
 * the settlement paths challenge, veto, recycle and grant.
 */

import { ccc } from "@ckb-ccc/shell";
import type { DeploymentConfig } from "./config.js";
import { feeRateOf } from "./fee.js";
import { Direction, ProposalStatus } from "./constants.js";
import { encodeProposalCellData, type ProposalCellData } from "./codec.js";
import {
  getCountingCell,
  loadConfigCell,
  getProposalCell,
  type CountingCellInfo,
} from "./query.js";
import {
  addHeaderDeps,
  addUniqueCellDeps,
  alwaysSuccessScript,
  blake160,
  cellDepFromDeployment,
  formatOutPoint,
  relativeSince,
  requireCellHeader,
  resolveLock,
  scriptHash,
  scriptFromDeployment,
  scriptId,
  signerLock,
  typeId,
  waitForBlockNumber,
} from "./utils.js";

/** Parameters of a new proposal. */
export interface CreateProposalParams {
  /** UTF-8 text describing the proposal. */
  description: string;
  /** Shannons granted to the recipient when the proposal passes. */
  requestedAmount: bigint | number;
  /** Recipient of the grant; defaults to the signer's lock. */
  recipient?: string | ccc.ScriptLike;
  /** Bond of the proposal, in shannons; defaults to the configured minimum. */
  capacity?: bigint | number;
  /** Lock controlling the proposal cell; defaults to the signer's lock. */
  lock?: string | ccc.ScriptLike;
}

/** The result of {@link createProposal}. */
export interface CreateProposalResult {
  txHash: ccc.Hex;
  proposalCell: { txHash: ccc.Hex; index: number };
  proposalTypeScript: ccc.Script;
  /** `blake160(proposal type script)`, the args of its votes and counting cells. */
  proposalId: ccc.Hex;
  proposalArgs: ccc.Hex;
}

function requireStatus(
  data: ProposalCellData,
  expected: ProposalStatus,
  outPoint: ccc.OutPointLike,
): void {
  if (data.status !== expected) {
    throw new Error(
      `proposal ${formatOutPoint(outPoint)} is in status ${data.status}, expected ${expected}`,
    );
  }
}

function loadCountingCells(
  client: ccc.Client,
  config: DeploymentConfig,
  outPoints: ccc.OutPointLike[],
): Promise<CountingCellInfo[]> {
  return Promise.all(
    outPoints.map((op) => getCountingCell(client, config, op)),
  );
}

/**
 * Checks that the counting cells certify `proposalId` in `direction` and that
 * their hash ranges do not overlap, then returns the certified amount.
 */
function sumCountingCells(
  cells: CountingCellInfo[],
  proposalId: ccc.Hex,
  direction: Direction,
): bigint {
  let total = 0n;
  const ranges: Array<[number, number]> = [];
  for (const { cell, typeScript, data } of cells) {
    if (typeScript.args !== proposalId) {
      throw new Error(
        `counting cell ${formatOutPoint(cell.outPoint)} counts another proposal`,
      );
    }
    if (data.direction !== direction) {
      throw new Error(
        `counting cell ${formatOutPoint(cell.outPoint)} has direction ` +
          `${data.direction}, expected ${direction}`,
      );
    }
    if (data.startHash > data.endHash) {
      throw new Error(
        `counting cell ${formatOutPoint(cell.outPoint)} has an empty hash range`,
      );
    }
    ranges.push([data.startHash, data.endHash]);
    total += data.voteAmount;
  }

  ranges.sort((a, b) => a[0] - b[0]);
  for (let i = 1; i < ranges.length; i++) {
    if (ranges[i][0] <= ranges[i - 1][1]) {
      throw new Error("the hash ranges of the counting cells overlap");
    }
  }
  return total;
}

/**
 * Adds the counting cells' creating headers to `tx` and rejects any cell
 * created before `origin + config.vote_duration`.
 */
async function addCountingCellHeaders(
  tx: ccc.Transaction,
  client: ccc.Client,
  counting: CountingCellInfo[],
  origin: bigint,
  voteDuration: bigint,
): Promise<void> {
  const headers = await addHeaderDeps(
    tx,
    client,
    counting.map(({ cell }) => cell.outPoint),
  );
  for (const [index, header] of headers.entries()) {
    if (header.number <= origin + voteDuration) {
      throw new Error(
        `counting cell ${formatOutPoint(counting[index].cell.outPoint)} was ` +
          `created in block ${header.number}, before config.vote_duration ` +
          `elapsed at block ${origin + voteDuration + 1n}; collect the votes ` +
          `later (see createCountingCell({ wait: true }))`,
      );
    }
  }
}

/**
 * Creates a proposal cell.
 *
 * The proposal type script is a Type ID, so its args are computed from the
 * first input after the inputs are collected.
 */
export async function createProposal(
  signer: ccc.Signer,
  config: DeploymentConfig,
  params: CreateProposalParams,
): Promise<CreateProposalResult> {
  const client = signer.client;
  const info = await loadConfigCell(client, config);

  const lock = params.lock
    ? await resolveLock(client, params.lock)
    : await signerLock(signer);
  const recipientLock = params.recipient
    ? await resolveLock(client, params.recipient)
    : lock;

  const requestedAmount = BigInt(params.requestedAmount);
  const capacity = BigInt(params.capacity ?? info.data.minimalProposalCapacity);
  if (capacity < info.data.minimalProposalCapacity) {
    throw new Error(
      `the bond ${capacity} is below config.minimal_proposal_capacity ` +
        `${info.data.minimalProposalCapacity}`,
    );
  }

  const data = encodeProposalCellData({
    status: ProposalStatus.Open,
    description: params.description,
    requestedAmount,
    recipientLockHash: blake160(recipientLock.toBytes()),
    totalYes: 0n,
    originBlockNumber: 0n,
  });
  const placeholder = scriptFromDeployment(
    config.scripts.proposal,
    `${info.id}${"00".repeat(20)}`,
  );

  const tx = ccc.Transaction.from({
    outputs: [ccc.CellOutput.from({ capacity, lock, type: placeholder }, data)],
    outputsData: [data],
  });
  addUniqueCellDeps(tx, [
    cellDepFromDeployment(config.scripts.proposal),
    ccc.CellDep.from({
      outPoint: info.cell.outPoint,
      depType: "code",
    }),
  ]);

  await tx.completeInputsByCapacity(signer);
  const first = tx.inputs[0];
  if (!first) {
    throw new Error(
      "no input was collected; the signer has no spendable cells",
    );
  }

  const proposalArgs = ccc.hexFrom(`${info.id}${typeId(first, 0).slice(2)}`);
  const proposalTypeScript = scriptFromDeployment(
    config.scripts.proposal,
    proposalArgs,
  );
  tx.outputs[0].type = proposalTypeScript;

  await tx.completeFeeBy(signer, feeRateOf(config));
  const txHash = await signer.sendTransaction(tx);

  return {
    txHash,
    proposalCell: { txHash, index: 0 },
    proposalTypeScript,
    proposalId: scriptId(proposalTypeScript),
    proposalArgs,
  };
}

/** Finalizes an open proposal: `open -> finalized`. */
export async function finalizeProposal(
  signer: ccc.Signer,
  config: DeploymentConfig,
  params: {
    proposalOutPoint: ccc.OutPointLike;
    countingOutPoints: ccc.OutPointLike[];
    /** Wait until the chain reaches the block the `since` requires. */
    wait?: boolean;
  },
): Promise<ccc.Hex> {
  const client = signer.client;
  const info = await loadConfigCell(client, config);
  const proposal = await getProposalCell(
    client,
    config,
    params.proposalOutPoint,
  );
  requireStatus(proposal.data, ProposalStatus.Open, params.proposalOutPoint);

  const proposalId = scriptId(proposal.typeScript);
  const counting = await loadCountingCells(
    client,
    config,
    params.countingOutPoints,
  );
  const totalYes = sumCountingCells(counting, proposalId, Direction.Yes);
  if (totalYes < info.data.yesThreshold) {
    throw new Error(
      `${totalYes} "YES" shannons are certified, below config.yes_threshold ` +
        `${info.data.yesThreshold}`,
    );
  }

  const origin = await requireCellHeader(client, params.proposalOutPoint);
  if (params.wait) {
    await waitForBlockNumber(
      client,
      origin.number + info.data.voteDuration + 1n,
    );
  }
  const data = encodeProposalCellData({
    ...proposal.data,
    status: ProposalStatus.Finalized,
    totalYes,
    originBlockNumber: origin.number,
  });

  const tx = ccc.Transaction.from({
    inputs: [
      {
        previousOutput: proposal.cell.outPoint,
        since: relativeSince(info.data.voteDuration + 1n),
        cellOutput: proposal.cell.cellOutput,
        outputData: proposal.cell.outputData,
      },
      ...counting.map(({ cell }) => ({
        previousOutput: cell.outPoint,
        cellOutput: cell.cellOutput,
        outputData: cell.outputData,
      })),
    ],
    outputs: [
      ccc.CellOutput.from(
        {
          capacity: proposal.cell.cellOutput.capacity,
          lock: alwaysSuccessScript(config),
          type: proposal.typeScript,
        },
        data,
      ),
    ],
    outputsData: [data],
  });
  addUniqueCellDeps(tx, [
    cellDepFromDeployment(config.scripts.proposal),
    ccc.CellDep.from({ outPoint: info.cell.outPoint, depType: "code" }),
    cellDepFromDeployment(config.scripts.alwaysSuccess),
    cellDepFromDeployment(config.scripts.counting),
  ]);
  await addHeaderDeps(tx, client, [params.proposalOutPoint]);
  await addCountingCellHeaders(
    tx,
    client,
    counting,
    origin.number,
    info.data.voteDuration,
  );

  await tx.completeFeeBy(signer, feeRateOf(config));
  return signer.sendTransaction(tx);
}

/** Passes a finalized proposal: `finalized -> passed`. */
export async function passProposal(
  signer: ccc.Signer,
  config: DeploymentConfig,
  params: {
    proposalOutPoint: ccc.OutPointLike;
    lock?: string | ccc.ScriptLike;
    /** Wait until the chain reaches the block the `since` requires. */
    wait?: boolean;
  },
): Promise<ccc.Hex> {
  const client = signer.client;
  const info = await loadConfigCell(client, config);
  const proposal = await getProposalCell(
    client,
    config,
    params.proposalOutPoint,
  );
  requireStatus(
    proposal.data,
    ProposalStatus.Finalized,
    params.proposalOutPoint,
  );
  if (params.wait) {
    const header = await requireCellHeader(client, params.proposalOutPoint);
    await waitForBlockNumber(
      client,
      header.number + info.data.challengeTime + 1n,
    );
  }

  const lock = params.lock
    ? await resolveLock(client, params.lock)
    : await signerLock(signer);
  if (blake160(lock.toBytes()) !== proposal.data.recipientLockHash) {
    throw new Error(
      `the lock of the passed cell does not hash to the proposal's ` +
        `recipient_lock_hash ${proposal.data.recipientLockHash}`,
    );
  }
  const data = encodeProposalCellData({
    ...proposal.data,
    status: ProposalStatus.Passed,
  });

  const tx = ccc.Transaction.from({
    inputs: [
      {
        previousOutput: proposal.cell.outPoint,
        since: relativeSince(info.data.challengeTime + 1n),
        cellOutput: proposal.cell.cellOutput,
        outputData: proposal.cell.outputData,
      },
    ],
    outputs: [
      ccc.CellOutput.from(
        {
          capacity: proposal.cell.cellOutput.capacity,
          lock,
          type: proposal.typeScript,
        },
        data,
      ),
    ],
    outputsData: [data],
  });
  addUniqueCellDeps(tx, [
    cellDepFromDeployment(config.scripts.proposal),
    ccc.CellDep.from({ outPoint: info.cell.outPoint, depType: "code" }),
    cellDepFromDeployment(config.scripts.alwaysSuccess),
  ]);

  await tx.completeFeeBy(signer, feeRateOf(config));
  return signer.sendTransaction(tx);
}

/**
 * Challenges a finalized proposal with "NO" counting cells.
 *
 * The bond is handed to `rewardLock`, which must be the lock of one of the
 * counting cells: the contract only accepts a reward that reaches the
 * challenger who certified the "NO" votes.
 */
export async function challengeProposal(
  signer: ccc.Signer,
  config: DeploymentConfig,
  params: {
    proposalOutPoint: ccc.OutPointLike;
    countingOutPoints: ccc.OutPointLike[];
    rewardLock?: string | ccc.ScriptLike;
  },
): Promise<ccc.Hex> {
  const client = signer.client;
  const info = await loadConfigCell(client, config);
  const proposal = await getProposalCell(
    client,
    config,
    params.proposalOutPoint,
  );
  requireStatus(
    proposal.data,
    ProposalStatus.Finalized,
    params.proposalOutPoint,
  );

  const proposalId = scriptId(proposal.typeScript);
  const counting = await loadCountingCells(
    client,
    config,
    params.countingOutPoints,
  );
  const totalNo = sumCountingCells(counting, proposalId, Direction.No);
  if (totalNo < proposal.data.totalYes) {
    throw new Error(
      `${totalNo} "NO" shannons are certified, below the ${proposal.data.totalYes} ` +
        `"YES" shannons of the finalized proposal`,
    );
  }

  const rewardLock = params.rewardLock
    ? await resolveLock(client, params.rewardLock)
    : await signerLock(signer);
  const rewardHash = scriptHash(rewardLock);
  if (
    !counting.some(
      ({ cell }) => scriptHash(cell.cellOutput.lock) === rewardHash,
    )
  ) {
    throw new Error(
      "the reward lock is not the lock of any counting cell, so the contract " +
        "would reject the challenge",
    );
  }

  const tx = ccc.Transaction.from({
    inputs: [
      {
        previousOutput: proposal.cell.outPoint,
        since: relativeSince(0),
        cellOutput: proposal.cell.cellOutput,
        outputData: proposal.cell.outputData,
      },
      ...counting.map(({ cell }) => ({
        previousOutput: cell.outPoint,
        cellOutput: cell.cellOutput,
        outputData: cell.outputData,
      })),
    ],
    outputs: [
      { capacity: proposal.cell.cellOutput.capacity, lock: rewardLock },
    ],
    outputsData: ["0x"],
  });
  addUniqueCellDeps(tx, [
    cellDepFromDeployment(config.scripts.proposal),
    ccc.CellDep.from({ outPoint: info.cell.outPoint, depType: "code" }),
    cellDepFromDeployment(config.scripts.alwaysSuccess),
    cellDepFromDeployment(config.scripts.counting),
  ]);
  await addCountingCellHeaders(
    tx,
    client,
    counting,
    proposal.data.originBlockNumber,
    info.data.voteDuration,
  );

  await tx.completeFeeBy(signer, feeRateOf(config));
  return signer.sendTransaction(tx);
}

/** Recycles the bond of a proposal that did not pass. */
export async function recycleProposal(
  signer: ccc.Signer,
  config: DeploymentConfig,
  params: {
    proposalOutPoint: ccc.OutPointLike;
    lock?: string | ccc.ScriptLike;
    /** Wait until the chain reaches the block the `since` requires. */
    wait?: boolean;
  },
): Promise<ccc.Hex> {
  const client = signer.client;
  const info = await loadConfigCell(client, config);
  const proposal = await getProposalCell(
    client,
    config,
    params.proposalOutPoint,
  );
  if (params.wait) {
    const header = await requireCellHeader(client, params.proposalOutPoint);
    await waitForBlockNumber(
      client,
      header.number + info.data.voteDuration + info.data.challengeTime + 1n,
    );
  }
  if (
    proposal.data.status !== ProposalStatus.Open &&
    proposal.data.status !== ProposalStatus.Finalized
  ) {
    throw new Error(
      `only an open or finalized proposal can be recycled, not status ` +
        `${proposal.data.status}`,
    );
  }

  const lock = params.lock
    ? await resolveLock(client, params.lock)
    : await signerLock(signer);
  const elapsed = info.data.voteDuration + info.data.challengeTime + 1n;

  const tx = ccc.Transaction.from({
    inputs: [
      {
        previousOutput: proposal.cell.outPoint,
        since: relativeSince(elapsed),
        cellOutput: proposal.cell.cellOutput,
        outputData: proposal.cell.outputData,
      },
    ],
    outputs: [{ capacity: proposal.cell.cellOutput.capacity, lock }],
    outputsData: ["0x"],
  });
  addUniqueCellDeps(tx, [
    cellDepFromDeployment(config.scripts.proposal),
    ccc.CellDep.from({ outPoint: info.cell.outPoint, depType: "code" }),
    cellDepFromDeployment(config.scripts.alwaysSuccess),
  ]);

  await tx.completeFeeBy(signer, feeRateOf(config));
  return signer.sendTransaction(tx);
}

/** Vetoes a finalized proposal with the administrator's lock. */
export async function vetoProposal(
  signer: ccc.Signer,
  config: DeploymentConfig,
  params: {
    proposalOutPoint: ccc.OutPointLike;
    lock?: string | ccc.ScriptLike;
  },
): Promise<ccc.Hex> {
  const client = signer.client;
  const info = await loadConfigCell(client, config);
  const proposal = await getProposalCell(
    client,
    config,
    params.proposalOutPoint,
  );
  requireStatus(
    proposal.data,
    ProposalStatus.Finalized,
    params.proposalOutPoint,
  );

  const lock = params.lock
    ? await resolveLock(client, params.lock)
    : await signerLock(signer);
  if (scriptHash(lock) !== info.data.vetoLockScriptHash) {
    throw new Error(
      "the signer does not control config.veto_lock_script_hash, so it cannot veto",
    );
  }

  const tx = ccc.Transaction.from({
    inputs: [
      {
        previousOutput: proposal.cell.outPoint,
        since: relativeSince(0),
        cellOutput: proposal.cell.cellOutput,
        outputData: proposal.cell.outputData,
      },
    ],
    outputs: [{ capacity: proposal.cell.cellOutput.capacity, lock }],
    outputsData: ["0x"],
  });
  addUniqueCellDeps(tx, [
    cellDepFromDeployment(config.scripts.proposal),
    ccc.CellDep.from({ outPoint: info.cell.outPoint, depType: "code" }),
    cellDepFromDeployment(config.scripts.alwaysSuccess),
  ]);

  await tx.completeFeeBy(signer, feeRateOf(config));
  return signer.sendTransaction(tx);
}

/**
 * Receives the grant of a passed proposal.
 *
 * The contract only checks that an output pays `recipient_lock_hash` at least
 * `requested_amount`; the assets themselves come from a treasury provider,
 * which is modelled here as the signer's own cells.
 */
export async function receiveGrant(
  signer: ccc.Signer,
  config: DeploymentConfig,
  params: {
    proposalOutPoint: ccc.OutPointLike;
    /** Recipient of the grant; defaults to the signer's lock. */
    recipient?: string | ccc.ScriptLike;
    /** Amount to deliver, in shannons; defaults to `requested_amount`. */
    amount?: bigint | number;
  },
): Promise<ccc.Hex> {
  const client = signer.client;
  const info = await loadConfigCell(client, config);
  const proposal = await getProposalCell(
    client,
    config,
    params.proposalOutPoint,
  );
  requireStatus(proposal.data, ProposalStatus.Passed, params.proposalOutPoint);

  const recipientLock = params.recipient
    ? await resolveLock(client, params.recipient)
    : await signerLock(signer);
  if (blake160(recipientLock.toBytes()) !== proposal.data.recipientLockHash) {
    throw new Error(
      `the recipient lock does not hash to the proposal's recipient_lock_hash ` +
        `${proposal.data.recipientLockHash}`,
    );
  }

  const amount = BigInt(params.amount ?? proposal.data.requestedAmount);
  if (amount < proposal.data.requestedAmount) {
    throw new Error(
      `the grant is ${amount} shannons, below the requested ` +
        `${proposal.data.requestedAmount}`,
    );
  }

  const tx = ccc.Transaction.from({
    inputs: [
      {
        previousOutput: proposal.cell.outPoint,
        cellOutput: proposal.cell.cellOutput,
        outputData: proposal.cell.outputData,
      },
    ],
    outputs: [{ capacity: amount, lock: recipientLock }],
    outputsData: ["0x"],
  });
  addUniqueCellDeps(tx, [
    cellDepFromDeployment(config.scripts.proposal),
    ccc.CellDep.from({ outPoint: info.cell.outPoint, depType: "code" }),
    cellDepFromDeployment(config.scripts.alwaysSuccess),
  ]);

  await tx.completeInputsByCapacity(signer);
  await tx.completeFeeBy(signer, feeRateOf(config));
  return signer.sendTransaction(tx);
}
