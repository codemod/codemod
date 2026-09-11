/**
 * One persistent JSSG worker process (`butterflow-execution-bridge
 * --jssg-worker`) speaking the JSONL protocol in `worker-protocol.ts`.
 *
 * The process's stdin/stdout pipes are the protocol channel and nothing
 * else ever writes to them. Requests are serialized: each waits for the
 * previous response. Aborting the signal kills the process; the host closing
 * stdin (including by dying) makes the worker exit on EOF, so no worker is
 * left behind.
 */
import { spawn, type ChildProcess } from "node:child_process";
import {
  isWorkerRequest,
  parseWorkerResponse,
  type WorkerRequest,
  type WorkerResponse,
} from "./worker-protocol.ts";

export interface WorkerSpawnOptions {
  bin: string;
  cwd?: string;
  env?: Record<string, string>;
  signal?: AbortSignal;
}

/** The worker exited (or could not start) before answering a request. */
export class WorkerExitError extends Error {
  constructor(
    readonly code: number | null,
    readonly signal: NodeJS.Signals | null,
    readonly lastError: string | undefined,
    message = `worker exited with ${signal ? `signal ${signal}` : `code ${code}`}${lastError ? `: ${lastError}` : ""}`,
  ) {
    super(message);
    this.name = "WorkerExitError";
  }
}

export class JssgWorker {
  readonly #child: ChildProcess;
  readonly #signal: AbortSignal | undefined;
  readonly #exited: Promise<{ code: number | null; signal: NodeJS.Signals | null }>;
  #buffer = "";
  #pending: { resolve: (r: WorkerResponse) => void; reject: (e: Error) => void } | undefined;
  #queue: Promise<unknown> = Promise.resolve();
  #exit: { code: number | null; signal: NodeJS.Signals | null } | undefined;
  #spawnError: Error | undefined;
  #lastError: string | undefined;

  static spawn(options: WorkerSpawnOptions): JssgWorker {
    return new JssgWorker(options);
  }

  private constructor(options: WorkerSpawnOptions) {
    this.#signal = options.signal;
    this.#child = spawn(options.bin, ["--jssg-worker"], {
      cwd: options.cwd,
      env: { ...process.env, ...options.env },
      stdio: ["pipe", "pipe", "ignore"],
    });
    this.#child.stdin?.on("error", () => {
      // EPIPE after the worker died; the close handler reports the exit.
    });
    this.#child.stdout?.setEncoding("utf8");
    this.#child.stdout?.on("data", (chunk: string) => this.#onData(chunk));
    this.#exited = new Promise((resolve) => {
      this.#child.once("error", (error) => {
        this.#spawnError = error;
        this.#settle({ code: null, signal: null });
        resolve({ code: null, signal: null });
      });
      this.#child.once("close", (code, signal) => {
        this.#settle({ code, signal });
        resolve({ code, signal });
      });
    });
    options.signal?.addEventListener("abort", this.#onAbort, { once: true });
    void this.#exited.then(() => options.signal?.removeEventListener("abort", this.#onAbort));
  }

  get pid(): number | undefined {
    return this.#child.pid;
  }

  get exited(): Promise<{ code: number | null; signal: NodeJS.Signals | null }> {
    return this.#exited;
  }

  /** Send one message and wait for its response. Rejects on exit or abort. */
  request(message: WorkerRequest): Promise<WorkerResponse> {
    if (!isWorkerRequest(message)) {
      return Promise.reject(new Error(`invalid worker request: ${JSON.stringify(message)}`));
    }
    const next = this.#queue.then(() => this.#send(message));
    this.#queue = next.catch(() => undefined);
    return next;
  }

  /** Ask the worker to close and wait for it to exit; kill it if it lingers. */
  async close(): Promise<void> {
    if (this.#exit) return;
    try {
      await Promise.race([this.request({ type: "close" }), this.#exited]);
    } catch {
      // Already exited or refused; the exit below still runs.
    }
    const timer = setTimeout(() => this.kill(), 2_000);
    await this.#exited;
    clearTimeout(timer);
  }

  kill(): void {
    if (this.#exit) return;
    this.#child.kill("SIGKILL");
  }

  readonly #onAbort = () => this.kill();

  #send(message: WorkerRequest): Promise<WorkerResponse> {
    if (this.#spawnError) return Promise.reject(this.#exitError());
    if (this.#exit) return Promise.reject(this.#exitError());
    if (this.#signal?.aborted) {
      this.kill();
      return Promise.reject(new WorkerExitError(null, "SIGKILL", undefined, "worker aborted"));
    }
    return new Promise<WorkerResponse>((resolve, reject) => {
      this.#pending = { resolve, reject };
      this.#child.stdin?.write(`${JSON.stringify(message)}\n`, (error) => {
        if (error && this.#pending) {
          this.#pending = undefined;
          reject(this.#exitError(error.message));
        }
      });
    });
  }

  #onData(chunk: string): void {
    this.#buffer += chunk;
    let newline = this.#buffer.indexOf("\n");
    while (newline !== -1) {
      const line = this.#buffer.slice(0, newline);
      this.#buffer = this.#buffer.slice(newline + 1);
      this.#deliver(line);
      newline = this.#buffer.indexOf("\n");
    }
  }

  #deliver(line: string): void {
    const pending = this.#pending;
    if (!pending) return; // an unsolicited line; the next exit/parse reports it
    this.#pending = undefined;
    try {
      const response = parseWorkerResponse(line);
      if (response.type === "error") this.#lastError = response.message;
      pending.resolve(response);
    } catch (error) {
      this.kill();
      pending.reject(error as Error);
    }
  }

  #settle(exit: { code: number | null; signal: NodeJS.Signals | null }): void {
    this.#exit = exit;
    const pending = this.#pending;
    this.#pending = undefined;
    pending?.reject(this.#exitError());
  }

  #exitError(detail?: string): WorkerExitError {
    if (this.#spawnError) {
      return new WorkerExitError(
        null,
        null,
        undefined,
        `failed to start worker: ${this.#spawnError.message}`,
      );
    }
    const exit = this.#exit ?? { code: null, signal: null };
    return new WorkerExitError(exit.code, exit.signal, detail ?? this.#lastError);
  }
}
