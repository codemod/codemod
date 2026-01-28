import { randomBytes } from "node:crypto";
import * as fs from "node:fs";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { AxiosError } from "axios";
import inquirer from "inquirer";
import prettyjson from "prettyjson";

import { CODEMOD_NOT_FOUND } from "@codemod-com/api-types";
import { type Printer, chalk, colorLongString } from "@codemod-com/printer";
import { Runner, parseFlowSettings } from "@codemod-com/runner";
import type { TelemetrySender } from "@codemod-com/telemetry";
import {
  doubleQuotify,
  execPromise,
  getCodemodRc,
} from "@codemod-com/utilities";
import { version as cliVersion } from "#/../package.json";
import { getDiff, getDiffScreen } from "#dryrun-diff.js";
import { fetchCodemod, populateCodemodArgs } from "#fetch-codemod.js";
import type { GlobalArgvOptions, RunArgvOptions } from "#flags.js";
import { handleInstallDependencies } from "#install-dependencies.js";
import { AuthService } from "#services/auth-service.js";
import { RunnerService } from "#services/runner-service.js";
import type { TelemetryEvent } from "#telemetry.js";
import type { NamedFileCommand } from "#types/commands.js";
import { originalStdoutWrite } from "#utils/constants.js";
import { logsPath, writeLogs } from "#utils/logs.js";
import { open } from "../utils/open.js";

const checkFileTreeVersioning = async (target: string) => {
  let force = true;

  try {
    const status = await execPromise("git status --porcelain", {
      cwd: target,
    });

    if (status.stdout.trim()) {
      const res = await inquirer.prompt<{ force: boolean }>({
        type: "confirm",
        name: "force",
        message:
          "Current git state contains uncommitted changes. Proceed anyway?",
        default: false,
      });

      force = res.force;
    }
  } catch (err) {
    if (!(err instanceof Error)) {
      return;
    }

    if (
      "stderr" in err &&
      typeof err.stderr === "string" &&
      err.stderr.trim().startsWith("fatal: not a git repository")
    ) {
      const res = await inquirer.prompt<{ force: boolean }>({
        type: "confirm",
        name: "force",
        message:
          "Target folder is not tracked by git. Codemod changes might be irreversible. Proceed anyway?",
        default: false,
      });

      force = res.force;
    } else {
      const res = await inquirer.prompt<{ force: boolean }>({
        type: "confirm",
        name: "force",
        message:
          "Could not run git working tree check. Codemod changes might be irreversible. Proceed anyway?",
        default: false,
      });

      force = res.force;
    }
  }

  if (!force) {
    process.exit(0);
  }
};

