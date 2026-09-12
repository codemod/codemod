/**
 * Plain JSON data. Everything that crosses a seam (history, protocol, workflow
 * output) must be representable as this type so it can move to Rust later.
 */
export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

/**
 * Canonical JSON: object keys sorted recursively, `undefined` values dropped.
 * Used for command identity and replay comparison. The Rust side compares
 * `serde_json::Value` equality, which has the same meaning.
 */
export function canonicalJson(value: unknown): string {
  return JSON.stringify(normalize(value));
}

function normalize(value: unknown): unknown {
  if (value === null || typeof value !== "object") return value;
  if (Array.isArray(value)) return value.map(normalize);
  const out: Record<string, unknown> = {};
  for (const key of Object.keys(value as object).sort()) {
    const child = (value as Record<string, unknown>)[key];
    if (child !== undefined) out[key] = normalize(child);
  }
  return out;
}

/** Deep clone through JSON so callers cannot mutate recorded data. */
export function cloneJson<T>(value: T): T {
  return value === undefined ? value : (JSON.parse(JSON.stringify(value)) as T);
}
