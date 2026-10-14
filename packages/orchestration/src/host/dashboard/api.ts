/**
 * The dashboard's HTTP surface as plain functions, independent of sockets:
 * route matching, the JSON endpoints over a `DashboardSession`, and the SSE
 * frame writer. `server.ts` binds them to `node:http`; tests call them
 * directly, which also keeps them runnable where a sandbox denies `listen`.
 *
 * Stream ids are `<runId>:<seq>`, so a `Last-Event-ID` from an earlier run is
 * never mistaken for a position in the current one: the client gets the
 * current run's snapshot instead. Controls always act on the active run.
 */
import type { DashboardEnvelope, DashboardSnapshot } from "./monitor.ts";
import {
  RunConflictError,
  RunNotFoundError,
  type DashboardSession,
  type RunSummary,
  type SessionNotice,
} from "./session.ts";

export type Method = "GET" | "POST";

export type RouteName =
  | "page"
  | "snapshot"
  | "events"
  | "runs"
  | "run"
  | "start"
  | "restart"
  | "pause"
  | "resume";

export type ApiRoute = Exclude<RouteName, "page" | "events">;

export interface MatchedRoute {
  methods: Partial<Record<Method, RouteName>>;
  runId?: string;
}

const STATIC_ROUTES: Record<string, MatchedRoute> = {
  "/": { methods: { GET: "page" } },
  "/api/snapshot": { methods: { GET: "snapshot" } },
  "/api/events": { methods: { GET: "events" } },
  "/api/runs": { methods: { GET: "runs", POST: "start" } },
  "/api/restart": { methods: { POST: "restart" } },
  "/api/pause": { methods: { POST: "pause" } },
  "/api/resume": { methods: { POST: "resume" } },
};

export function matchRoute(pathname: string): MatchedRoute | undefined {
  const fixed = STATIC_ROUTES[pathname];
  if (fixed !== undefined) return fixed;
  const run = /^\/api\/runs\/([^/]+)$/u.exec(pathname);
  if (run === null) return undefined;
  return { methods: { GET: "run" }, runId: decodeURIComponent(run[1]!) };
}

export interface ApiResponse {
  status: number;
  body: unknown;
}

export interface ApiRequestContext {
  runId?: string;
  /** The raw `Content-Type` header, checked on every control request. */
  contentType?: string;
}

/** The `runs` list plus the two ids a client needs to place itself. */
export interface RunsView {
  current: string | null;
  active: string | null;
  runs: RunSummary[];
}

export function runsView(session: DashboardSession): RunsView {
  return {
    current: session.currentRunId ?? null,
    active: session.activeRunId ?? null,
    runs: session.runs(),
  };
}

/** Every JSON endpoint. Errors the session raises become structured 404/409 bodies. */
export async function handleApi(
  session: DashboardSession,
  route: ApiRoute,
  context: ApiRequestContext = {},
): Promise<ApiResponse> {
  try {
    switch (route) {
      case "snapshot":
        return { status: 200, body: requireSnapshot(session, session.currentRunId) };
      case "runs":
        return { status: 200, body: runsView(session) };
      case "run":
        return { status: 200, body: requireSnapshot(session, context.runId) };
      case "start":
        requireJson(context);
        return { status: 201, body: { run: await session.start() } };
      case "restart":
        requireJson(context);
        return { status: 200, body: await session.restart() };
      case "pause":
        requireJson(context);
        return { status: 200, body: session.pause() };
      case "resume":
        requireJson(context);
        return { status: 200, body: session.resume() };
    }
  } catch (error) {
    if (error instanceof UnsupportedMediaType)
      return { status: 415, body: { error: error.message } };
    if (error instanceof RunNotFoundError) {
      return { status: 404, body: { error: error.message, code: error.code, runId: error.runId } };
    }
    if (error instanceof RunConflictError) {
      return {
        status: 409,
        body: {
          error: error.message,
          code: error.code,
          ...(error.runId === undefined ? {} : { runId: error.runId }),
          ...(error.runStatus === undefined ? {} : { status: error.runStatus }),
          ...(error.code === "run_active" ? { restart: "/api/restart" } : {}),
        },
      };
    }
    throw error;
  }
}

class UnsupportedMediaType extends Error {}

/** Cross-site forms cannot send this type, so a stray form post cannot drive a run. */
function requireJson(context: ApiRequestContext): void {
  const type = context.contentType?.split(";")[0]?.trim();
  if (type !== "application/json") {
    throw new UnsupportedMediaType("control requests must send Content-Type: application/json");
  }
}

function requireSnapshot(session: DashboardSession, runId: string | undefined): DashboardSnapshot {
  if (runId === undefined) throw new RunNotFoundError("current");
  const snapshot = session.snapshot(runId);
  if (snapshot === undefined) throw new RunNotFoundError(runId);
  return snapshot;
}

/** `event: run` payload: the notice plus the list, so a page needs no extra fetch. */
export type RunFrame = SessionNotice & RunsView;

export function sseFrame(id: string | undefined, event: string, data: unknown): string {
  return `${id === undefined ? "" : `id: ${id}\n`}event: ${event}\ndata: ${JSON.stringify(data)}\n\n`;
}

export function streamId(runId: string, seq: number): string {
  return `${runId}:${seq}`;
}

/** Split a `Last-Event-ID`; `undefined` when it is not one this server issued. */
export function parseStreamId(
  value: string | undefined,
): { runId: string; seq: number } | undefined {
  if (value === undefined) return undefined;
  const at = value.lastIndexOf(":");
  if (at <= 0) return undefined;
  const digits = value.slice(at + 1);
  if (!/^\d+$/u.test(digits)) return undefined;
  return { runId: value.slice(0, at), seq: Number(digits) };
}

/**
 * Feed one SSE client. Opens on the current run (catching up from `lastEventId`
 * when it names that run and the buffer reaches back far enough, a fresh
 * snapshot otherwise), then follows every run the session creates: each
 * `run.created` notice is forwarded as `event: run` and followed by the new
 * run's snapshot, after which only that run's envelopes flow. Snapshot or
 * catch-up and subscription happen in one synchronous step, so no envelope can
 * fall between them. Returns the detach function.
 */
export function openEventStream(
  session: DashboardSession,
  lastEventId: string | undefined,
  write: (frame: string) => void,
): () => void {
  let unfollow = (): void => {};
  const sendEnvelope = (envelope: DashboardEnvelope): void => {
    write(sseFrame(streamId(envelope.runId, envelope.seq), "event", envelope));
  };
  const follow = (runId: string, since?: number): void => {
    unfollow();
    const monitor = session.monitor(runId);
    if (monitor === undefined) return;
    const catchUp = since === undefined ? undefined : monitor.since(since);
    if (catchUp === undefined) {
      const snapshot = monitor.snapshot();
      write(sseFrame(streamId(runId, snapshot.seq), "snapshot", snapshot));
    } else {
      for (const envelope of catchUp) sendEnvelope(envelope);
    }
    unfollow = monitor.subscribe(sendEnvelope);
  };

  write("retry: 1000\n\n");
  const current = session.currentRunId;
  if (current === undefined) {
    write(": waiting for the first run\n\n");
  } else {
    const last = parseStreamId(lastEventId);
    follow(current, last?.runId === current ? last.seq : undefined);
  }
  const unsubscribe = session.subscribe((notice) => {
    const frame: RunFrame = { ...notice, ...runsView(session) };
    write(sseFrame(undefined, "run", frame));
    if (notice.type === "run.created") follow(notice.run.runId);
  });
  return () => {
    unfollow();
    unsubscribe();
  };
}
