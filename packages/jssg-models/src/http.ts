/**
 * HTTP channel keys.
 *
 * A key is the method and the path's segments. Every router and client
 * writes a varying segment its own way; they all become the same hole:
 *
 *     /users/:id      Express, Hono, React Router
 *     /users/[id]     Next.js, SvelteKit
 *     /users/{id}     OpenAPI, FastAPI, Spring
 *     /users/<id>     Flask
 *     /users/${id}    a template literal in a client
 *
 * The key is `http.route.method` + `http.route` in OpenTelemetry's HTTP
 * semantic conventions, with the parameter names dropped: two frameworks
 * rarely agree on them, and position is what matches.
 */

import type { ChannelKey, KeyPart, Ports, SgNode } from "./index.ts";
import type { StringPart } from "./values.ts";

const METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

function segment(text: string): KeyPart {
  let found: RegExpMatchArray | null;
  if ((found = text.match(/^\[\[?\.\.\.(\w+)\]?\]$/))) return { rest: found[1] };
  if (text === "*" || text === "**") return { rest: "" };
  if ((found = text.match(/^:(\w+)[?*+]?$/))) return { hole: found[1] };
  if ((found = text.match(/^\[(\w+)\]$/))) return { hole: found[1] };
  if ((found = text.match(/^\{(\w+)(?::[^}]*)?\}$/))) return { hole: found[1] };
  if ((found = text.match(/^<(?:\w+:)?(\w+)>$/))) return { hole: found[1] };
  try {
    return decodeURIComponent(text);
  } catch {
    return text;
  }
}

/** Drop the origin, the query and the fragment; keep the path. */
function pathOf(url: string): string {
  return url.replace(/^[a-z][a-z0-9+.-]*:\/\/[^/]*/i, "").replace(/[?#].*$/, "");
}

function label(parts: KeyPart[]): string {
  const path = parts
    .slice(1)
    .map((part) =>
      typeof part === "string" ? part : "hole" in part ? `{${part.hole}}` : "rest" in part ? `{...${part.rest}}` : "?",
    )
    .join("/");
  return `${parts[0]} /${path}`;
}

export const http = {
  METHODS,

  /**
   * The key of a route as a router writes it: `http.key("get", "/users/:id")`,
   * `http.key("GET", "/users/[id]")`.
   */
  key(method: string, route: string): ChannelKey {
    const parts: KeyPart[] = [method.toUpperCase()];
    for (const text of pathOf(route).split("/")) {
      if (text !== "") parts.push(segment(text));
    }
    return { protocol: "http", parts, text: label(parts) };
  },

  /** The route parameters of a key, in order: `["id"]`. */
  params(key: ChannelKey): string[] {
    return key.parts.flatMap((part) =>
      typeof part === "string" ? [] : "hole" in part && part.hole ? [part.hole] : "rest" in part && part.rest ? [part.rest] : [],
    );
  },

  /**
   * The key of a request a client builds, from the parts of its URL
   * (`stringParts(url)`), and the ports for the values put into the path.
   *
   *     `/api/users/${id}`   GET /api/users/{p0}, sends { p0: id }
   *
   * A value takes one segment. A URL that starts with a value followed by
   * `/` (`${API_URL}/users`) is taken to start with an origin, which is
   * dropped. A URL the code does not show (a parameter) gives an open key:
   * `GET /?`.
   */
  request(method: string, url: StringPart[], base: StringPart[] = []): { key: ChannelKey; sends: Ports } {
    const parts: KeyPart[] = [method.toUpperCase()];
    const sends: Ports = {};
    let pieces = [...base, ...url];
    // `${ORIGIN}/path`: the origin is not part of the route.
    const [first, second] = pieces;
    if (first && "node" in first && second && "text" in second && second.text.startsWith("/")) pieces = pieces.slice(1);
    if (pieces[0] && "text" in pieces[0]) pieces[0] = { text: pathOf(pieces[0].text) };

    // One segment at a time: text up to a `/`, with at most the values in it.
    let text = "";
    let holes: SgNode[] = [];
    let open = false;
    const flush = () => {
      if (holes.length > 0) {
        const name = `p${Object.keys(sends).length}`;
        sends[name] = holes;
        parts.push({ hole: name });
      } else if (text !== "") {
        parts.push(segment(text));
      }
      text = "";
      holes = [];
    };
    for (const piece of pieces) {
      if ("unknown" in piece) {
        flush();
        parts.push({ unknown: true });
        open = true;
        break;
      }
      if ("node" in piece) {
        holes.push(piece.node);
        continue;
      }
      const cut = piece.text.replace(/[?#].*$/, "");
      const segments = cut.split("/");
      segments.forEach((part, index) => {
        if (index > 0) flush();
        text += part;
      });
      // A query string ends the path.
      if (cut !== piece.text) break;
    }
    if (!open) flush();
    return { key: { protocol: "http", parts, text: label(parts) }, sends };
  },
};
