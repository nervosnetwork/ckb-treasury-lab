#!/usr/bin/env node
/**
 * Thin wrapper that runs the TypeScript CLI.
 * - Under Bun: imports and runs the .ts source directly.
 * - Under Node: delegates to tsx for on-the-fly TypeScript execution.
 */
if (typeof globalThis.Bun !== "undefined") {
  import("../src/cli/index.ts").catch((err) => {
    console.error(err instanceof Error ? err.message : String(err));
    process.exit(1);
  });
} else {
  import { spawnSync } from "child_process";
  import { fileURLToPath } from "url";
  import { dirname, join } from "path";

  const __dirname = dirname(fileURLToPath(import.meta.url));
  const tsxBin = join(__dirname, "../node_modules/.bin/tsx");
  const cliEntry = join(__dirname, "../src/cli/index.ts");

  const result = spawnSync(tsxBin, [cliEntry, ...process.argv.slice(2)], {
    stdio: "inherit",
    env: process.env,
  });

  process.exit(result.status ?? 1);
}
