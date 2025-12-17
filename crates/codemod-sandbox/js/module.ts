import { Capabilities } from "./capabilities.js";
import type {
  CapabilityRegistry,
  DescriberInputs,
  DescriberOutputs,
  InvokeInputs,
  InvokeOutputs,
  ModuleSpec,
  Sandbox,
} from "./types.js";

export { SandboxedModule };

type UUID = string;

interface ExecutionSession {
  sessionId: UUID;
  startTime: number;
  label: string;
}

class SandboxedModule {
  private readonly executionTracker = new Map<UUID, ExecutionSession>();

  constructor(
    public readonly sandbox: Sandbox,
    public readonly capabilities: CapabilityRegistry,
    public readonly modules: ModuleSpec,
  ) {}

  private async executeModule(
    operation: "describe" | "default",
    moduleName: string,
    moduleInputs: DescriberInputs | InvokeInputs,
  ) {
    const sessionId = crypto.randomUUID();
    const operationLabel = operation === "describe" ? "Describe" : "Invoke";
    const executionLabel = `${operationLabel} module "${moduleName}": uuid="${sessionId}"`;

    const session: ExecutionSession = {
      sessionId,
      startTime: globalThis.performance.now(),
      label: executionLabel,
    };

    this.executionTracker.set(sessionId, session);
    Capabilities.instance().install(sessionId, this.capabilities);

    const moduleOutputs = await this.sandbox.runModule(
      sessionId,
      operation,
      this.modules,
      moduleName,
      moduleInputs,
    );

    Capabilities.instance().uninstall(sessionId);

    this.logExecutionTime(sessionId);
    return moduleOutputs;
  }

  private logExecutionTime(sessionId: UUID) {
    const session = this.executionTracker.get(sessionId);
    if (session) {
      const duration = globalThis.performance.now() - session.startTime;
      console.debug?.(`${session.label}: ${duration.toFixed(0)} ms`);
      this.executionTracker.delete(sessionId);
    } else {
      console.warn(`Unable to find timing for "${sessionId}"`);
    }
  }

  async invoke(name: string, inputs: InvokeInputs): Promise<InvokeOutputs> {
    return this.executeModule("default", name, inputs);
  }

  describe(name: string, inputs: DescriberInputs): Promise<DescriberOutputs> {
    return this.executeModule("describe", name, inputs) as Promise<DescriberOutputs>;
  }
}
