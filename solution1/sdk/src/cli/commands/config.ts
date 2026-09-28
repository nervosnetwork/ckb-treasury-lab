/**
 * `create-config`, `show-config` and `check-deployment`.
 */

import { Command } from "commander";
import {
  createConfigCell,
  checkDeployment,
  updateConfigCell,
} from "../../config-cell.js";
import { loadConfigCell } from "../../query.js";
import { patchConfigFile } from "../../config.js";
import {
  act,
  addSignerOptions,
  addCommonOptions,
  clientOf,
  configOf,
  output,
  shannons,
  signerOf,
  formatOutPoint,
} from "../shared.js";

export function registerConfigCommands(program: Command): void {
  addSignerOptions(program.command("create-config"))
    .description("mint the singleton config cell")
    .option("--lock <address>", "lock of the config cell")
    .option("--emergent-halt", "set emergent_halt and stop every script")
    .option("--yes-threshold <ckb>", '"YES" votes required to finalize', "0")
    .option("--minimal-proposal-capacity <ckb>", "minimum proposal bond", "0")
    .option("--vote-duration <blocks>", "blocks before finalizing", "0")
    .option("--vote-window <blocks>", "blocks a vote stays valid", "0")
    .option("--challenge-time <blocks>", "blocks before a proposal passes", "0")
    .option("--veto-lock <address>", "lock allowed to veto a proposal")
    .option("--minimal-vote-amount <ckb>", "minimum vote amount", "0")
    .option("--capacity <ckb>", "capacity of the config cell")
    .option("--write <path>", "patch the config Type ID into this config file")
    .action(
      act(async (options) => {
        const signer = signerOf(options);
        const config = configOf(options);
        const result = await createConfigCell(signer, config, {
          lock: options.lock,
          emergentHalt: !!options.emergentHalt,
          yesThreshold: shannons(options.yesThreshold),
          minimalProposalCapacity: shannons(options.minimalProposalCapacity),
          voteDuration: options.voteDuration,
          voteWindow: options.voteWindow,
          challengeTime: options.challengeTime,
          vetoLock: options.vetoLock,
          minimalVoteAmount: shannons(options.minimalVoteAmount),
          capacity: options.capacity ? shannons(options.capacity) : undefined,
        });
        if (options.write) {
          patchConfigFile(options.write, {
            configArgs: result.configArgs,
            configCell: result.configCell,
          });
        }
        output(
          options,
          result,
          [
            "Config cell created.",
            `  tx hash:     ${result.txHash}`,
            `  config cell: ${formatOutPoint(result.configCell)}`,
            `  config args: ${result.configArgs}`,
            `  config id:   ${result.configId}`,
            "",
            options.write
              ? `scripts.config.args and configCell written to ${options.write}`
              : "Add these to the deployment config before creating a proposal:",
            ...(options.write
              ? []
              : [
                  `  scripts.config.args = ${result.configArgs}`,
                  `  configCell = ${formatOutPoint(result.configCell)}`,
                ]),
          ].join("\n"),
        );
      }),
    );

  addSignerOptions(program.command("update-config"))
    .description("update the config cell in place, keeping its Type ID")
    .option("--halt <bool>", "set emergent_halt to true or false")
    .option("--yes-threshold <ckb>", '"YES" votes required to finalize')
    .option("--minimal-proposal-capacity <ckb>", "minimum proposal bond")
    .option("--vote-duration <blocks>", "blocks before finalizing")
    .option("--vote-window <blocks>", "blocks a vote stays valid")
    .option("--challenge-time <blocks>", "blocks before a proposal passes")
    .option("--veto-lock <address>", "lock allowed to veto a proposal")
    .option("--minimal-vote-amount <ckb>", "minimum vote amount")
    .option("--capacity <ckb>", "capacity of the config cell")
    .option("--write <path>", "patch the new config cell into this config file")
    .action(
      act(async (options) => {
        const signer = signerOf(options);
        const config = configOf(options);
        const result = await updateConfigCell(signer, config, {
          emergentHalt:
            options.halt === undefined ? undefined : options.halt === "true",
          yesThreshold:
            options.yesThreshold === undefined
              ? undefined
              : shannons(options.yesThreshold),
          minimalProposalCapacity:
            options.minimalProposalCapacity === undefined
              ? undefined
              : shannons(options.minimalProposalCapacity),
          voteDuration: options.voteDuration,
          voteWindow: options.voteWindow,
          challengeTime: options.challengeTime,
          vetoLock: options.vetoLock,
          minimalVoteAmount:
            options.minimalVoteAmount === undefined
              ? undefined
              : shannons(options.minimalVoteAmount),
          capacity: options.capacity ? shannons(options.capacity) : undefined,
        });
        if (options.write) {
          patchConfigFile(options.write, { configCell: result.configCell });
        }
        output(
          options,
          result,
          [
            `Config cell updated.`,
            `  tx hash:     ${result.txHash}`,
            `  config cell: ${formatOutPoint(result.configCell)}`,
            ...(options.write ? [`  written to ${options.write}`] : []),
          ].join("\n"),
        );
      }),
    );

  addCommonOptions(program.command("show-config"))
    .description("show the on-chain config cell")
    .action(
      act(async (options) => {
        const client = clientOf(options);
        const config = configOf(options);
        const info = await loadConfigCell(client, config);
        output(
          options,
          {
            cell: formatOutPoint(info.cell.outPoint),
            configArgs: info.typeScript.args,
            configId: info.id,
            ...info.data,
          },
          [
            `config cell: ${formatOutPoint(info.cell.outPoint)}`,
            `config args: ${info.typeScript.args}`,
            `emergent_halt: ${info.data.emergentHalt}`,
            `vote_code_hash: ${info.data.voteCodeHash}`,
            `counting_code_hash: ${info.data.countingCodeHash}`,
            `always_success_code_hash: ${info.data.alwaysSuccessCodeHash}`,
            `yes_threshold: ${info.data.yesThreshold}`,
            `minimal_proposal_capacity: ${info.data.minimalProposalCapacity}`,
            `vote_duration: ${info.data.voteDuration}`,
            `vote_window: ${info.data.voteWindow}`,
            `challenge_time: ${info.data.challengeTime}`,
            `veto_lock_script_hash: ${info.data.vetoLockScriptHash}`,
            `minimal_vote_amount: ${info.data.minimalVoteAmount}`,
          ].join("\n"),
        );
      }),
    );

  addCommonOptions(program.command("check-deployment"))
    .description("verify that every deployed script matches the config")
    .action(
      act(async (options) => {
        const client = clientOf(options);
        const config = configOf(options);
        const checks = await checkDeployment(client, config);
        output(
          options,
          checks,
          checks
            .map(
              (check) =>
                `${check.ok ? "ok  " : "FAIL"} ${check.script}: ${check.message}`,
            )
            .join("\n"),
        );
        if (checks.some((check) => !check.ok)) {
          process.exitCode = 2;
        }
      }),
    );
}
