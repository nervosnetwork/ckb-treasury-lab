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
  /** 32-byte hex: hash of the SP1 guest program verifying key */
  sp1VerifyingKeyHash: string;
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
      "0xfb9026d2c0acff05496c4ccbeabe968440373fc71921680b22d25ca3f2ea634f",
    hashType: "type",
    outPoint: {
      txHash:
        "0x2cccea9eb9b564a3a2e2d74145c71b9cc9e11fbeea494be41e3f5f4001a16286",
      index: 0,
    },
  },

  proposalTypeScript: {
    codeHash:
      "0xa5bf702b6f096b2e800a619edbc920a165759ffb806c6955a15865d1cccf7958",
    hashType: "type",
    outPoint: {
      txHash:
        "0x5e30bf87ee22929b0b1747f2201a40731d4528a5af2a1f3f7c90909dc68ddec2",
      index: 0,
    },
  },

  voteTypeScript: {
    codeHash:
      "0xf659306b518b6c3cc2cc7079bf8d349910821eb459534f42f17dfe392d38b2d4",
    hashType: "type",
    outPoint: {
      txHash:
        "0x3d4d939bef8d45f19d38d7de163ddbf1c6ae50cbbe2d7aeace9e397fccabe691",
      index: 0,
    },
  },

  // From sp1/ckb-vote-verification/verifying-key.txt
  sp1VerifyingKeyHash:
    "0x00469e90740aaaa13a626262b02ffe3b1434c19af72b0f8eb0c3ceec2b3228fe",

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
