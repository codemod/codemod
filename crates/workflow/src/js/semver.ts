// `semver` for `codemod/deps`, shared by the workflow body and isolates.
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
    gte: (a: unknown, b: unknown, options?: SemverOptions) => compare(a, b, options) >= 0,
    lt: (a: unknown, b: unknown, options?: SemverOptions) => compare(a, b, options) < 0,
    lte: (a: unknown, b: unknown, options?: SemverOptions) => compare(a, b, options) <= 0,
    /** Whether `from` to `to` changes the left-most non-zero component (0.29 to 0.30 is breaking). */
    isBreaking: (from: unknown, to: unknown, options?: SemverOptions) =>
      call("breaking", eco(options), text(from, "from"), text(to, "to")) as boolean,
    isPrerelease: (version: unknown, options?: SemverOptions) =>
      call("prerelease", eco(options), text(version, "version"), "") as boolean,
    /** The lowest version a range accepts; null when it cannot be read. */
    minVersion: (range: unknown, options?: SemverOptions) =>
      call("minVersion", eco(options), text(range, "range"), "") as string | null,
    /** The major of a version, or of a range's lowest version; null when unreadable. */
    major: (versionOrRange: unknown, options?: SemverOptions) =>
      call("major", eco(options), text(versionOrRange, "version"), "") as number | null,
    /** The first version in a text, padded to three parts ("v1.2" is "1.2.0"); null when none. */
    coerce: (value: unknown) => call("coerce", "npm", text(value, "text"), "") as string | null,
    /** The highest of `versions` the range accepts; null when none does. */
    maxSatisfying: (versions: readonly unknown[], range: unknown, options?: SemverOptions) => {
      if (!Array.isArray(versions)) throw new TypeError("semver: versions must be an array");
      const list = versions.map((version, i) => text(version, `versions[${i}]`));
      return call("maxSatisfying", eco(options), JSON.stringify(list), text(range, "range")) as
        | string
        | null;
    },
    /** Whether a range can be read (not git, file, a url, or a tag such as "latest"). */
    validRange: (range: unknown, options?: SemverOptions) =>
      call("validRange", eco(options), text(range, "range"), "") as boolean,
  });
}

export type Semver = ReturnType<typeof makeSemver>;
