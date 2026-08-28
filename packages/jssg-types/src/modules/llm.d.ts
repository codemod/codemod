declare module "codemod:llm" {
  export interface LlmRequest {
    prompt: string;
    systemPrompt?: string;
    outputSchema?: Record<string, unknown> | boolean;
    maxTokens?: number;
  }

  export interface LlmResponse {
    output: string;
  }

  /**
   * Generate text through the engine-owned LLM client.
   *
   * Provider credentials and model selection come from the engine. The engine
   * records provider-reported usage automatically; codemods never submit usage
   * telemetry themselves. The codemod must declare the `fetch` capability.
   */
  export function generate(request: LlmRequest): Promise<LlmResponse>;

  export default generate;
}
