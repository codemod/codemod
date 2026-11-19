// `semver` for `codemod/workflow`, shared by the workflow body and isolates.
// `call(op, ecosystem, a, b)` is the host's version logic (crates/deps).

export type Ecosystem = "npm" | "cargo" | "pypi";
export interface SemverOptions {
  ecosystem?: Ecosystem;
}
export type SemverCall = (op: string, ecosystem: Ecosystem, a: string, b: string) => unknown;

export function makeSemver(call: SemverCall) {
  const eco = (options?: SemverOptions): Ecosystem => options?.ecosystem ?? "npm";
  const text = (value: unknown, what: string): string => {
    if (typeof value !== "string") throw new TypeError(`semver: ${what} must be a string`);
    return value;
  };
  const compare = (a: unknown, b: unknown, options?: SemverOptions) =>
    call("compare", eco(options), text(a, "a"), text(b, "b")) as number;
  return Object.freeze({
    /** true, false, or null when the range cannot be read (`workspace:`, `git:` ...). */
    satisfies: (version: unknown, range: unknown, options?: SemverOptions) =>
      call("satisfies", eco(options), text(version, "version"), text(range, "range")) as
        | boolean
        | null,
    compare,
    gt: (a: unknown, b: unknown, options?: SemverOptions) => compare(a, b, options) > 0,
    lt: (a: unknown, b: unknown, options?: SemverOptions) => compare(a, b, options) < 0,
    /** Whether `from` to `to` changes the left-most non-zero component (0.29 to 0.30 is breaking). */
    isBreaking: (from: unknown, to: unknown, options?: SemverOptions) =>
      call("breaking", eco(options), text(from, "from"), text(to, "to")) as boolean,
    isPrerelease: (version: unknown, options?: SemverOptions) =>
      call("prerelease", eco(options), text(version, "version"), "") as boolean,
  });
}

export type Semver = ReturnType<typeof makeSemver>;
