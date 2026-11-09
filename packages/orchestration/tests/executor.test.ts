/**
 * Artifact identity through the runtime: a JSSG command records the transform
 * as `{ name, hash }`, the executor supplies the source in the request
 * context only, and a history replays on any checkout.
 */
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  MemoryHistoryStore,
  OperationError,
  isSafeRelativePath,
  jssg,
  run,
  dynamic,
  type OperationRequest,
} from "../src/index.ts";
import { echoExecutor } from "./echo-runtime.ts";
import { artifact, ref } from "./helpers.ts";

describe("safe relative paths", () => {
  it.each([
    ["scripts/migrate.ts", true],
    ["scripts/foo..bar.ts", true],
    ["a..b/c", true],
    ["", false],
    ["  ", false],
    ["/abs/x.ts", false],
    ["\\x.ts", false],
    ["\\\\server\\x.ts", false],
    ["C:\\x.ts", false],
    ["c:/x.ts", false],
    ["../x.ts", false],
    ["scripts/../x.ts", false],
    ["scripts\\..\\x.ts", false],
  ])("%j -> %s", (path, ok) => {
    expect(isSafeRelativePath(path)).toBe(ok);
  });
});

describe("jssg definitions", () => {
  it("refuse an unbuilt inline transform and malformed references", () => {
    expect(() => jssg({ name: "inline", language: "typescript", transform: () => null })).toThrow(
      /jssg 'inline': inline transform was not extracted; run the workflow with codemod workflow run -w/u,
    );
    for (const transform of [
      { name: "x", hash: "abc" },
      { name: "x", hash: "A".repeat(64) },
      { name: " ", hash: "a".repeat(64) },
      { name: "x", hash: "a".repeat(64), source: "x" },
    ]) {
      expect(() =>
        jssg({ name: "x", language: "typescript", transform: transform as never }),
      ).toThrow(/must be the \{ name, hash \} reference the build produced/u);
    }
  });

  it("validate selector data and semanticAnalysis.root", () => {
    const define = (selector: unknown) =>
      jssg({ name: "x", language: "typescript", transform: ref("x"), selector: selector as never });
    expect(
      define({ rule: { pattern: "a($B)" }, constraints: { B: { kind: "string" } } }).selector,
    ).toEqual({ rule: { pattern: "a($B)" }, constraints: { B: { kind: "string" } } });
    expect(define(undefined)).not.toHaveProperty("selector");
    for (const bad of [{}, { rule: {} }, { rule: "a" }, { rule: { pattern: "a" }, id: "s" }, "a"]) {
      expect(() => define(bad), JSON.stringify(bad)).toThrow(
        /selector must be JSON with a non-empty 'rule'/u,
      );
    }

    const semantic = (root: string) =>
      jssg({
        name: "x",
        language: "typescript",
        transform: ref("x"),
        semanticAnalysis: { mode: "workspace", root },
      });
    expect(semantic("src..gen").toOperation(undefined).semanticAnalysis).toEqual({
      mode: "workspace",
      root: "src..gen",
    });
    for (const root of ["/src", "C:\\src", "..", "src/../..", " "]) {
      expect(() => semantic(root), root).toThrow(/must be a safe relative path/u);
    }
    expect(() =>
      jssg({
        name: "x",
        language: "typescript",
        transform: ref("x"),
        semanticAnalysis: { mode: "file", root: "src" },
      }),
    ).toThrow(/requires workspace mode/u);
  });
});

const SOURCE = "export default async function transform(root) { return null; }\n";
const built = artifact("migrate", SOURCE);
const migrate = jssg({
  name: "migrate",
  language: "typescript",
  transform: { name: built.name, hash: built.hash },
});
const wf = dynamic(() => migrate({ target: { root: "apps/web" } }));

interface EchoOutput {
  path: string;
  context: { targetRoot: string; artifact: { source: string } };
}

describe("artifacts through an executor", () => {
  let repo: string;
  beforeEach(() => {
    repo = realpathSync.native(mkdtempSync(join(tmpdir(), "codemod-shell-")));
    mkdirSync(join(repo, "apps/web"), { recursive: true });
    writeFileSync(join(repo, "apps/web/a.ts"), "a\n");
  });
  afterEach(() => rmSync(repo, { recursive: true, force: true }));

  it("records the reference in history and sends the source only in the request context", async () => {
    const executor = echoExecutor({ cwd: repo, artifacts: new Map([[built.hash, built]]) });
    const store = new MemoryHistoryStore();

    const first = await run(wf, { executor, history: store });

    const [output] = first.output as EchoOutput[];
    expect(output).toMatchObject({
      path: "a.ts",
      context: { targetRoot: join(repo, "apps/web"), artifact: { source: SOURCE } },
    });
    const scheduled = first.history.events.find((event) => event.type === "scheduled");
    expect(scheduled).toEqual({
      type: "scheduled",
      command: {
        id: "migrate",
        runnable: "migrate",
        kind: "jssg",
        operation: {
          kind: "jssg",
          transform: { name: "migrate", hash: built.hash },
          language: "typescript",
          target: { root: "apps/web" },
        },
      },
    });
    // Only the completion (which the echo executor fills with the echoed
    // context) mentions the source or the root; the recorded command does not.
    expect(JSON.stringify(scheduled)).not.toContain("transform(root");
    expect(JSON.stringify(scheduled)).not.toContain(repo);

    // A replay needs neither the artifact nor the checkout it was built on.
    const executed: OperationRequest[] = [];
    const elsewhere = echoExecutor({ cwd: repo });
    const second = await run(wf, {
      executor: {
        execute(request, signal) {
          executed.push(request);
          return elsewhere.execute(request, signal);
        },
      },
      history: MemoryHistoryStore.fromJSON(store.serialize()),
    });
    expect(second.replayed).toBe(true);
    expect(second.output).toEqual(first.output);
    expect(executed).toHaveLength(0);
  });

  it("fails a command whose artifact the executor does not hold, before dispatching", async () => {
    const executor = echoExecutor({ cwd: repo });
    const store = new MemoryHistoryStore();
    const error = await run(wf, { executor, history: store }).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(OperationError);
    expect((error as OperationError).status).toBe("failed");
    expect((error as OperationError).detail).toEqual({
      message: expect.stringMatching(
        /no built artifact for jssg 'migrate' \(hash [0-9a-f]{12}\); run the workflow with codemod workflow run -w/u,
      ),
      details: { phase: "artifact" },
    });
    expect(store.toJSON().events.map((e) => e.type)).toEqual(["scheduled", "completed"]);
  });
});
