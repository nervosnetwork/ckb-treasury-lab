/**
 * Molecule codecs of the four on-chain cell payloads.
 *
 * They mirror the generated builders of `crates/ckb-vote-types`:
 * every type is a molecule `table`, so each field - fixed size or not - gets a
 * 4 byte offset in the header.
 */

import { ccc } from "@ckb-ccc/shell";

/** A fixed size byte codec, used where the schema says `ByteN`. */
function fixedBytesCodec(byteLength: number): ccc.Codec<ccc.HexLike, ccc.Hex> {
  return ccc.mol.Codec.from<ccc.HexLike, ccc.Hex>({
    byteLength,
    encode(value) {
      const bytes = ccc.bytesFrom(value);
      if (bytes.byteLength !== byteLength) {
        throw new Error(
          `expected ${byteLength} bytes, got ${bytes.byteLength}`,
        );
      }
      return bytes;
    },
    decode(value) {
      return ccc.hexFrom(value);
    },
  });
}

/** 20 bytes, the ckb-blake160-hash of a lock script. */
export const Bytes20 = fixedBytesCodec(20);

/** `table VotingConfig` of the config cell. */
export const VotingConfigCodec = ccc.mol.table({
  emergentHalt: ccc.mol.Uint8,
  voteCodeHash: ccc.mol.Byte32,
  voteHashType: ccc.mol.Uint8,
  countingCodeHash: ccc.mol.Byte32,
  countingHashType: ccc.mol.Uint8,
  alwaysSuccessCodeHash: ccc.mol.Byte32,
  alwaysSuccessHashType: ccc.mol.Uint8,
  yesThreshold: ccc.mol.Uint64,
  minimalProposalCapacity: ccc.mol.Uint64,
  voteDuration: ccc.mol.Uint64,
  voteWindow: ccc.mol.Uint64,
  challengeTime: ccc.mol.Uint64,
  vetoLockScriptHash: ccc.mol.Byte32,
  minimalVoteAmount: ccc.mol.Uint64,
});

/** Decoded `VotingConfig` with `bigint` counters. */
export interface VotingConfig {
  emergentHalt: number;
  voteCodeHash: ccc.Hex;
  voteHashType: number;
  countingCodeHash: ccc.Hex;
  countingHashType: number;
  alwaysSuccessCodeHash: ccc.Hex;
  alwaysSuccessHashType: number;
  yesThreshold: bigint;
  minimalProposalCapacity: bigint;
  voteDuration: bigint;
  voteWindow: bigint;
  challengeTime: bigint;
  vetoLockScriptHash: ccc.Hex;
  minimalVoteAmount: bigint;
}

/** `table ProposalCellData` of the proposal / finalized / passed cell. */
export const ProposalCellDataCodec = ccc.mol.table({
  status: ccc.mol.Uint8,
  description: ccc.mol.Bytes,
  requestedAmount: ccc.mol.Uint64,
  recipientLockHash: Bytes20,
  totalYes: ccc.mol.Uint64,
  originBlockNumber: ccc.mol.Uint64,
});

/** Decoded `ProposalCellData`; `description` is kept as UTF-8 text. */
export interface ProposalCellData {
  status: number;
  description: string;
  requestedAmount: bigint;
  recipientLockHash: ccc.Hex;
  totalYes: bigint;
  originBlockNumber: bigint;
}

/** `table Vote` of a vote cell. */
export const VoteCodec = ccc.mol.table({
  voteAmount: ccc.mol.Uint64,
  direction: ccc.mol.Uint8,
});

/** Decoded `Vote`. */
export interface VoteData {
  voteAmount: bigint;
  direction: number;
}

/**
 * `table Counting` of a counting cell.
 *
 * `start_hash` and `end_hash` are big endian `u16`, which is exactly what
 * `mol.Uint16BE` encodes.
 */
export const CountingCodec = ccc.mol.table({
  startHash: ccc.mol.Uint16BE,
  endHash: ccc.mol.Uint16BE,
  direction: ccc.mol.Uint8,
  voteAmount: ccc.mol.Uint64,
});

/** Decoded `Counting`. */
export interface CountingData {
  startHash: number;
  endHash: number;
  direction: number;
  voteAmount: bigint;
}

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();

/** Encodes a `VotingConfig` to the cell data hex. */
export function encodeVotingConfig(config: VotingConfig): ccc.Hex {
  return ccc.hexFrom(VotingConfigCodec.encode(config));
}

/** Decodes a `VotingConfig` from cell data. */
export function decodeVotingConfig(data: ccc.HexLike): VotingConfig {
  return VotingConfigCodec.decode(data) as VotingConfig;
}

/** Encodes a `ProposalCellData` to the cell data hex. */
export function encodeProposalCellData(data: ProposalCellData): ccc.Hex {
  return ccc.hexFrom(
    ProposalCellDataCodec.encode({
      ...data,
      description: ccc.hexFrom(textEncoder.encode(data.description)),
    }),
  );
}

/** Decodes a `ProposalCellData` from cell data. */
export function decodeProposalCellData(data: ccc.HexLike): ProposalCellData {
  const decoded = ProposalCellDataCodec.decode(data) as Omit<
    ProposalCellData,
    "description"
  > & { description: ccc.Hex };
  return {
    ...decoded,
    description: textDecoder.decode(ccc.bytesFrom(decoded.description)),
  };
}

/** Encodes a `Vote` to the cell data hex. */
export function encodeVote(data: VoteData): ccc.Hex {
  return ccc.hexFrom(VoteCodec.encode(data));
}

/** Decodes a `Vote` from cell data. */
export function decodeVote(data: ccc.HexLike): VoteData {
  return VoteCodec.decode(data) as VoteData;
}

/** Encodes a `Counting` to the cell data hex. */
export function encodeCounting(data: CountingData): ccc.Hex {
  return ccc.hexFrom(CountingCodec.encode(data));
}

/** Decodes a `Counting` from cell data. */
export function decodeCounting(data: ccc.HexLike): CountingData {
  return CountingCodec.decode(data) as CountingData;
}
