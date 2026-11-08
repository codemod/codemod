// A TypeSafe API stub for e2e tests, run as its own process (the tests drive
// the CLI with spawnSync, which blocks their event loop). Prints its port on
// the first stdout line, answers POST /v1/systemone with a `noul` answer per
// question, and appends each request (`{path, authorization, body}`) as one
// JSON line to the file named by argv[2].
import { appendFileSync } from "node:fs";
import { createServer } from "node:http";

const log = process.argv[2];
const server = createServer((request, response) => {
  let body = "";
  request.setEncoding("utf8");
  request.on("data", (chunk) => (body += chunk));
  request.on("end", () => {
    appendFileSync(
      log,
      `${JSON.stringify({ path: request.url, authorization: request.headers.authorization, body })}\n`,
    );
    if (request.method !== "POST" || request.url !== "/v1/systemone") {
      response.writeHead(404, { "content-type": "application/json" });
      response.end(JSON.stringify({ error: { message: "not found" } }));
      return;
    }
    const { questions } = JSON.parse(body);
    const answers = Object.fromEntries(
      Object.keys(questions).map((id) => [id, { type: "noul", noul: 0.25 }]),
    );
    response.writeHead(200, { "content-type": "application/json" });
    response.end(
      JSON.stringify({ model: "stub-1", answers, usage: { input_tokens: 3, output_tokens: 1 } }),
    );
  });
});
server.listen(0, "127.0.0.1", () => console.log(server.address().port));
