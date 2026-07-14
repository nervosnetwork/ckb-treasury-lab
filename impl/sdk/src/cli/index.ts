#!/usr/bin/env tsx

import { Command } from "commander";
import { registerCreateProposal } from "./commands/create-proposal.js";
import { registerConsumeProposal } from "./commands/consume-proposal.js";
import { registerVote } from "./commands/vote.js";
import { registerCheck } from "./commands/check.js";
import { registerQuery } from "./commands/query.js";

const program = new Command();

program
  .name("ckb-vote")
  .description("CLI for the CKB Vote SDK — create proposals, vote, and settle")
  .version("0.1.0");

registerCreateProposal(program);
registerConsumeProposal(program);
registerVote(program);
registerCheck(program);
registerQuery(program);

program.parseAsync(process.argv).catch((err: unknown) => {
  console.error(err instanceof Error ? err.message : String(err));
  process.exit(1);
});
