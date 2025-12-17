import type { OutputValues, UUID } from "./types.js";
import type { CapabilityFunction, CapabilityRegistry } from "./types.js";

export { fetch, Capabilities };

interface CapabilityContext {
  registry: Map<string, CapabilityFunction>;
}

class Capabilities {
  private readonly contextMap = new Map<UUID, CapabilityContext>();
  private static globalInstance: Capabilities = new Capabilities();

  async invoke(sessionId: UUID, capabilityName: string, inputData: string) {
    const context = this.contextMap.get(sessionId);
    const handler = context?.registry.get(capabilityName);

    if (!context || !handler) {
      throw new Error(
        `Capability "${capabilityName}" is not available for session "${sessionId}".`,
      );
    }

    const parsedInputs = JSON.parse(inputData);
    const isOutputOperation = capabilityName === "output";
    const { $metadata, ...cleanInputs } = parsedInputs;

    const actualInputs = !isOutputOperation && $metadata ? cleanInputs : parsedInputs;

    let result: OutputValues;
    try {
      result = (await handler(actualInputs, [])) || {};
    } catch (error) {
      result = {
        $error: `Unable to invoke capability: ${(error as Error).message}`,
      };
    }

    return JSON.stringify(result);
  }

  install(sessionId: UUID, capabilities: CapabilityRegistry) {
    if (this.contextMap.has(sessionId)) {
      throw new Error(`Session ID collision: "${sessionId}" capabilities were already installed.`);
    }

    this.contextMap.set(sessionId, {
      registry: new Map(Object.entries(capabilities)),
    });
  }

  uninstall(sessionId: UUID) {
    this.contextMap.delete(sessionId);
  }

  static instance() {
    return Capabilities.globalInstance;
  }
}

async function fetch(sessionId: UUID, inputData: string) {
  return Capabilities.instance().invoke(sessionId, "fetch", inputData);
}
