// `semver` for `codemod:workflow`, shared by the workflow body and isolates.
// `call(op, ecosystem, a, b)` is the host's version logic (crates/deps).

export function makeSemver(call) {
  const eco = (options) => (options && options.ecosystem) || "npm";
  const text = (value, what) => {
    if (typeof value !== "string") throw new TypeError(`semver: ${what} must be a string`);
    return value;
  };
  return Object.freeze({
    /** true, false, or null when the range cannot be read (`workspace:`, `git:` ...). */
    satisfies: (version, range, options) =>
      call("satisfies", eco(options), text(version, "version"), text(range, "range")),
    compare: (a, b, options) => call("compare", eco(options), text(a, "a"), text(b, "b")),
    gt: (a, b, options) => call("compare", eco(options), text(a, "a"), text(b, "b")) > 0,
    lt: (a, b, options) => call("compare", eco(options), text(a, "a"), text(b, "b")) < 0,
    /** Whether `from` to `to` changes the left-most non-zero component (0.29 to 0.30 is breaking). */
    isBreaking: (from, to, options) =>
      call("breaking", eco(options), text(from, "from"), text(to, "to")),
    isPrerelease: (version, options) =>
      call("prerelease", eco(options), text(version, "version"), ""),
  });
}
