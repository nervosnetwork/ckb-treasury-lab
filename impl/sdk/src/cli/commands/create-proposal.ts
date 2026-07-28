import { Command } from "commander";
import { createProposal } from "../../proposal.js";
import { buildSigner, die, resolveNetworkConfig } from "../shared.js";

export function registerCreateProposal(program: Command): void {
  program
    .command("create-proposal")
    .description("Create a new proposal cell on-chain")
    .requiredOption(
      "--private-key-file <path>",
      "Path to file containing hex private key",
    )
    .requiredOption("--duration <blocks>", "Voting duration in blocks", (v) =>
      parseInt(v, 10),
    )
    .requiredOption(
      "--description <text>",
      "Plain-text description of the proposal",
    )
    .option(
      "--receiver <address>",
      "CKB address to receive funds if proposal passes (defaults to signer's address)",
    )
    .option(
      "--amount <ckb>",
      "Amount (in CKB) to transfer when proposal passes (defaults to 500)",
      parseFloat,
      500,
    )
    .option(
      "--minimal-requirement <ckb>",
      "Minimum total CKB vote weight required for proposal to pass (defaults to 10000)",
      parseFloat,
      10000,
    )
    .option("--start-block-hash <hash>", "Start block hash")
    .option("--end-block-hash <hash>", "End block hash")
    .option("--rpc-url <url>", "CKB RPC endpoint")
    .option("--config <path>", "JSON NetworkConfig file (defaults to devnet)")
    .action(async (opts) => {
      try {
        const config = resolveNetworkConfig(opts.config, opts.rpcUrl);
        const signer = buildSigner(opts.privateKeyFile, config);

        const amountShannon =
          opts.amount !== undefined
            ? BigInt(Math.round(opts.amount * 1e8))
            : 0n;
        const result = await createProposal(signer, {
          duration: opts.duration,
          description: opts.description,
          receiver: opts.receiver,
          amount: amountShannon,
          minimalRequirement: opts.minimalRequirement,
          startBlockHash: opts.startBlockHash,
          endBlockHash: opts.endBlockHash,
          config,
        });

        console.log("Proposal created successfully.");
        console.log(
          `  outpoint:     ${result.proposalOutPoint.txHash}:${result.proposalOutPoint.index}`,
        );
        console.log(`  duration:     ${result.duration}`);
        console.log(`  code_hash:    ${result.proposalTypeScript.codeHash}`);
        console.log(`  hash_type:    ${result.proposalTypeScript.hashType}`);
        console.log(`  args:         ${result.proposalTypeScript.args}`);
      } catch (err) {
        die(err);
      }
    });
}
