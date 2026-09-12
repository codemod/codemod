/**
 * File selection for one JSSG invocation, with the workflow engine's walker
 * semantics (`codemod_walk_builder` plus `OverrideBuilder` in the Rust
 * engine): hidden files visited, symlinks skipped and never followed,
 * `.ignore` / `.gitignore` / `.git/info/exclude` and the global git excludes
 * honored without requiring a git repository, ignore files in ancestor
 * directories applied, and include/exclude globs that take precedence over
 * every ignore file. Deterministic component-wise order.
 *
 * Matching is the `ignore` npm package: gitignore semantics, which is also
 * what the engine's override globs use, plus brace alternation (globset has
 * it, gitignore does not) expanded up front. `fixtures/walker/cases.json`
 * pins parity with the engine; `tests/files.test.ts` and
 * `crates/execution-bridge/tests/contracts.rs` run the two sides.
 */
import { readFileSync, readdirSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, relative, sep } from "node:path";
import ignoreModule, { type Ignore } from "ignore";

// `ignore` is CommonJS; its default export is the factory under both Node's
// interop and its own type declarations only through `.default`.
const ignore = ignoreModule.default;

const LANGUAGES = JSON.parse(
  readFileSync(new URL("./languages.json", import.meta.url), "utf8"),
) as Record<string, string[]>;

/**
 * The engine's file extensions for a language (`languages.json`, pinned to
 * `create_language_extension_map` by a cargo test). Empty for languages the
 * table does not list, which then need an explicit `include`.
 */
export function languageExtensions(language: string): string[] {
  return LANGUAGES[language.toLowerCase()] ?? [];
}

type Match = "ignore" | "whitelist" | "none";

interface Override {
  /** Absolute directory the patterns are relative to. */
  root: string;
  matcher: Ignore;
  /** With at least one include pattern, an unmatched file is ignored. */
  whitelists: boolean;
}

interface Level {
  dir: string;
  /** `.ignore`, `.gitignore`, `.git/info/exclude`, in precedence order. */
  kinds: (Ignore | undefined)[];
}

/** `a/{b,c}` -> `a/b`, `a/c`. */
function expandBraces(pattern: string): string[] {
  const match = /\{([^{}]*)\}/.exec(pattern);
  if (!match) return [pattern];
  const [whole, body] = match;
  return body!
    .split(",")
    .flatMap((alternative) =>
      expandBraces(
        pattern.slice(0, match.index) + alternative + pattern.slice(match.index + whole.length),
      ),
    );
}

/**
 * `OverrideBuilder` semantics: an include pattern whitelists, an exclude
 * pattern (or an include written as `!pattern`) ignores, the last matching
 * pattern wins.
 */
function override(
  root: string,
  include: readonly string[] | undefined,
  exclude: readonly string[] | undefined,
): Override {
  const lines: string[] = [];
  let whitelists = false;
  for (const glob of (include ?? []).flatMap(expandBraces)) {
    if (glob.startsWith("!")) lines.push(glob.slice(1));
    else {
      lines.push(`!${glob}`);
      whitelists = true;
    }
  }
  for (const glob of (exclude ?? []).flatMap(expandBraces)) lines.push(glob.replace(/^!/u, ""));
  return { root, matcher: ignore().add(lines), whitelists };
}

function test(matcher: Ignore, candidate: string, isDir: boolean): Match {
  const { ignored, unignored } = matcher.test(isDir ? `${candidate}/` : candidate);
  return ignored ? "ignore" : unignored ? "whitelist" : "none";
}

function readIgnoreFile(path: string): Ignore | undefined {
  try {
    return ignore().add(readFileSync(path, "utf8"));
  } catch {
    return undefined;
  }
}

function readLevel(dir: string): Level {
  return {
    dir,
    kinds: [".ignore", ".gitignore", join(".git", "info", "exclude")].map((name) =>
      readIgnoreFile(join(dir, name)),
    ),
  };
}

function toPosix(path: string): string {
  return sep === "/" ? path : path.split(sep).join("/");
}

/** Component-wise byte order, the order `Vec<PathBuf>::sort` produces. */
export function comparePaths(a: string, b: string): number {
  const left = a.split("/");
  const right = b.split("/");
  for (let index = 0; index < Math.min(left.length, right.length); index++) {
    if (left[index] !== right[index]) return left[index]! < right[index]! ? -1 : 1;
  }
  return left.length - right.length;
}

