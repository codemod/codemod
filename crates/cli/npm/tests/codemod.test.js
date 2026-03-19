const test = require("node:test");
const assert = require("node:assert/strict");
const { EventEmitter } = require("node:events");

const {
  CODEMOD_BOOTSTRAPPED_ENV,
  latestBootstrapArgs,
  npmExecutable,
  shouldBootstrapLatestNoCommand,
  run,
} = require("../codemod");

test("shouldBootstrapLatestNoCommand only bootstraps bare invocation", () => {
  assert.equal(shouldBootstrapLatestNoCommand([], {}), true);
  assert.equal(shouldBootstrapLatestNoCommand(["--help"], {}), false);
  assert.equal(shouldBootstrapLatestNoCommand(["init"], {}), false);
  assert.equal(shouldBootstrapLatestNoCommand([], { [CODEMOD_BOOTSTRAPPED_ENV]: "1" }), false);
});

test("latest bootstrap args target codemod@latest with prefer-online", () => {
  assert.deepEqual(latestBootstrapArgs(), [
    "exec",
    "--yes",
    "--prefer-online",
    "--package",
    "codemod@latest",
    "--",
    "codemod",
  ]);
});

test("npmExecutable uses npm.cmd on Windows", () => {
  assert.equal(npmExecutable("win32"), "npm.cmd");
  assert.equal(npmExecutable("linux"), "npm");
});

test("run bootstraps bare no-command invocation through npm exec", () => {
  const calls = [];
  const fakeChild = new EventEmitter();

  run({
    argv: [],
    env: { PATH: process.env.PATH || "" },
    platform: "linux",
    spawnImpl(command, args, options) {
      calls.push({ command, args, options });
      return fakeChild;
    },
    resolveBinaryPathImpl() {
      throw new Error("should not resolve local binary during bootstrap");
    },
  });

  assert.equal(calls.length, 1);
  assert.equal(calls[0].command, "npm");
  assert.deepEqual(calls[0].args, latestBootstrapArgs());
  assert.equal(calls[0].options.stdio, "inherit");
  assert.equal(calls[0].options.env[CODEMOD_BOOTSTRAPPED_ENV], "1");
});

test("run resolves the native binary for explicit commands", () => {
  const calls = [];
  const fakeChild = new EventEmitter();

  run({
    argv: ["init"],
    spawnImpl(command, args, options) {
      calls.push({ command, args, options });
      return fakeChild;
    },
    resolveBinaryPathImpl() {
      return { binaryPath: "/tmp/codemod" };
    },
  });

  assert.equal(calls.length, 1);
  assert.equal(calls[0].command, "/tmp/codemod");
  assert.deepEqual(calls[0].args, ["init"]);
  assert.deepEqual(calls[0].options, { stdio: "inherit" });
});
