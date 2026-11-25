/**
 * Testing a model: run it on a fixture file and compare what it states with
 * a text file kept next to the fixture.
 */

type Fact = any;

function at(node: { file: string; start: number; end: number }, sources: Map<string, string>): string {
  const source = sources.get(node.file) ?? "";
  const line = source.slice(0, node.start).split("\n").length;
  const text = source.slice(node.start, node.end).split("\n")[0];
  const name = node.file.slice(node.file.lastIndexOf("/") + 1);
  return `${name}:${line} \`${text.length > 48 ? `${text.slice(0, 45)}...` : text}\``;
}

function keyText(key: any): string {
  return `${key.protocol} ${key.text ?? key.parts.map((p: any) => (typeof p === "string" ? p : JSON.stringify(p))).join("/")}`;
}

/**
 * One line per fact, with the line and the first line of text of each node
 * instead of offsets, so the expected file survives reformatting:
 *
 *     entry-point http "GET /api/users/[id]" route.ts:5 `export async function GET(`
 *     consume http GET /api/users/{id} route.ts:5 ... receives id=route.ts:5 `params`
 *     tag source:user-input route.ts:5 `request`
 */
export function describeFacts(facts: Fact[], sources: Map<string, string> | Record<string, string>): string {
  const map = sources instanceof Map ? sources : new Map(Object.entries(sources));
  const ports = (label: string, list: any[]) =>
    list.length === 0 ? "" : ` ${label} ${list.map((port) => `${port.name}=${at(port.node, map)}`).join(", ")}`;
  return facts
    .map((fact) => {
      switch (fact.kind) {
        case "entry-point":
          return `entry-point ${fact.entryKind} ${JSON.stringify(fact.label ?? "")} ${at(fact.node, map)}`;
        case "invokes":
          return `invokes ${at(fact.from, map)} -> ${at(fact.to, map)}`;
        case "flows":
          return `flows ${at(fact.from, map)} -> ${at(fact.to, map)}`;
        case "channel":
          return `${fact.side === "producer" ? "produce" : "consume"} ${keyText(fact.key)} ${at(fact.node, map)}${ports("sends", fact.sends)}${ports("receives", fact.receives)}`;
        case "tag":
          return `tag ${fact.labels.join(",")} ${at(fact.node, map)}`;
        default:
          return JSON.stringify(fact);
      }
    })
    .join("\n");
}
