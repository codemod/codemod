/**
 * Loopback HTTP host for a dashboard session: the page, JSON endpoints for
 * the current and retained runs, an SSE stream that follows the session's
 * newest run with reconnect catch-up, and the operator controls (pause,
 * resume, restart, new run). Trusted local use only: it binds to 127.0.0.1,
 * has no authentication, and lives as long as the session that owns it.
 *
 * The stream never blocks a run. Each frame is written with a non-blocking
 * `write`; a client whose socket backlog exceeds a fixed bound is dropped and
 * reconnects with `Last-Event-ID`, catching up from the monitor's buffer or
 * from a fresh snapshot when it fell too far behind. Routing, the endpoint
 * bodies, and the stream frames live in `api.ts`.
 */
import { readFileSync } from "node:fs";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import { handleApi, matchRoute, openEventStream, type Method } from "./api.ts";
import type { DashboardSession } from "./session.ts";

export interface DashboardOptions {
  session: DashboardSession;
  /** `0` (the default) picks a free port. */
  port?: number;
}

export interface Dashboard {
  url: string;
  port: number;
  /** Ends every stream and stops listening; resolves once the server is closed. */
  close(): Promise<void>;
}

export const DASHBOARD_HOST = "127.0.0.1";

/** Bytes a stream client may leave unread before it is dropped to reconnect. */
const MAX_CLIENT_BACKLOG = 1024 * 1024;

let cachedPage: string | undefined;
function page(): string {
  cachedPage ??= readFileSync(new URL("./ui.html", import.meta.url), "utf8");
  return cachedPage;
}

export async function startDashboard(options: DashboardOptions): Promise<Dashboard> {
  const { session } = options;
  /** Open streams and how to detach each from the session. */
  const streams = new Map<ServerResponse, () => void>();
  let port = 0;

  const stream = (req: IncomingMessage, res: ServerResponse): void => {
    res.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-store",
      Connection: "keep-alive",
    });
    let open = true;
    const detach = (): void => {
      if (!open) return;
      open = false;
      streams.get(res)?.();
      streams.delete(res);
    };
    const write = (frame: string): void => {
      if (!open) return;
      res.write(frame);
      if (res.writableLength > MAX_CLIENT_BACKLOG) {
        // Not keeping up; the client reconnects and catches up from the buffer.
        detach();
        res.destroy();
      }
    };
    const raw = req.headers["last-event-id"];
    const unsubscribe = openEventStream(session, Array.isArray(raw) ? raw[0] : raw, write);
    streams.set(res, unsubscribe);
    res.once("close", detach);
  };

  const server = createServer((req, res) => {
    if (!isLocalHost(req.headers.host, port)) {
      json(res, 403, { error: "the dashboard only answers loopback requests" });
      return;
    }
    const pathname = new URL(req.url ?? "/", `http://${DASHBOARD_HOST}`).pathname;
    const route = matchRoute(pathname);
    if (route === undefined) {
      json(res, 404, { error: "not found" });
      return;
    }
    const name = route.methods[req.method as Method];
    if (name === undefined) {
      res.setHeader("Allow", Object.keys(route.methods).join(", "));
      json(res, 405, { error: "method not allowed" });
      return;
    }
    switch (name) {
      case "page":
        res.writeHead(200, {
          "Content-Type": "text/html; charset=utf-8",
          "Cache-Control": "no-store",
        });
        res.end(page());
        return;
      case "events":
        stream(req, res);
        return;
      default:
        // Request bodies are ignored; every control is a bare POST.
        req.resume();
        handleApi(session, name, {
          ...(route.runId === undefined ? {} : { runId: route.runId }),
          ...(req.headers["content-type"] === undefined
            ? {}
            : { contentType: req.headers["content-type"] }),
        }).then(
          (response) => json(res, response.status, response.body),
          (error: unknown) =>
            json(res, 500, { error: error instanceof Error ? error.message : String(error) }),
        );
    }
  });

  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(options.port ?? 0, DASHBOARD_HOST, () => {
      server.off("error", reject);
      resolve();
    });
  });
  port = (server.address() as AddressInfo).port;

  return {
    url: `http://${DASHBOARD_HOST}:${port}/`,
    port,
    close: () =>
      new Promise<void>((resolve, reject) => {
        for (const [res, unsubscribe] of streams) {
          unsubscribe();
          res.end();
        }
        streams.clear();
        server.close((error) => (error ? reject(error) : resolve()));
        server.closeAllConnections();
      }),
  };
}

/** Only the loopback names this server was bound under; defeats DNS rebinding. */
function isLocalHost(header: string | undefined, port: number): boolean {
  return header === `${DASHBOARD_HOST}:${port}` || header === `localhost:${port}`;
}

function json(res: ServerResponse, status: number, body: unknown): void {
  res.writeHead(status, { "Content-Type": "application/json", "Cache-Control": "no-store" });
  res.end(JSON.stringify(body));
}
