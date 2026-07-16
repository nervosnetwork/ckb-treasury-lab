import { ccc } from "@ckb-ccc/shell";
import { DEVNET_CONFIG, type NetworkConfig } from "./config.js";
import { VoteCodec } from "./codec.js";
import {
  blake160,
  cellDepFromInfo,
  cellDepFromOutPoint,
  getRequiredCell,
  getSignerLock,
  mergeConfig,
} from "./utils.js";

export interface CreateVoteParams {
  proposalOutPoint: { txHash: string; index: number };
  vote: "yes" | "no";
  config?: Partial<NetworkConfig>;
}

export interface CreateVoteResult {
  txHash: string;
  /** Outpoint of the created vote cell */
  voteOutPoint: { txHash: string; index: number };
  /** Total DAO capacity voted, in shannons */
  capacityVoted: bigint;
}

/**
 * Cast a vote on a proposal.
 *
 * All DAO deposit cells owned by the signer are automatically discovered and
 * added as cell_deps. Their indices in cell_deps are recorded in dao_index.
 *
 * Cell dep layout (stable indices):
 *   [0]      vote-type-script contract
 *   [1]      proposal cell (so the vote type script can verify it)
 *   [2..N+1] DAO deposit cells  →  dao_index = [2, 3, ..., N+1]
 *   [N+2]    secp256k1 lock contract (added automatically by signer during signing)
 */
export async function createVote(
  signer: ccc.Signer,
  params: CreateVoteParams,
): Promise<CreateVoteResult> {
  const config = mergeConfig(DEVNET_CONFIG, params.config);
  const client = signer.client;

  // Fetch the proposal cell to get its type script
  const proposalCell = await getRequiredCell(client, params.proposalOutPoint);
  if (!proposalCell.cellOutput.type) {
    throw new Error("Proposal cell has no type script");
  }
  const proposalTypeScript = proposalCell.cellOutput.type;

  // vote type script args = blake160(proposalTypeScript.toBytes())
  const voteTypeArgs = blake160(proposalTypeScript.toBytes());
  const voteTypeScript = ccc.Script.from({
    codeHash: config.voteTypeScript.codeHash,
    hashType: config.voteTypeScript.hashType,
    args: voteTypeArgs,
  });

  // Get signer lock (used to find DAO deposits)
  const signerLock = await getSignerLock(signer);

  // Discover all DAO deposit cells owned by the signer
  const daoType = await ccc.Script.fromKnownScript(
    client,
    ccc.KnownScript.NervosDao,
    "0x",
  );
  const daoCells: ccc.Cell[] = [];
  for await (const cell of client.findCellsByLock(signerLock, daoType, true)) {
    // Only deposited phase (outputData == "0x0000000000000000")
    if (await cell.isNervosDao(client, "deposited")) {
      daoCells.push(cell);
    }
  }

  if (daoCells.length === 0) {
    throw new Error(
      "No DAO deposit cells found for this signer. Deposit CKB into Nervos DAO first.",
    );
  }

  // Sum DAO deposit capacities
  const totalDaoCapacity = daoCells.reduce(
    (sum, cell) => sum + cell.cellOutput.capacity,
    0n,
  );

  // Build cell deps in stable order
  // Index 0: vote-type-script contract
  // Index 1: proposal cell
  // Index 2+: DAO deposit cells
  const voteTypeScriptDep = cellDepFromInfo(config.voteTypeScript);
  const proposalCellDep = cellDepFromOutPoint(params.proposalOutPoint);

  const daoBaseIndex = 2;
  const daoIndexes = daoCells.map((_, i) => daoBaseIndex + i);

  // Encode vote cell data
  const voteData = VoteCodec.encode({
    vote: params.vote === "yes" ? 1 : 0,
    amount: totalDaoCapacity,
    daoIndex: daoIndexes,
  });
  const voteDataHex = ccc.hexFrom(voteData);

  // Build vote cell output with auto-calculated minimum capacity
  const voteOutput = ccc.CellOutput.from(
    {
      capacity: 0n,
      lock: signerLock,
      type: voteTypeScript,
    },
    voteDataHex,
  );

  // Assemble transaction
  const tx = ccc.Transaction.from({
    outputs: [voteOutput],
    outputsData: [voteDataHex],
  });

  // Add cell deps in the planned order
  tx.addCellDeps(voteTypeScriptDep); // index 0
  tx.addCellDeps(proposalCellDep); // index 1
  for (const daoCell of daoCells) {
    if (!daoCell.outPoint) {
      throw new Error("DAO cell has no outpoint");
    }
    tx.addCellDeps(
      cellDepFromOutPoint({
        txHash: daoCell.outPoint.txHash,
        index: daoCell.outPoint.index,
      }),
    ); // indices 2+
  }

  // Collect inputs for capacity + fee (signer's secp256k1 cells)
  await tx.completeInputsByCapacity(signer);
  await tx.completeFeeBy(signer, config.feeRate);

  const txHash = await signer.sendTransaction(tx);
  return {
    txHash,
    voteOutPoint: { txHash, index: 0 },
    capacityVoted: totalDaoCapacity,
  };
}

/**
 * Consume (recycle) a vote cell to reclaim the occupied CKB.
 * Anyone can consume a vote cell at any time — no special proof required.
 */
export async function consumeVote(
  signer: ccc.Signer,
  voteOutPoint: { txHash: string; index: number },
  params?: { config?: Partial<NetworkConfig> },
): Promise<string> {
  const config = mergeConfig(DEVNET_CONFIG, params?.config);
  const client = signer.client;

  const voteCell = await getRequiredCell(client, voteOutPoint);
  const signerLock = await getSignerLock(signer);

  const tx = ccc.Transaction.from({
    inputs: [
      {
        previousOutput: {
          txHash: voteOutPoint.txHash,
          index: voteOutPoint.index,
        },
      },
    ],
    outputs: [
      {
        capacity: voteCell.cellOutput.capacity,
        lock: signerLock,
      },
    ],
    outputsData: ["0x"],
  });

  // vote-type-script contract needed for the VM to verify the type script
  tx.addCellDeps(cellDepFromInfo(config.voteTypeScript));

  await tx.completeFeeBy(signer, config.feeRate);
  return signer.sendTransaction(tx);
}
