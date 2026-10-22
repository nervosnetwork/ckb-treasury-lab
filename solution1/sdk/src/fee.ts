import type { DeploymentConfig } from "./config.js";

export function feeRateOf(config: DeploymentConfig): bigint {
  return BigInt(config.feeRate ?? 1500);
}
