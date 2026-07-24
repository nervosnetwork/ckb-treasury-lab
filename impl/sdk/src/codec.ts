/**
 * Molecule codecs for CKB Vote types.
 * Mirrors crates/types/molecules/types.mol
 */

import { ccc } from "@ckb-ccc/shell";

const mol = ccc.mol;

/**
 * Codec that wraps ccc.Script for use as a nested field inside mol.table().
 * Matches the `Script` table type from blockchain.mol.
 */
export const ScriptMolCodec: ccc.mol.Codec<ccc.ScriptLike, ccc.Script> =
  mol.Codec.from({
    encode: (s: ccc.ScriptLike): ccc.Bytes => ccc.Script.encode(s),
    decode: (bytes: ccc.BytesLike): ccc.Script => ccc.Script.fromBytes(bytes),
  });

/**
 * table Vote {
 *   vote: byte,          // 0=NO, 1=YES
 *   amount: Uint64,
 *   dao_index: Uint16Vec,
 * }
 */
export const VoteCodec = mol.table({
  vote: mol.Uint8,
  amount: mol.Uint64,
  daoIndex: mol.Uint16Vec,
});

export type VoteEncodable = ccc.mol.EncodableType<typeof VoteCodec>;
export type VoteDecoded = ccc.mol.DecodedType<typeof VoteCodec>;

/**
 * table Proposal {
 *   duration: Uint32,
 *   vote_cell_code_hash: Byte32,
 *   vote_cell_hash_type: byte,
 *   description: Bytes,
 *   receiver: Script,
 *   amount: Uint64,
 *   minimal_requirement: Uint64,
 *   start_block_hash: Byte32,
 *   end_block_hash: Byte32,
 * }
 */
export const ProposalCodec = mol.table({
  duration: mol.Uint32,
  voteCellCodeHash: mol.Byte32,
  voteCellHashType: mol.Uint8,
  description: mol.Bytes,
  receiver: ScriptMolCodec,
  amount: mol.Uint64,
  minimalRequirement: mol.Uint64,
  startBlockHash: mol.Byte32,
  endBlockHash: mol.Byte32,
});

export type ProposalEncodable = ccc.mol.EncodableType<typeof ProposalCodec>;
export type ProposalDecoded = ccc.mol.DecodedType<typeof ProposalCodec>;
