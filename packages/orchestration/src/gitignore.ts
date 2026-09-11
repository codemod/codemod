/**
 * Gitignore-style pattern matching with the exact semantics of the Rust
 * `ignore` crate (v0.4) that the workflow engine's walker uses: line parsing
 * from `GitignoreBuilder::add_line`, glob-to-regex translation from `globset`
 * with `literal_separator(true)` and `backslash_escape(true)`, last-match-wins
 * evaluation, and `OverrideBuilder` include/exclude semantics. The shared
 * contract in `fixtures/walker/cases.json` pins this against the engine.
 */

export type MatchKind = "none" | "ignore" | "whitelist";

export interface CompiledGlob {
  readonly original: string;
  readonly regex: RegExp;
  readonly whitelist: boolean;
  readonly onlyDir: boolean;
}

type Token =
  | { type: "literal"; value: string }
  | { type: "any" }
  | { type: "star" }
  | { type: "recursivePrefix" }
  | { type: "recursiveSuffix" }
  | { type: "recursiveZeroOrMore" }
  | { type: "class"; negated: boolean; ranges: [string, string][] }
  | { type: "altOpen" }
  | { type: "altSep" }
  | { type: "altClose" };

export class GlobError extends Error {
  constructor(
    readonly glob: string,
    message: string,
  ) {
    super(`invalid glob '${glob}': ${message}`);
    this.name = "GlobError";
  }
}

/** `globset` parser with `literal_separator` and `backslash_escape` enabled. */
function tokenize(glob: string): Token[] {
  const tokens: Token[] = [];
  let i = 0;
  let inAlternate = false;
  // Index of the first token of the current top-level segment or alternate
  // branch; `**` behaves like a prefix when no token precedes it there.
  let segmentStart = 0;
  const next = () => glob[i];
  const bump = () => glob[i++];
  const isSeparator = (c: string | undefined) => c === "/";

  while (i < glob.length) {
    const c = bump()!;
    switch (c) {
      case "?":
        tokens.push({ type: "any" });
        break;
      case "*": {
        const before = glob[i - 2];
        if (next() !== "*") {
          tokens.push({ type: "star" });
          break;
        }
        bump();
        if (tokens.length === segmentStart) {
          if (next() !== undefined && !isSeparator(next())) {
            tokens.push({ type: "star" }, { type: "star" });
          } else {
            tokens.push({ type: "recursivePrefix" });
            if (next() !== undefined) bump();
          }
          break;
        }
        if (!isSeparator(before)) {
          tokens.push({ type: "star" }, { type: "star" });
          break;
        }
        let suffix: boolean;
        if (next() === undefined) {
          suffix = true;
        } else if ((next() === "," || next() === "}") && inAlternate) {
          suffix = true;
        } else if (isSeparator(next())) {
          bump();
          suffix = false;
        } else {
          tokens.push({ type: "star" }, { type: "star" });
          break;
        }
        // The preceding `/` literal is folded into the recursive token.
        const last = tokens[tokens.length - 1];
        if (last?.type === "literal" && last.value === "/") tokens.pop();
        tokens.push({ type: suffix ? "recursiveSuffix" : "recursiveZeroOrMore" });
        break;
      }
      case "[": {
        let negated = false;
        if (next() === "!") {
          negated = true;
          bump();
        }
        const ranges: [string, string][] = [];
        let first = true;
        let closed = false;
        while (i < glob.length) {
          let ch = bump()!;
          if (ch === "]" && !first) {
            closed = true;
            break;
          }
          if (ch === "\\" && i < glob.length) ch = bump()!;
          if (ch === "-" && !first && next() !== undefined && next() !== "]") {
            const last = ranges.pop();
            let end = bump()!;
            if (end === "\\" && i < glob.length) end = bump()!;
            if (!last || last[0] !== last[1] || last[0] > end) {
              throw new GlobError(glob, "invalid character range");
            }
            ranges.push([last[0], end]);
          } else {
            ranges.push([ch, ch]);
          }
          first = false;
        }
        if (!closed) throw new GlobError(glob, "unclosed character class");
        tokens.push({ type: "class", negated, ranges });
        break;
      }
      case "{":
        if (inAlternate) throw new GlobError(glob, "nested alternate groups are not supported");
        inAlternate = true;
        tokens.push({ type: "altOpen" });
        segmentStart = tokens.length;
        break;
      case "}":
        if (!inAlternate) {
          tokens.push({ type: "literal", value: "}" });
          break;
        }
        inAlternate = false;
        tokens.push({ type: "altClose" });
        segmentStart = 0;
        break;
      case ",":
        if (!inAlternate) {
          tokens.push({ type: "literal", value: "," });
          break;
        }
        tokens.push({ type: "altSep" });
        segmentStart = tokens.length;
        break;
      case "\\": {
        const escaped = i < glob.length ? bump()! : "\\";
        tokens.push({ type: "literal", value: escaped });
        break;
      }
      default:
        tokens.push({ type: "literal", value: c });
    }
  }
  if (inAlternate) throw new GlobError(glob, "unclosed alternate group");
  return tokens;
}

const escapeRegex = (value: string) => value.replace(/[.*+?^${}()|[\]\\/-]/g, "\\$&");

