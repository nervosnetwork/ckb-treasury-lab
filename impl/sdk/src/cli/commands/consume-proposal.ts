import { Command } from "commander";
import { consumeProposal } from "../../proposal.js";
import { buildSigner, die, resolveNetworkConfig } from "../shared.js";

export function registerConsumeProposal(program: Command): void {
  program
    .command("consume-proposal")
    .description(
      "Settle a passed proposal. The node tallies votes natively — no proof required.",
    )
    .requiredOption(
      "--private-key-file <path>",
      "Path to file containing hex private key",
    )
    .requiredOption(
      "--proposal-tx-hash <hex>",
      "Transaction hash of the proposal cell",
    )
    .option(
      "--proposal-index <n>",
      "Output index of the proposal cell",
      (v) => parseInt(v, 10),
      0,
    )
    .requiredOption(
      "--start-block-hash <hex>",
      "Hash of the block containing the proposal cell (header_deps[0])",
    )
    .requiredOption(
      "--end-block-hash <hex>",
      "Hash of the end block (start + duration, header_deps[1])",
    )
    .option("--rpc-url <url>", "CKB RPC endpoint")
    .option("--config <path>", "JSON NetworkConfig file (defaults to devnet)")
    .action(async (opts) => {
      try {
        const config = resolveNetworkConfig(opts.config, opts.rpcUrl);
        const signer = buildSigner(opts.privateKeyFile, config);

        const result = await consumeProposal(signer, {
          proposalOutPoint: {
            txHash: opts.proposalTxHash,
            index: opts.proposalIndex ?? 0,
          },
          startBlockHash: opts.startBlockHash,
          endBlockHash: opts.endBlockHash,
          config,
        });

        console.log("Proposal consumed successfully.");
        console.log(`  tx hash:      ${result.txHash}`);
        console.log(`  output index: ${result.outputIndex}`);
      } catch (err) {
        die(err);
      }
    });
}
