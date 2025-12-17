export type Values = Record<string, unknown>;
export type OutputValues = Values; // Alias for backward compatibility
export type ModuleSpec = Record<string, string>;
export type ModuleMethod = "default" | "describe";

export type UUID = `${string}-${string}-${string}-${string}-${string}`;

export type DescriberInputs = {
  inputs?: Values;
  inputSchema?: unknown;
  outputSchema?: unknown;
  asType?: boolean;
};

export type DescriberOutputs = {
  inputSchema: unknown;
  outputSchema: unknown;
};

export type InvokeInputs = Values;
export type InvokeOutputs = Values;

export type CapabilityFunction = (inputs: Values, path: number[]) => Promise<Values | undefined>;

export type CapabilityRegistry = {
  fetch?: CapabilityFunction;
};

// Legacy alias
export type Capability = CapabilityFunction;
export type CapabilitySpec = CapabilityRegistry;

export type Sandbox = {
  runModule(
    invocationId: UUID,
    method: "default" | "describe",
    modules: ModuleSpec,
    name: string,
    inputs: Record<string, unknown>,
  ): Promise<InvokeOutputs | DescriberOutputs>;
};
