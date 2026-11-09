// A decision API stub for e2e tests, run as its own process (the tests drive
// the CLI with spawnSync, which blocks their event loop). Prints its port on
// the first stdout line, answers POST .../systemone (System One shape) and
// POST .../decisions (OpenAI shape) with a yes/no probability of 0.25 per
// question, and appends each request (`{path, authorization, apiKey, body}`)
// as one JSON line to the file named by argv[2].
import { appendFileSync } from "node:fs";
import { createServer } from "node:http";

const log = process.argv[2];

function answer(path, body) {
  const { questions } = JSON.parse(body);
  if (path.endsWith("/systemone")) {
    const answers = Object.fromEntries(
      Object.keys(questions).map((id) => [id, { type: "noul", noul: 0.25 }]),
    );
    return { model: "stub-1", answers, usage: { input_tokens: 3, output_tokens: 1 } };
  }
  if (path.endsWith("/decisions")) {
    const answers = questions.map(({ name }) => ({ type: "predicate", name, probability: 0.25 }));
    return { model: "stub-luna", answers, usage: { input_tokens: 5 } };
  }
  return undefined;
}

const server = createServer((request, response) => {
  let body = "";
  request.setEncoding("utf8");
  request.on("data", (chunk) => (body += chunk));
  request.on("end", () => {
    const { authorization, "api-key": apiKey } = request.headers;
    appendFileSync(log, `${JSON.stringify({ path: request.url, authorization, apiKey, body })}\n`);
    const result = request.method === "POST" ? answer(request.url, body) : undefined;
    response.writeHead(result === undefined ? 404 : 200, { "content-type": "application/json" });
    response.end(JSON.stringify(result ?? { error: { message: "not found" } }));
  });
});
server.listen(0, "127.0.0.1", () => console.log(server.address().port));
