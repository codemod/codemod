/**
 * File selection for one JSSG invocation, with the workflow engine's walker
 * semantics (`codemod_walk_builder` plus `OverrideBuilder` in the Rust
 * engine): hidden files visited, symlinks skipped and never followed,
 * `.ignore` / `.gitignore` / `.git/info/exclude` and the global git excludes
 * honored without requiring a git repository, ignore files in ancestor
 * directories applied, and include/exclude globs that take precedence over
 * every ignore file. Deterministic component-wise order.
 *
 * Matching is the `ignore` npm package (pure JavaScript): gitignore semantics,
 * which is also what the engine's override globs use, plus brace alternation
 * (globset has it, gitignore does not) expanded up front.
 * `fixtures/walker/cases.json` pins parity with the engine;
 * `tests/files.test.ts` and `crates/jssg-batch/tests/contracts.rs` run
 * the two sides. Every filesystem and path operation goes through the `Host`
 * (`core/host.ts`).
 */
import ignoreModule, { type Ignore } from "ignore";
import { errorCode, getHost, type Host } from "../core/host.ts";
import type { BatchFile } from "../core/protocol.ts";
import LANGUAGES from "./languages.json" with { type: "json" };

// `ignore` is CommonJS; its default export is the factory under both Node's
// interop and its own type declarations only through `.default`.
const ignore = ignoreModule.default;

/**
 * The engine's file extensions for a language (`languages.json`, pinned to
 * `create_language_extension_map` by a cargo test). Empty for languages the
 * table does not list, which then need an explicit `include`.
 */
export function languageExtensions(language: string): string[] {
  return (LANGUAGES as Record<string, string[]>)[language.toLowerCase()] ?? [];
}

type Match = "ignore" | "whitelist" | "none";

interface Override {
  /** Absolute directory the patterns are relative to. */
  root: string;
  /** The `ignore` lines `matcher` is built from (what `Host.selectFiles` receives). */
  patterns: string[];
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
  return { root, patterns: lines, matcher: ignore().add(lines), whitelists };
}

function test(matcher: Ignore, candidate: string, isDir: boolean): Match {
  const { ignored, unignored } = matcher.test(isDir ? `${candidate}/` : candidate);
  return ignored ? "ignore" : unignored ? "whitelist" : "none";
}

function readIgnoreFile(host: Host, path: string): Ignore | undefined {
  try {
    return ignore().add(host.fs.readText(path));
  } catch {
    return undefined;
  }
}

function readLevel(host: Host, dir: string): Level {
  const { join } = host.path;
  return {
    dir,
    kinds: [".ignore", ".gitignore", join(".git", "info", "exclude")].map((name) =>
      readIgnoreFile(host, join(dir, name)),
    ),
  };
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
  env: Readonly<Record<string, string | undefined>> = getHost().env(),
  host: Host = getHost(),
): string | undefined {
  const { join } = host.path;
  const home = env.HOME ?? env.USERPROFILE ?? host.os.homedir();
  const xdg = env.XDG_CONFIG_HOME ? env.XDG_CONFIG_HOME : home ? join(home, ".config") : undefined;
  const fromConfig = (path: string | undefined): string | undefined => {
    if (!path) return undefined;
    let text: string;
    try {
      text = host.fs.readText(path);
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

/**
 * `Host.selectFiles` takes POSIX paths that are absolute, normalized, and
 * nested (every override root is the walk root or an ancestor of it), the
 * shape `executeJssgCommand` always produces. Anything else (a hand-written
 * `selectFiles` call) walks in TypeScript.
 */
function nativeEligible(host: Host, root: string, overrides: Override[]): boolean {
  const { isAbsolute, normalize, sep } = host.path;
  if (sep !== "/") return false;
  const canonical = (path: string) => isAbsolute(path) && normalize(path) === path;
  if (!canonical(root)) return false;
  return overrides.every(
    (set) =>
      canonical(set.root) &&
      (set.root === root || root.startsWith(set.root.endsWith("/") ? set.root : `${set.root}/`)),
  );
}

function walk(
  host: Host,
  root: string,
  overrides: Override[],
  globalExcludes: string | null | undefined,
) {
  const { dirname, join, relative, sep } = host.path;
  const toPosix = (path: string): string => (sep === "/" ? path : path.split(sep).join("/"));
  const globalPath =
    globalExcludes === undefined ? discoverGlobalExcludesPath(host.env(), host) : globalExcludes;
  if (host.selectFiles !== undefined && nativeEligible(host, root, overrides)) {
    return host.selectFiles({
      root,
      overrides: overrides.map((set) => ({
        root: set.root,
        patterns: set.patterns,
        whitelists: set.whitelists,
      })),
      globalExcludes: globalPath ? globalPath : null,
    });
  }
  const global = globalPath ? readIgnoreFile(host, globalPath) : undefined;
  const ancestors: Level[] = [];
  for (let dir = dirname(root); ; dir = dirname(dir)) {
    ancestors.push(readLevel(host, dir));
    if (dirname(dir) === dir) break;
  }
  // Hot path: every entry is tested against every level. Walked paths are
  // normalized joins beneath `base`, so stripping the prefix is exactly
  // `relative()` (which is slow in an interpreter); anything else takes it.
  const candidate = (base: string, absolute: string) => {
    if (absolute.startsWith(base)) {
      if (absolute.length === base.length) return "";
      if (base.endsWith(sep)) return toPosix(absolute.slice(base.length));
      if (absolute[base.length] === sep) return toPosix(absolute.slice(base.length + 1));
    }
    return toPosix(relative(base, absolute));
  };

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
    for (const entry of host.fs.readdir(dir)) {
      const isDir = entry.isDirectory;
      if (!isDir && !entry.isFile) continue; // symlinks and special files
      const absolute = join(dir, entry.name);
      if (matched(absolute, isDir, levels) === "ignore") continue;
      if (isDir) visit(absolute, [readLevel(host, absolute), ...levels]);
      else files.push(candidate(root, absolute));
    }
  };
  visit(root, [readLevel(host, root), ...ancestors]);
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
  /** Filesystem and paths. Defaults to the installed host. */
  host?: Host;
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
  return walk(
    selection.host ?? getHost(),
    selection.targetRoot,
    overrides,
    selection.globalExcludes,
  );
}

/**
 * Read a selected file. `undefined` means it vanished since selection or is
 * not valid UTF-8; both are skipped, as the workflow engine does.
 */
function readSource(host: Host, absolute: string): string | undefined {
  try {
    return host.fs.readText(absolute, { fatal: true });
  } catch (error) {
    const code = errorCode(error);
    if (code === "ENOENT" || code === "EILSEQ") return undefined;
    throw error;
  }
}

/**
 * The content of every selected file (`root`-relative paths, in order),
 * skipping files that vanished or are not UTF-8. Uses the host's batch
 * `fs.readSources` when it has one.
 */
export function readSelectedSources(
  host: Host,
  root: string,
  paths: readonly string[],
): BatchFile[] {
  const { isAbsolute, join, normalize, sep } = host.path;
  if (
    host.fs.readSources !== undefined &&
    sep === "/" &&
    isAbsolute(root) &&
    normalize(root) === root
  ) {
    const contents = host.fs.readSources(root, paths);
    const files: BatchFile[] = [];
    for (let index = 0; index < paths.length; index++) {
      const content = contents[index];
      if (content !== undefined) files.push({ path: paths[index]!, content });
    }
    return files;
  }
  return paths.flatMap((path) => {
    const content = readSource(host, join(root, path));
    return content === undefined ? [] : [{ path, content }];
  });
}
