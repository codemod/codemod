declare module "codemod:llm" {
  /** @experimental */
  export interface LlmRequest {
    prompt: string;
    systemPrompt?: string;
    outputSchema?: Record<string, unknown> | boolean;
    maxTokens?: number;
  }

  /** @experimental */
  export interface LlmResponse {
    output: string;
  }

  /**
   * Generate text through the engine-owned LLM client.
   *
   * Provider credentials and model selection come from the engine. The engine
   * records provider-reported usage automatically; codemods never submit usage
   * telemetry themselves. The codemod must declare the `fetch` capability.
   * @experimental
   */
  export function generate(request: LlmRequest): Promise<LlmResponse>;

  /** @experimental */
  export default generate;
}