/** `gitconfig_excludes_path` from the `ignore` crate. */
export function discoverGlobalExcludesPath(
  env: NodeJS.ProcessEnv = process.env,
): string | undefined {
  const home = env.HOME ?? env.USERPROFILE ?? homedir();
  const xdg = env.XDG_CONFIG_HOME ? env.XDG_CONFIG_HOME : home ? join(home, ".config") : undefined;
  const fromConfig = (path: string | undefined): string | undefined => {
    if (!path) return undefined;
    let text: string;
    try {
      text = readFileSync(path, "utf8");
    } catch {
      return undefined;
    }
    const value = /^\s*excludesfile\s*=\s*"?(.+?)"?\s*$/imu.exec(text)?.[1];
    return value?.startsWith("~") && home ? join(home, value.slice(1)) : value;
  };
  return (
    fromConfig(home ? join(home, ".gitconfig") : undefined) ??
    fromConfig(xdg ? join(xdg, "git", "config") : undefined) ??
    (xdg ? join(xdg, "git", "ignore") : undefined)
  );
}

function walk(root: string, overrides: Override[], globalExcludes: string | null | undefined) {
  const globalPath = globalExcludes === undefined ? discoverGlobalExcludesPath() : globalExcludes;
  const global = globalPath ? readIgnoreFile(globalPath) : undefined;
  const ancestors: Level[] = [];
  for (let dir = dirname(root); ; dir = dirname(dir)) {
    ancestors.push(readLevel(dir));
    if (dirname(dir) === dir) break;
  }
  const candidate = (base: string, absolute: string) => toPosix(relative(base, absolute));

  // `Ignore::matched`: an override match, whitelist or ignore, is final.
  // Otherwise the nearest `.ignore` beats the nearest `.gitignore` beats the
  // nearest `.git/info/exclude` beats the global excludes.
  const matched = (absolute: string, isDir: boolean, levels: Level[]): Match => {
    let result: Match = "none";
    for (const set of overrides) {
      let match = test(set.matcher, candidate(set.root, absolute), isDir);
      if (match === "none" && set.whitelists && !isDir) match = "ignore";
      if (match === "ignore") return "ignore";
      if (match === "whitelist") result = "whitelist";
    }
    if (result !== "none") return result;
    for (let kind = 0; kind < 3; kind++) {
      for (const level of levels) {
        const matcher = level.kinds[kind];
        const match = matcher && test(matcher, candidate(level.dir, absolute), isDir);
        if (match && match !== "none") return match;
      }
    }
    return global ? test(global, toPosix(absolute).replace(/^\/+/u, ""), isDir) : "none";
  };

  const files: string[] = [];
  const visit = (dir: string, levels: Level[]) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const isDir = entry.isDirectory();
      if (!isDir && !entry.isFile()) continue; // symlinks and special files
      const absolute = join(dir, entry.name);
      if (matched(absolute, isDir, levels) === "ignore") continue;
      if (isDir) visit(absolute, [readLevel(absolute), ...levels]);
      else files.push(candidate(root, absolute));
    }
  };
  visit(root, [readLevel(root), ...ancestors]);
  return files.sort(comparePaths);
}

export interface Selection {
  /** Absolute repository root the definition's include/exclude are relative to. */
  cwd: string;
  /** Absolute directory to walk; the invocation's include/exclude are relative to it. */
  targetRoot: string;
  language: string;
  definition: { include?: readonly string[]; exclude?: readonly string[] };
  invocation: { include?: readonly string[]; exclude?: readonly string[] };
  /**
   * Global git excludes file. `undefined` discovers it like the `ignore`
   * crate (`core.excludesFile`, else `$XDG_CONFIG_HOME/git/ignore`); `null`
   * disables it.
   */
  globalExcludes?: string | null;
}

/**
 * The effective file set of one JSSG invocation: files under the target root
 * accepted by the definition's applicability (repository-relative, defaulting
 * to `**` + `/*<ext>` for each of the language's extensions, exactly as
 * `CodemodExecutionConfig::build_globs` derives them) and by the invocation
 * target (target-root-relative). Paths are `/`-separated and relative to the
 * target root, sorted component-wise.
 */
export function selectFiles(selection: Selection): string[] {
  const { definition, invocation } = selection;
  const extensions = languageExtensions(selection.language);
  const include =
    definition.include ??
    (extensions.length > 0 ? extensions.map((extension) => `**/*${extension}`) : undefined);
  const overrides: Override[] = [];
  if (include !== undefined || definition.exclude !== undefined) {
    overrides.push(override(selection.cwd, include, definition.exclude));
  }
  if (invocation.include !== undefined || invocation.exclude !== undefined) {
    overrides.push(override(selection.targetRoot, invocation.include, invocation.exclude));
  }
  return walk(selection.targetRoot, overrides, selection.globalExcludes);
}
