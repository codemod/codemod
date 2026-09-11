/**
 * Repository traversal with the workflow engine's walker semantics
 * (`codemod_walk_builder` in `crates/codemod-sandbox`): hidden files visited,
 * symlinks not followed, `.ignore` / `.gitignore` / `.git/info/exclude` and
 * the global git excludes honored without requiring a git repository, ignore
 * files in ancestor directories applied, and include/exclude overrides that
 * take precedence over every ignore file. Deterministic component-wise order.
 *
 * The contract with the engine is `fixtures/walker/cases.json`, checked here
 * by `tests/walker.test.ts` and in Rust by
 * `crates/execution-bridge/tests/walker_parity.rs`.
 */
import { readFileSync, readdirSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, relative, resolve } from "node:path";
import { GitignoreMatcher, OverrideMatcher, parseGitignore, type MatchKind } from "./gitignore.ts";
import { toPosix } from "./paths.ts";

export interface OverrideSet {
  /** Absolute directory the patterns are relative to. */
  root: string;
  matcher: OverrideMatcher;
}

export interface WalkOptions {
  /** Every set must accept a file; any set may prune a directory. */
  overrides?: readonly OverrideSet[];
  /**
   * Global git excludes file. `undefined` discovers it like the `ignore`
   * crate (`core.excludesFile` in `~/.gitconfig` or `$XDG_CONFIG_HOME/git/config`,
   * else `$XDG_CONFIG_HOME/git/ignore`); `null` disables it.
   */
  globalExcludes?: string | null;
}

interface Level {
  dir: string;
  ignore: GitignoreMatcher;
  gitignore: GitignoreMatcher;
  exclude: GitignoreMatcher;
}

/** Component-wise byte order, the order `Vec<PathBuf>::sort` produces. */
export function comparePaths(a: string, b: string): number {
  const left = a.split("/");
  const right = b.split("/");
  const length = Math.min(left.length, right.length);
  for (let index = 0; index < length; index++) {
    const l = left[index]!;
    const r = right[index]!;
    if (l !== r) return l < r ? -1 : 1;
  }
  return left.length - right.length;
}

function readMatcher(path: string): GitignoreMatcher {
  try {
    return new GitignoreMatcher(parseGitignore(readFileSync(path, "utf8")));
  } catch {
    return new GitignoreMatcher([]);
  }
}

function readLevel(dir: string): Level {
  return {
    dir,
    ignore: readMatcher(join(dir, ".ignore")),
    gitignore: readMatcher(join(dir, ".gitignore")),
    exclude: readMatcher(join(dir, ".git", "info", "exclude")),
  };
}

function candidateFor(dir: string, absolute: string): string {
  return toPosix(relative(dir, absolute));
}

/** `gitconfig_excludes_path` from the `ignore` crate. */
export function discoverGlobalExcludesPath(
  env: NodeJS.ProcessEnv = process.env,
): string | undefined {
  const home = env.HOME ?? env.USERPROFILE ?? homedir();
  const xdg =
    env.XDG_CONFIG_HOME !== undefined && env.XDG_CONFIG_HOME !== ""
      ? env.XDG_CONFIG_HOME
      : home
        ? join(home, ".config")
        : undefined;
  const fromConfig = (path: string | undefined): string | undefined => {
    if (!path) return undefined;
    let text: string;
    try {
      text = readFileSync(path, "utf8");
    } catch {
      return undefined;
    }
    const match = /^\s*excludesfile\s*=\s*"?(.+?)"?\s*$/imu.exec(text);
    if (!match) return undefined;
    const value = match[1]!;
    if (value.startsWith("~") && home) return join(home, value.slice(1));
    return value;
  };
  return (
    fromConfig(home ? join(home, ".gitconfig") : undefined) ??
    fromConfig(xdg ? join(xdg, "git", "config") : undefined) ??
    (xdg ? join(xdg, "git", "ignore") : undefined)
  );
}

/**
 * Files beneath `root`, as `/`-separated paths relative to it, sorted
 * component-wise. Symlinks are skipped as entries and never followed.
 */
