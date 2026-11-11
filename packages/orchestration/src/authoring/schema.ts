/**
 * Minimal Standard Schema (https://standardschema.dev) surface. Any library
 * implementing the spec (zod, valibot, arktype, ...) can be used for runnable
 * `input` and `output` typing without adding a dependency here.
 */
export interface StandardSchemaV1<Input = unknown, Output = Input> {
  readonly "~standard": {
    readonly version: 1;
    readonly vendor: string;
    readonly validate: (value: unknown) => StandardResult<Output> | Promise<StandardResult<Output>>;
    readonly types?: { readonly input: Input; readonly output: Output } | undefined;
  };
}

export type StandardResult<Output> =
  | { readonly value: Output; readonly issues?: undefined }
  | { readonly issues: ReadonlyArray<{ readonly message: string }> };

export type InferOutput<S> = S extends StandardSchemaV1<unknown, infer O> ? O : never;

export class SchemaError extends Error {
  constructor(
    readonly where: string,
    readonly issues: ReadonlyArray<{ message: string }>,
  ) {
    super(`${where}: ${issues.map((issue) => issue.message).join("; ")}`);
    this.name = "SchemaError";
  }
}

export async function validate<O>(
  schema: StandardSchemaV1<unknown, O> | undefined,
  value: unknown,
  where: string,
): Promise<O> {
  if (!schema) return value as O;
  const result = await schema["~standard"].validate(value);
  if (result.issues) throw new SchemaError(where, result.issues);
  return result.value;
}

/**
 * Tiny Standard Schema from a predicate. Enough for tests and prototypes;
 * real workflows can pass any Standard Schema library instead.
 */
export function guard<T>(
  name: string,
  check: (value: unknown) => value is T,
): StandardSchemaV1<unknown, T> {
  return {
    "~standard": {
      version: 1,
      vendor: "codemod-orchestration",
      validate: (value) =>
        check(value) ? { value } : { issues: [{ message: `expected ${name}` }] },
    },
  };
}
