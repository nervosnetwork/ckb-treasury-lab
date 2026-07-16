import { ccc } from "@ckb-ccc/shell";
import { DEVNET_CONFIG, type NetworkConfig } from "./config.js";
import { ProposalCodec } from "./codec.js";
import {
  buildClient,
  buildProposalTypeScriptArgs,
  cellDepFromInfo,
  getRequiredCell,
  getSignerLock,
  hashTypeToByte,
  mergeConfig,
  scriptFromInfo,
} from "./utils.js";

// ─── Create Proposal ─────────────────────────────────────────────────────────

export interface CreateProposalParams {
  duration: number;
  description: string;
  /** CKB address string; defaults to signer's own address */
  receiver?: string;
  /** amount in shannon; defaults to 0n */
  amount?: bigint;
  /** minimal_requirement in CKB; defaults to 0 */
  minimalRequirement?: number;
  config?: Partial<NetworkConfig>;
}

export interface CreateProposalResult {
  txHash: string;
  /** Outpoint of the created proposal cell */
  proposalOutPoint: { txHash: string; index: number };
  proposalTypeScript: { codeHash: string; hashType: string; args: string };
  duration: number;
}

/**
 * Create a proposal cell on-chain.
 *
 * The proposal type script args are a 20-byte blake160 Type ID computed from
 * the first input's outpoint and the output index.
 */
export async function createProposal(
  signer: ccc.Signer,
  params: CreateProposalParams,
): Promise<CreateProposalResult> {
  const config = mergeConfig(DEVNET_CONFIG, params.config);
  const client = signer.client;

  const amount = params.amount ?? 0n;
  const minimalRequirement = BigInt(params.minimalRequirement ?? 0);

  // Resolve receiver lock script
  let receiverLock: ccc.Script;
  if (params.receiver) {
    const addr = await ccc.Address.fromString(params.receiver, client);
    receiverLock = addr.script;
  } else {
    receiverLock = await getSignerLock(signer);
  }

  const alwaysSuccessLock = scriptFromInfo(config.alwaysSuccess);
  const proposalTypeMeta = config.proposalTypeScript;

  // Placeholder args (20 bytes zeros) for capacity estimation
  const placeholderArgs = ("0x" + "00".repeat(20)) as ccc.Hex;
  const placeholderTypeScript = ccc.Script.from({
    codeHash: proposalTypeMeta.codeHash,
    hashType: proposalTypeMeta.hashType,
    args: placeholderArgs,
  });

  // Build proposal cell data
  const proposalData = buildProposalData(
    params,
    config,
    receiverLock,
    amount,
    minimalRequirement,
  );
  const proposalDataHex = ccc.hexFrom(proposalData);

  // Anti-spam floor enforced by the embedded proposal script (1000 CKB).
  const MIN_PROPOSAL_CAPACITY = 100_000_000_000n;
  const proposalOutput = ccc.CellOutput.from(
    {
      capacity: MIN_PROPOSAL_CAPACITY,
      lock: alwaysSuccessLock,
      type: placeholderTypeScript,
    },
    proposalDataHex,
  );

  const tx = ccc.Transaction.from({
    outputs: [proposalOutput],
    outputsData: [proposalDataHex],
  });

  // Always-success lock contract is the only required cell dep.
  // The proposal type script is embedded in the node — no code cell dep needed.
  tx.addCellDeps(cellDepFromInfo(config.alwaysSuccess));

  // Collect inputs to cover capacity
  await tx.completeInputsByCapacity(signer);

  // Now we know the first input — compute the real type ID args (20 bytes)
  if (tx.inputs.length === 0) {
    throw new Error("No inputs collected");
  }
  const realArgs = buildProposalTypeScriptArgs(
    tx.inputs[0],
    0, // proposal cell is at output index 0
  );

  tx.outputs[0].type = ccc.Script.from({
    codeHash: proposalTypeMeta.codeHash,
    hashType: proposalTypeMeta.hashType,
    args: realArgs,
  });

  await tx.completeFeeBy(signer, config.feeRate);
  const txHash = await signer.sendTransaction(tx);

  const finalTypeScript = tx.outputs[0].type!;
  return {
    txHash,
    proposalOutPoint: { txHash, index: 0 },
    proposalTypeScript: {
      codeHash: finalTypeScript.codeHash,
      hashType: finalTypeScript.hashType,
      args: finalTypeScript.args,
    },
    duration: params.duration,
  };
}

function buildProposalData(
  params: CreateProposalParams,
  config: NetworkConfig,
  receiverLock: ccc.Script,
  amount: bigint,
  minimalRequirement: bigint,
): Uint8Array {
  const descBytes = new TextEncoder().encode(params.description);

  return ProposalCodec.encode({
    duration: params.duration,
    voteCellCodeHash: config.voteTypeScript.codeHash,
    voteCellHashType: hashTypeToByte(config.voteTypeScript.hashType),
    description: ccc.hexFrom(descBytes),
    receiver: receiverLock,
    amount,
    minimalRequirement,
  });
}

// ─── Consume Proposal ────────────────────────────────────────────────────────

export interface ConsumeProposalParams {
  proposalOutPoint: { txHash: string; index: number };
  startBlockHash: string;
  endBlockHash: string;
  config?: Partial<NetworkConfig>;
}

export interface ConsumeProposalResult {
  txHash: string;
  /** Output index of the payment cell in the consume transaction */
  outputIndex: number;
}

/**
 * Consume a proposal cell.
 *
 * The node performs the vote tally natively using the embedded proposal type
 * script. No witness is required — only two header_deps to define the
 * voting block range.
 *
 * To define the range:
 *   header_deps[0] = start block (creating block of the proposal)
 *   header_deps[1] = end block   (start + duration, must exist on chain)
 */
export async function consumeProposal(
  signer: ccc.Signer,
  params: ConsumeProposalParams,
): Promise<ConsumeProposalResult> {
  const config = mergeConfig(DEVNET_CONFIG, params.config);
  const client = signer.client;

  // Fetch the proposal cell to read its data
  const proposalCell = await getRequiredCell(client, params.proposalOutPoint);
  if (!proposalCell.outputData || proposalCell.outputData === "0x") {
    throw new Error("Proposal cell has no data");
  }

  const proposalData = ProposalCodec.decode(proposalCell.outputData);
  const receiverLock = proposalData.receiver;

  // Build transaction
  const tx = ccc.Transaction.from({
    inputs: [
      {
        previousOutput: {
          txHash: params.proposalOutPoint.txHash,
          index: params.proposalOutPoint.index,
        },
      },
    ],
    headerDeps: [params.startBlockHash, params.endBlockHash],
    outputs: [
      {
        capacity: proposalData.amount,
        lock: receiverLock,
      },
    ],
    outputsData: ["0x"],
  });

  // Always-success lock contract is the only required cell dep.
  tx.addCellDeps(cellDepFromInfo(config.alwaysSuccess));

  // Collect additional inputs for fees (the proposal cell output goes to receiver,
  // so we might need the sender's cells for tx fee)
  await tx.completeFeeBy(signer, config.feeRate);

  const txHash = await signer.sendTransaction(tx);

  return { txHash, outputIndex: 0 };
}

/**
 * Build a NetworkConfig that overrides only the RPC URL, keeping all other
 * devnet defaults. Useful when talking to testnet/mainnet with the same scripts.
 */
export function configWithRpcUrl(
  rpcUrl: string,
  base: NetworkConfig = DEVNET_CONFIG,
): NetworkConfig {
  return { ...base, ckbRpcUrl: rpcUrl };
}

export { buildClient };
