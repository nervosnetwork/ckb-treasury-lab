export interface ScriptInfo {
  /** code_hash used when referencing this script in type/lock fields */
  codeHash: string;
  hashType: "type" | "data" | "data1";
  /** outPoint of the cell containing the script code */
  outPoint: {
    txHash: string;
    index: number;
  };
}

/** Known system script override for a CCC client (matches CCC's ScriptInfo shape). */
export interface KnownScriptInfo {
  codeHash: string;
  hashType: string;
  cellDeps: Array<{
    cellDep: {
      outPoint: { txHash: string; index: number };
      depType: string;
    };
  }>;
}

export interface NetworkConfig {
  ckbRpcUrl: string;
  alwaysSuccess: ScriptInfo;
  proposalTypeScript: ScriptInfo;
  voteTypeScript: ScriptInfo;
  /** Fee rate in shannons per KB (default: 1500) */
  feeRate: bigint;
  /**
   * Override CCC's built-in known-script outpoints.
   * Required when connecting to a node (e.g. devnet) whose genesis differs
   * from the public testnet/mainnet.
   */
  knownScripts?: Record<string, KnownScriptInfo>;
}

export const DEVNET_CONFIG: NetworkConfig = {
  ckbRpcUrl: "http://127.0.0.1:8114",

  alwaysSuccess: {
    codeHash:
      "0xe06f2a38d20d1e4ce7c536dd410818a5e352b308fdf5c9bc31124887beb9b0d7",
    hashType: "type",
    outPoint: {
      txHash:
        "0xdfafc64e52bc365505e10fbfe4bf1bca9bddec4decf10a753b13ba8698b939d0",
      index: 0,
    },
  },

  proposalTypeScript: {
    codeHash:
      "0x00000000000000000000000000000000000000000000000050524f504f53414c",
    hashType: "type",
    outPoint: {
      txHash:
        "0xdfafc64e52bc365505e10fbfe4bf1bca9bddec4decf10a753b13ba8698b939d0",
      index: 0,
    },
  },

  voteTypeScript: {
    codeHash:
      "0x590102bc8a88ad923b8b8d643f12507d84eec15934a7eeb0a9a01beade3b573b",
    hashType: "type",
    outPoint: {
      txHash:
        "0xa75456e7008777e32835df57fce22c1a6d467bd5d0c980ab9b4fc04af5a5dcc0",
      index: 0,
    },
  },

  feeRate: 1500n,

  // Devnet genesis system-script outpoints (differ from testnet/mainnet).
  // genesis tx[0] = 0x7822543fea81950fbf161ff27f825e18ba50733fb269e3b8811638c800c4250e
  // genesis tx[1] = 0x2bea94097abba269ff88d83726044d8eef10a2aa427bd3b5fcb348a81fd0ce4a
  knownScripts: {
    Secp256k1Blake160: {
      codeHash:
        "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8",
      hashType: "type",
      cellDeps: [
        {
          cellDep: {
            outPoint: {
              txHash:
                "0x2bea94097abba269ff88d83726044d8eef10a2aa427bd3b5fcb348a81fd0ce4a",
              index: 0,
            },
            depType: "depGroup",
          },
        },
      ],
    },
    Secp256k1Multisig: {
      codeHash:
        "0x5c5069eb0857efc65e1bca0c07df34c31663b3622fd3876c876320fc9634e2a8",
      hashType: "type",
      cellDeps: [
        {
          cellDep: {
            outPoint: {
              txHash:
                "0x2bea94097abba269ff88d83726044d8eef10a2aa427bd3b5fcb348a81fd0ce4a",
              index: 1,
            },
            depType: "depGroup",
          },
        },
      ],
    },
    NervosDao: {
      codeHash:
        "0x82d76d1b75fe2fd9a27dfbaa65a039221a380d76c926f378d3f81cf3e7e13f2e",
      hashType: "type",
      cellDeps: [
        {
          cellDep: {
            outPoint: {
              txHash:
                "0x7822543fea81950fbf161ff27f825e18ba50733fb269e3b8811638c800c4250e",
              index: 2,
            },
            depType: "code",
          },
        },
      ],
    },
  },
};
