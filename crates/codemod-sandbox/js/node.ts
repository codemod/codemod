import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { ConsoleStdout, OpenFile, WASI, File as WasiFile } from "@bjorn3/browser_wasi_shim";
import factory from "./factory.js";
import type { ModuleSpec, Sandbox } from "./types.js";
import type { UUID } from "./types.js";

export { NodeSandbox };

async function loadWasmRuntime(): Promise<Buffer> {
  const wasmFile = import.meta.resolve("@codemod.com/codemod-sandbox/sandbox.wasm");
  return readFile(fileURLToPath(wasmFile));
}

export interface LogEntry {
  timestamp: number;
  level: "info" | "warn" | "error";
  message: string;
}

class NodeSandbox implements Sandbox {
  private readonly runtimeBuffer: Promise<Buffer>;
  private sandbox: ReturnType<typeof factory> | null = null;
  private logs: LogEntry[] = [];

  constructor(buffer?: Buffer) {
    this.runtimeBuffer = buffer ? Promise.resolve(buffer) : loadWasmRuntime();
  }

  async initializeTreeSitter(locateFile: (path: string) => string) {
    const instance = await this.getWasmInstance();
    await instance.Parser.init({ locateFile });
  }

  async setupParser(lang: string, path: string) {
    const instance = await this.getWasmInstance();
    return instance.setupParser(lang, path);
  }

  getLogs() {
    return this.logs;
  }

  clearLogs() {
    this.logs = [];
  }

  private async getWasmInstance() {
    if (this.sandbox) {
      return this.sandbox;
    }

    const wasmBytes = await this.runtimeBuffer;

    const wasiInstance = new WASI(
      [],
      [],
      [
        new OpenFile(new WasiFile([])), // stdin
        ConsoleStdout.lineBuffered((message) =>
          this.logs.push({
            timestamp: Date.now(),
            level: "info",
            message,
          }),
        ),
        ConsoleStdout.lineBuffered((message) =>
          this.logs.push({
            timestamp: Date.now(),
            level: "warn",
            message,
          }),
        ),
      ],
    );

    const sandboxFactory = factory();
    const { instance } = await WebAssembly.instantiate(wasmBytes, {
      "./codemod-sandbox_bg.js": sandboxFactory,
      wasi_snapshot_preview1: wasiInstance.wasiImport,
    });

    sandboxFactory.__wbg_set_wasm(instance.exports);
    // @ts-expect-error 2739
    wasiInstance.start({ exports: instance.exports });

    if (typeof sandboxFactory.__wbindgen_init_externref_table === "function") {
      sandboxFactory.__wbindgen_init_externref_table();
    } else {
      console.warn("Heap system detected - ensure factory.js has proper heap setup");
    }

    this.sandbox = sandboxFactory;

    return this.sandbox;
  }

  async getWasmExports() {
    const instance = await this.getWasmInstance();

    return {
      scanFind: instance.scanFind,
      scanFix: instance.scanFix,
      dumpASTNodes: instance.dumpASTNodes,
      dumpPattern: instance.dumpPattern,
    };
  }

  async runModule(
    sessionId: UUID,
    operation: "default" | "describe",
    moduleRegistry: ModuleSpec,
    moduleName: string,
    moduleInputs: Record<string, unknown>,
  ) {
    const moduleCode = moduleRegistry[moduleName];

    if (!moduleCode) {
      return { $error: `Unable to find module "${moduleName}"` };
    }

    this.clearLogs();

    const instance = await this.getWasmInstance();

    const executionResult = await instance.run_module(
      sessionId,
      operation,
      moduleName,
      moduleRegistry,
      moduleCode,
      JSON.stringify(moduleInputs),
    );

    return executionResult;
  }
}
