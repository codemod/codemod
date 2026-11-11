/**
 * One `shell` operation executed in-process through the `Host`, with the
 * semantics of the workflow engine's `butterflow_runners::DirectRunner`:
 *
 * - the environment is the host's, plus the executor's `env`, plus the
 *   operation's `env` (later wins), and nothing else;
 * - a command starting with `#!/` is written to a temporary executable script
 *   and run directly; anything else runs as `sh -c <command>` (`cmd /C` on
 *   Windows) in `cwd`;
 * - stdout and stderr are one stream; the output is kept line by line, each
 *   line terminated by `\n` (a final newline is added, `\r\n` becomes `\n`);
 * - exit 0 is `succeeded` with `{ stdout }`; any other exit (a signal is
 *   exit code -1) is `failed` with `exitCode` and `output`; a start failure is
 *   `unknown`;
 * - aborting the signal kills the process tree: `cancelled` when the child
 *   died by a signal, `unknown` when it exited on its own meanwhile.

 */
import { getHost, type Host } from "../core/host.ts";
import type { Json } from "../core/json.ts";
import {
  PROTOCOL_VERSION,
  type OperationCompletion,
  type OperationRequest,
  type ShellOperation,
} from "../core/protocol.ts";

export interface ShellExecutionOptions {
  /** Working directory of the command. */
  cwd: string;
  /** Added to the host environment, under the operation's own `env`. */
  env?: Record<string, string>;
  /** Defaults to the installed host. */
  host?: Host;
  /** Streams combined output lines-in-progress as they arrive. */
  onOutput?: (chunk: string) => void;
}

/** DirectRunner's `BufRead::lines` + `push('\n')`: every line ends with exactly one `\n`. */
export function normalizeShellOutput(text: string): string {
  if (text === "") return "";
  const lines = text.split("\n");
  if (lines[lines.length - 1] === "") lines.pop();
  return lines.map((line) => `${line.endsWith("\r") ? line.slice(0, -1) : line}\n`).join("");
}

export async function executeShell(
  options: ShellExecutionOptions,
  request: OperationRequest,
  signal?: AbortSignal,
): Promise<OperationCompletion> {
  const host = options.host ?? getHost();
  const operation = request.operation as ShellOperation;
  const completion = (
    status: "failed" | "cancelled" | "unknown",
    message: string,
    extra: { exitCode?: number; output?: string; details?: { [key: string]: Json } } = {},
  ): OperationCompletion => ({
    protocolVersion: PROTOCOL_VERSION,
    commandId: request.commandId,
    status,
    error: { message, ...extra },
  });
  if (operation.kind !== "shell") {
    return completion("failed", `executeShell received a '${operation.kind}' operation`);
  }
  if (signal?.aborted) return completion("cancelled", "aborted before start");

  const env: Record<string, string> = {};
  for (const [name, value] of Object.entries(host.env())) {
    if (value !== undefined) env[name] = value;
  }
  Object.assign(env, options.env, operation.env);

  let script: string | undefined;
  let file: string;
  let args: string[];
  if (operation.command.startsWith("#!/")) {
    script = host.path.join(host.os.tmpdir(), `butterflow-script-${host.randomUUID()}.sh`);
    try {
      host.fs.writeText(script, operation.command, { mode: 0o755 });
    } catch (error) {
      return completion(
        "unknown",
        `Failed to write script to temporary file: ${(error as Error).message}`,
      );
    }
    file = script;
    args = [];
  } else if (host.os.platform === "win32") {
    file = "cmd";
    args = ["/C", operation.command];
  } else {
    file = "sh";
    args = ["-c", operation.command];
  }

  try {
    let result;
    try {
      result = await host.spawn({
        file,
        args,
        cwd: options.cwd,
        env,
        stderr: "merge",
        signal,
        ...(options.onOutput === undefined
          ? {}
          : {
              onOutput: (_stream: "stdout" | "stderr", chunk: string) => options.onOutput!(chunk),
            }),
      }).result;
    } catch (error) {
      return completion("unknown", `Failed to spawn command: ${(error as Error).message}`);
    }
    const output = normalizeShellOutput(result.stdout + result.stderr);
    if (signal?.aborted) {
      return result.signal !== null
        ? completion("cancelled", `shell killed by ${result.signal} on abort`)
        : completion("unknown", "aborted while the command was finishing");
    }
    if (result.code === 0) {
      return {
        protocolVersion: PROTOCOL_VERSION,
        commandId: request.commandId,
        status: "succeeded",
        output: { stdout: output },
      };
    }
    const exitCode = result.code ?? -1;
    return completion("failed", `Command failed with exit code ${exitCode}: ${output}`, {
      exitCode,
      output,
    });
  } finally {
    if (script !== undefined) {
      try {
        host.fs.rm(script, { force: true });
      } catch {
        // Best effort, as DirectRunner's `remove_file(..).ok()`.
      }
    }
  }
}
