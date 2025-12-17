import { ConsoleStdout, OpenFile, WASI, File as WasiFile } from "@bjorn3/browser_wasi_shim";

import factory from "./factory.js";
import type { InvokeInputs, ModuleSpec, Sandbox } from "./types.js";
import type { UUID } from "./types.js";

export { WebSandbox };

class WebSandbox implements Sandbox {
  private readonly sandboxInstance: Promise<ReturnType<typeof factory>>;

  constructor(public readonly runtimeUrl: URL) {
    this.sandboxInstance = this.initializeSandbox();
  }

  async getWasmExports() {
    const result = await this.sandboxInstance;

    return {
      scanFind: result.scanFind,
      scanFix: result.scanFix,
      dumpASTNodes: result.dumpASTNodes,
      dumpPattern: result.dumpPattern,
    };
  }

  private async initializeSandbox() {
    const wasiRuntime = new WASI(
      [],
      [],
      [
        new OpenFile(new WasiFile([])), // stdin
        ConsoleStdout.lineBuffered((message) => console.log(`[WASI stdout] ${message}`)),
        ConsoleStdout.lineBuffered((message) => console.warn(`[WASI stderr] ${message}`)),
      ],
    );

    const sandboxFactory = factory();
    const { instance } = await WebAssembly.instantiateStreaming(fetch(this.runtimeUrl), {
      "./codemod-sandbox_bg.js": sandboxFactory,
      wasi_snapshot_preview1: wasiRuntime.wasiImport,
    });

    sandboxFactory.__wbg_set_wasm(instance.exports);

    if (typeof sandboxFactory.__wbindgen_init_externref_table === "function") {
      sandboxFactory.__wbindgen_init_externref_table();
    } else {
      console.warn("Heap system detected - ensure factory.js has proper heap setup");
    }

    // @ts-expect-error 2739
    wasiRuntime.start({ exports: instance.exports });
    return sandboxFactory;
  }

  async initializeTreeSitter() {
    const sandbox = await this.sandboxInstance;
    return sandbox.Parser.init();
  }

  async setupParser(lang: string, path: string) {
    const sandbox = await this.sandboxInstance;
    return sandbox.setupParser(lang, path);
  }

  async runModule(
    sessionId: UUID,
    operation: "default" | "describe",
    moduleRegistry: ModuleSpec,
    moduleName: string,
    moduleInputs: InvokeInputs,
  ) {
    const sandboxInstance = await this.sandboxInstance;
    const moduleCode = moduleRegistry[moduleName];

    if (!moduleCode) {
      return { $error: `Unable to find module "${moduleName}"` };
    }

    const inputData = JSON.stringify(moduleInputs);
    console.debug(...formatDataSize(`Run module: "${moduleName}": input size`, inputData.length));

    const executionResult = await sandboxInstance.run_module(
      sessionId,
      operation,
      moduleName,
      moduleRegistry,
      moduleCode,
      inputData,
    );
    return executionResult;
  }
}

function formatDataSize(prefix: string, sizeInBytes: number): string[] {
  if (!Number.isInteger(sizeInBytes)) {
    console.warn(
      "formatDataSize: Input number should be an integer. Proceeding with the given value.",
    );
  }

  const kilobyte = 1024;
  const sizeUnits = ["Bytes", "KiB", "MiB", "GiB", "TiB"];
  const colorStyles = [
    "color:darkgray",
    "color:blue",
    "color:red;font-weight:bold",
    "color:red;font-weight:bold",
    "color:red;font-weight:bold",
  ];

  if (sizeInBytes === 0) {
    return ["%c0", "color: lightgray", "Bytes"];
  }

  let currentSize = sizeInBytes;
  let unitIndex = 0;
  while (currentSize >= kilobyte && unitIndex < sizeUnits.length - 1) {
    currentSize /= kilobyte;
    unitIndex++;
  }

  const formattedSize = Number.parseFloat(currentSize.toFixed(2));
  let unitLabel = sizeUnits[unitIndex];
  const consoleStyle = colorStyles[unitIndex];

  if (currentSize === 1) {
    unitLabel = "Byte";
  }

  return [`${prefix} %c${formattedSize}`, consoleStyle, unitLabel];
}
