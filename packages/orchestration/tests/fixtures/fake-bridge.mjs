#!/usr/bin/env node
// Stand-in for butterflow-execution-bridge in unit tests: echoes the request
// it received inside a succeeded completion so tests can inspect the wire.
import { readFileSync, writeFileSync } from "node:fs";

const [requestPath, responsePath] = process.argv.slice(2);
const request = JSON.parse(readFileSync(requestPath, "utf8"));
writeFileSync(
  responsePath,
  JSON.stringify({
    protocolVersion: 2,
    commandId: request.commandId,
    status: "succeeded",
    output: { request },
  }),
);