function render(tokens: Token[]): string {
  if (tokens.length === 1 && tokens[0]!.type === "recursivePrefix") return "^.*$";
  let out = "^";
  for (const token of tokens) {
    switch (token.type) {
      case "literal":
        out += escapeRegex(token.value);
        break;
      case "any":
        out += "[^/]";
        break;
      case "star":
        out += "[^/]*";
        break;
      case "recursivePrefix":
        out += "(?:/?|.*/)";
        break;
      case "recursiveSuffix":
        out += "/.*";
        break;
      case "recursiveZeroOrMore":
        out += "(?:/|/.*/)";
        break;
      case "class":
        out += `[${token.negated ? "^" : ""}${token.ranges
          .map(([start, end]) =>
            start === end ? escapeRegex(start) : `${escapeRegex(start)}-${escapeRegex(end)}`,
          )
          .join("")}]`;
        break;
      case "altOpen":
        out += "(?:";
        break;
      case "altSep":
        out += "|";
        break;
      case "altClose":
        out += ")";
        break;
    }
  }
  return `${out}$`;
}

/** Compile one glob with `globset` semantics (no gitignore line handling). */
export function globToRegex(glob: string): RegExp {
  return new RegExp(render(tokenize(glob)), "s");
}

/**
 * One line of a gitignore file (or one override pattern), or `null` for a
 * blank line or comment. Throws `GlobError` for an unparseable glob.
 */
export function parseGitignoreLine(raw: string): CompiledGlob | null {
  let line = raw;
  if (line.startsWith("#")) return null;
  if (!line.endsWith("\\ ")) line = line.replace(/\s+$/u, "");
  if (line === "") return null;
  let whitelist = false;
  let absolute = false;
  if (line.startsWith("\\!") || line.startsWith("\\#")) {
    line = line.slice(1);
  } else {
    if (line.startsWith("!")) {
      whitelist = true;
      line = line.slice(1);
    }
    if (line.startsWith("/")) {
      absolute = true;
      line = line.slice(1);
    }
  }
  let onlyDir = false;
  if (line.endsWith("/")) {
    onlyDir = true;
    line = line.slice(0, -1);
  }
  let actual = line;
  if (!absolute && !line.includes("/") && !(actual.startsWith("**/") || actual === "**")) {
    actual = `**/${actual}`;
  }
  if (actual.endsWith("/**")) actual = `${actual}/*`;
  return { original: raw, regex: globToRegex(actual), whitelist, onlyDir };
}

/** All globs of a gitignore-style file; unparseable lines are skipped, as the walker does. */
export function parseGitignore(text: string): CompiledGlob[] {
  const globs: CompiledGlob[] = [];
  for (const line of text.split(/\r?\n/u)) {
    try {
      const glob = parseGitignoreLine(line);
      if (glob) globs.push(glob);
    } catch (error) {
      if (!(error instanceof GlobError)) throw error;
    }
  }
  return globs;
}

/** Last matching glob wins; directory-only globs never match files. */
export function matchGlobs(
  globs: readonly CompiledGlob[],
  candidate: string,
  isDir: boolean,
): MatchKind {
  for (let index = globs.length - 1; index >= 0; index--) {
    const glob = globs[index]!;
    if (glob.onlyDir && !isDir) continue;
    if (glob.regex.test(candidate)) return glob.whitelist ? "whitelist" : "ignore";
  }
  return "none";
}

/** A gitignore-style file whose patterns are relative to its directory. */
export class GitignoreMatcher {
  constructor(readonly globs: readonly CompiledGlob[]) {}

  get empty(): boolean {
    return this.globs.length === 0;
  }

  /** `candidate` is `/`-separated and relative to the file's directory (a leading `./` is stripped). */
  match(candidate: string, isDir: boolean): MatchKind {
    if (this.globs.length === 0) return "none";
    return matchGlobs(
      this.globs,
      candidate.startsWith("./") ? candidate.slice(2) : candidate,
      isDir,
    );
  }
}

/**
 * `OverrideBuilder` semantics: include patterns are whitelists, exclude
 * patterns (`!pattern`) are ignores, the last matching pattern wins, and a
 * file (never a directory) that matches nothing is ignored when at least one
 * include pattern exists. Invalid patterns throw `GlobError`.
 */
export class OverrideMatcher {
  readonly globs: readonly CompiledGlob[];
  readonly whitelists: number;

  constructor(patterns: { include?: readonly string[]; exclude?: readonly string[] }) {
    const globs: CompiledGlob[] = [];
    for (const pattern of patterns.include ?? []) {
      const glob = parseGitignoreLine(pattern);
      if (!glob) throw new GlobError(pattern, "empty pattern");
      // An override line is inverted: `pattern` selects, `!pattern` rejects.
      globs.push({ ...glob, whitelist: !glob.whitelist });
    }
    for (const pattern of patterns.exclude ?? []) {
      const glob = parseGitignoreLine(pattern.startsWith("!") ? pattern : `!${pattern}`);
      if (!glob) throw new GlobError(pattern, "empty pattern");
      globs.push({ ...glob, whitelist: !glob.whitelist });
    }
    this.globs = globs;
    this.whitelists = globs.filter((glob) => glob.whitelist).length;
  }

  get empty(): boolean {
    return this.globs.length === 0;
  }

  match(candidate: string, isDir: boolean): MatchKind {
    if (this.globs.length === 0) return "none";
    const matched = matchGlobs(this.globs, candidate, isDir);
    if (matched === "none" && this.whitelists > 0 && !isDir) return "ignore";
    return matched;
  }
}