export function walkFiles(root: string, options: WalkOptions = {}): string[] {
  const absoluteRoot = resolve(root);
  if (!statSync(absoluteRoot).isDirectory()) throw new Error(`not a directory: ${root}`);
  const overrides = (options.overrides ?? []).filter((set) => !set.matcher.empty);
  const globalPath =
    options.globalExcludes === undefined ? discoverGlobalExcludesPath() : options.globalExcludes;
  const global = globalPath ? readMatcher(globalPath) : new GitignoreMatcher([]);
  const ancestors: Level[] = [];
  for (let dir = dirname(absoluteRoot); ; dir = dirname(dir)) {
    ancestors.push(readLevel(dir));
    if (dirname(dir) === dir) break;
  }

  const matchOverrides = (absolute: string, isDir: boolean): MatchKind => {
    let whitelisted: MatchKind = "none";
    for (const set of overrides) {
      const matched = set.matcher.match(candidateFor(set.root, absolute), isDir);
      if (matched === "ignore") return "ignore";
      if (matched === "whitelist") whitelisted = "whitelist";
    }
    return whitelisted;
  };

  // `Ignore::matched_ignore`: nearest `.ignore` beats nearest `.gitignore`
  // beats nearest `.git/info/exclude` beats the global excludes.
  const matchIgnoreFiles = (
    absolute: string,
    isDir: boolean,
    levels: readonly Level[],
  ): MatchKind => {
    let ignore: MatchKind = "none";
    let gitignore: MatchKind = "none";
    let exclude: MatchKind = "none";
    for (const level of levels) {
      const candidate = candidateFor(level.dir, absolute);
      if (ignore === "none") ignore = level.ignore.match(candidate, isDir);
      if (gitignore === "none") gitignore = level.gitignore.match(candidate, isDir);
      if (exclude === "none") exclude = level.exclude.match(candidate, isDir);
    }
    if (ignore !== "none") return ignore;
    if (gitignore !== "none") return gitignore;
    if (exclude !== "none") return exclude;
    return global.match(toPosix(absolute).replace(/^\/+/u, ""), isDir);
  };

  // `Ignore::matched`: an override match, whitelist or ignore, is final.
  const matched = (absolute: string, isDir: boolean, levels: readonly Level[]): MatchKind => {
    const override = matchOverrides(absolute, isDir);
    if (override !== "none") return override;
    return matchIgnoreFiles(absolute, isDir, levels);
  };

  const files: string[] = [];
  const visit = (dir: string, levels: Level[]) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const absolute = join(dir, entry.name);
      const isDir = entry.isDirectory();
      if (!isDir && !entry.isFile()) continue; // symlinks and special files
      if (matched(absolute, isDir, levels) === "ignore") continue;
      if (isDir) visit(absolute, [readLevel(absolute), ...levels]);
      else files.push(toPosix(relative(absoluteRoot, absolute)));
    }
  };
  visit(absoluteRoot, [readLevel(absoluteRoot), ...ancestors]);
  return files.sort(comparePaths);
}

export interface Selection {
  /** Repository root the definition's include/exclude are relative to. */
  cwd: string;
  /** Directory to walk; the invocation's include/exclude are relative to it. */
  targetRoot: string;
  definition: { include?: readonly string[]; exclude?: readonly string[] };
  invocation: { include?: readonly string[]; exclude?: readonly string[] };
  /** The language's extensions; each becomes a `**` + `/*<ext>` glob when the definition has no `include`. */
  extensions: readonly string[];
  globalExcludes?: string | null;
}

/**
 * The effective file set of one JSSG invocation: files under the target root
 * accepted by the definition's applicability (repository-relative, defaulting
 * to the language's extensions like `CodemodExecutionConfig::build_globs`) and
 * by the invocation target (target-root-relative). Paths are relative to the
 * target root.
 */
export function selectFiles(selection: Selection): string[] {
  const intrinsicInclude =
    selection.definition.include ??
    (selection.extensions.length > 0
      ? selection.extensions.map((extension) => `**/*${extension}`)
      : undefined);
  const overrides: OverrideSet[] = [];
  if (intrinsicInclude !== undefined || selection.definition.exclude !== undefined) {
    overrides.push({
      root: selection.cwd,
      matcher: new OverrideMatcher({
        include: intrinsicInclude,
        exclude: selection.definition.exclude,
      }),
    });
  }
  if (selection.invocation.include !== undefined || selection.invocation.exclude !== undefined) {
    overrides.push({
      root: selection.targetRoot,
      matcher: new OverrideMatcher(selection.invocation),
    });
  }
  return walkFiles(selection.targetRoot, {
    overrides,
    globalExcludes: selection.globalExcludes,
  });
}
