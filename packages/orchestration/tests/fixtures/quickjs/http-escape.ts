// Workflow code reaching for the host's network primitive directly: the
// host must refuse any URL outside the TypeSafe API root. Output: the error
// code, or "reached" if the request went out.
import { dynamic } from "../../../src/index.ts";

type Native = { httpRequest(request: Record<string, unknown>): Promise<unknown> };

export default dynamic(async (url: string) => {
  const native = (globalThis as Record<string, unknown>).__codemodQuickjsNative as Native;
  try {
    await native.httpRequest({ url, method: "GET", headers: {}, timeoutMs: 5000 });
    return "reached";
  } catch (error) {
    return (error as { code?: string }).code ?? String(error);
  }
});