export const handleRunCliCommand = async (options: {
  printer: Printer;
  args: GlobalArgvOptions & RunArgvOptions;
  telemetry: TelemetrySender<TelemetryEvent>;
  onExit: () => void;
}) => {
  const { printer, args, telemetry, onExit } = options;

  if (args.mode === "json") {
    process.stdout.write = () => false;
    process.stderr.write = () => false;
  }

  const flowSettings = await parseFlowSettings(args, printer);

  const nameOrPath = args._.at(0)?.toString() ?? args.source ?? null;
  if (nameOrPath === null) {
    if (args.logs) {
      return open(logsPath, printer);
    }

    throw new Error("Codemod to run was not specified!");
  }

  let codemod = await fetchCodemod({ nameOrPath, printer, argv: args }).catch(
    (error: Error) => {
      if (error instanceof AxiosError) {
        if (
          error.response?.status === 400 &&
          error.response.data.error === CODEMOD_NOT_FOUND
        ) {
          printer.printConsoleMessage(
            "error",
            chalk.red(
              "The specified command or codemod name could not be recognized.",
              "\nTo view available commands, execute",
              `${chalk.yellow.bold(doubleQuotify("codemod --help"))}.`,
              "\nTo see a list of existing codemods, run",
              `${chalk.yellow.bold(doubleQuotify("codemod search"))}`,
              "or",
              `${chalk.yellow.bold(doubleQuotify("codemod list"))}`,
              "with a query representing the codemod you are looking for.",
            ),
          );

          process.exit(1);
        }
      }

      throw new Error(`Error while fetching codemod ${nameOrPath}: ${error}`);
    },
  );

  if (
    (args.readme || args.config || args.version) &&
    codemod.type === "standalone"
  ) {
    return printer.printConsoleMessage(
      "error",
      chalk.red("Standalone codemods do not support this feature."),
    );
  }

  if (args.version) {
    return printer.printConsoleMessage(
      "log",
      chalk.cyan(`v${codemod.config.version}`),
    );
  }

  if (args.readme) {
    try {
      const readmeContents = await readFile(join(codemod.path, "README.md"), {
        encoding: "utf8",
      });

      return printer.printConsoleMessage(
        "log",
        colorLongString(readmeContents, chalk.cyan, 80),
      );
    } catch (err) {
      return printer.printConsoleMessage(
        "error",
        chalk.red("Could not find the manual file for the codemod."),
      );
    }
  }

  if (args.config) {
    try {
      const { config: codemodConfig } = await getCodemodRc({
        source: codemod.path,
        throwOnNotFound: false,
      });

      return printer.printConsoleMessage(
        "log",
        prettyjson.render(codemodConfig, { inlineArrays: true }),
      );
    } catch (err) {
      return printer.printConsoleMessage(
        "error",
        chalk.red("Could not find the configuration file for the codemod."),
      );
    }
  }

  codemod = await populateCodemodArgs({ codemod, argv: args, printer });
  const authService = new AuthService(printer);
  const runnerService = new RunnerService(printer);
  const runner = new Runner({
    flowSettings,
    authService,
    runnerService,
  });

  if (!flowSettings.dry && args.interactive) {
    await checkFileTreeVersioning(flowSettings.target);
  }

  const depsToInstall: Record<
    string,
    { deps: string[]; affectedFiles: string[] }
  > = {};

  const executionId = randomBytes(20).toString("base64url");
  const allExecutedCommands: NamedFileCommand[] = [];

  const argEntries = Object.entries(codemod.safeArgumentRecord);
  if (argEntries.length > 0) {
    printer.printConsoleMessage(
      "info",
      chalk.cyan(
        "Running with arguments:\n",
        ...argEntries.map(([key, value]) =>
          chalk.bold(`\n- ${key}: ${value} (${typeof value})`),
        ),
      ),
    );
  }

  const executionErrors = await runner.run({
    codemod,
    onSuccess: async ({ codemod, output, commands }) => {
      const modifiedFilePaths = [
        ...new Set(
          commands.map((c) => ("oldPath" in c ? c.oldPath : c.newPath)),
        ),
      ];

      if (args.mode === "json" && typeof output === "object") {
        process.stdout.write = originalStdoutWrite;
        process.stdout.write(JSON.stringify(output, null, 2));
        process.stdout.write = () => false;
      } else if (args.mode === "plain") {
        process.stdout.write = originalStdoutWrite;
        process.stdout.write(String(output));
        process.stdout.write = () => false;
      }

      let codemodName: string;
      if (codemod.type === "standalone") {
        codemodName = "from-source-file";
      } else if (codemod.type === "package" && codemod.source === "local") {
        codemodName = "from-source-compatible-package";
      } else {
        codemodName = codemod.config.name;
      }

      if (codemod.type === "package") {
        const { config: codemodConfig } = await getCodemodRc({
          source: codemod.path,
          throwOnNotFound: false,
        });

        if (codemodConfig?.deps) {
          depsToInstall[codemodName] = {
            affectedFiles: modifiedFilePaths,
            deps: codemodConfig.deps,
          };
        }
      }

      allExecutedCommands.push(...commands.map((c) => ({ ...c, codemodName })));

      telemetry.sendDangerousEvent({
        kind: "codemodExecuted",
        codemodName,
        executionId,
        fileCount: modifiedFilePaths.length,
        cliVersion: cliVersion,
      });

      // For standalone codemods we create a temp folder with a codemod-compatible package
      if (codemod.type === "standalone") {
        await fs.promises.rm(codemod.path, { recursive: true });
      }
    },
    onFailure: (error) => {
      if (!(error instanceof Error)) {
        return;
      }

      printer.printOperationMessage({
        kind: "error",
        message: `Error while running the codemod:\n${error.message}`,
      });

      telemetry.sendEvent({
        kind: "failedToExecuteCommand",
        commandName: "codemod.executeCodemod",
        cliVersion: cliVersion,
      });
    },
  });

  let finishRun = () => onExit();

  if (args.logs && executionErrors?.length) {
    const notice = await writeLogs({
      prefix: chalk.red("Codemod execution encountered errors."),
      content: executionErrors
        .map(
          (e) =>
            `Error at ${e.filePath}${
              e.codemodName ? ` (${e.codemodName})` : ""
            }:\n${e.message}`,
        )
        .join("\n\n"),
    });

    finishRun = () => {
      printer.terminateExecutionProgress();
      printer.printConsoleMessage("info", notice);
      return onExit();
    };
  }

  if (allExecutedCommands.length === 0) {
    printer.terminateExecutionProgress();
    if (codemod.config.engine !== "workflow") {
      printer.printConsoleMessage(
        "info",
        chalk.yellow("\nNo changes were made during the codemod run."),
      );
    }
    return finishRun();
  }

  if (flowSettings.dry && allExecutedCommands.length > 0) {
    const screen = getDiffScreen(allExecutedCommands.map(getDiff));

    screen.key(["escape", "q", "C-c"], () => {
      screen?.destroy();
      return finishRun();
    });

    return screen.render();
  }

  if (flowSettings.install) {
    for (const [codemodName, { deps, affectedFiles }] of Object.entries(
      depsToInstall,
    )) {
      await handleInstallDependencies({
        codemodName,
        printer,
        affectedFiles,
        target: flowSettings.target,
        deps,
      });
    }
  }

  return finishRun();
};
