/**
 * The cross-language questions: a Next.js frontend (TypeScript), a Flask API
 * and Celery tasks (Python). Run on `api/controllers/apps.py`; answers in
 * JSON for `model_tests.rs`.
 */

import { applyModels, channels } from "@jssg/models";
import celeryTasks from "../celery-tasks/model.ts";
import flaskRoutes from "../flask-routes/model.ts";
import httpClient from "../http-client/model.ts";
import nextAppRoutes from "../next-app-routes/model.ts";

const firstLine = (text: string) => text.split("\n")[0];

export default function transform(root: any, options: any) {
  const filename: string = root.filename();
  const base = filename.slice(0, filename.indexOf("/api/controllers/"));
  const short = (path: string) => path.slice(base.length + 1);
  const open = (relative: string) => root.openFile(`${base}/${relative}`);
  const fn = (inRoot: any, name: string) =>
    inRoot
      .root()
      .findAll({ rule: { kind: inRoot.filename().endsWith(".py") ? "function_definition" : "function_declaration" } })
      .find((node: any) => node.field("name")?.text() === name);

  const caller = (c: any) => ({
    name: c.name,
    via: c.via,
    depth: c.depth,
    file: short(c.root.filename()),
    site: firstLine(c.callSite.node.text()),
    models: c.models,
    channel: c.channel,
  });
  const callee = (c: any) => ({ name: c.name, via: c.via, file: short(c.root.filename()), models: c.models, channel: c.channel });
  const step = (s: any) => ({
    via: s.via,
    depth: s.depth,
    file: short(s.root.filename()),
    text: firstLine(s.node.text()),
    models: s.models,
    channel: s.channel,
    tags: s.tags,
  });

  const on = options?.params?.models !== "off";
  const applied = on ? applyModels(root, [nextAppRoutes, httpClient, flaskRoutes, celeryTasks], { root: base }) : null;

  // Python: who calls the Flask view? (Flask, and the TypeScript frontend.)
  const view = fn(root, "run_app");
  const viewCallers = view.callers().map(caller);
  // Python: what does the view call that the code does not show? (The Celery task.)
  const viewCallees = view.callees().map(callee);

  // Python: who, in the end, runs the task? (The view, and behind it the frontend.)
  const task = fn(open("api/tasks/run_app.py"), "run_app_task");
  const taskCallers = task.callers({ transitive: true }).map(caller);

  // Python: a task sent by name.
  const mail = fn(open("api/tasks/mail.py"), "send_invite");
  const mailCallers = mail.callers().map(caller);
  // Python: a task nothing sends is still an entry point.
  const digestCallers = fn(open("api/tasks/mail.py"), "send_digest").callers().map(caller);

  // TypeScript: what does the frontend's `run` call? (The Python view.)
  const page = open("web/app/apps/[id]/run-button.tsx");
  const run = fn(page, "run");
  const runCallees = run
    .callees()
    .filter((c: any) => c.resolved)
    .map(callee);

  // TypeScript → Python: where does the app id in the URL go?
  const appId = fn(page, "RunButton").field("parameters").find({ rule: { kind: "shorthand_property_identifier_pattern" } });
  const appIdFlowsTo = appId.flowsTo().map(step);
  // … and the request body.
  const body = page.root().find({ rule: { pattern: "JSON.stringify($BODY)" } }).getMatch("BODY");
  const bodyFlowsTo = body.flowsTo().map(step);

  // Python → Python, across the queue: the task's first parameter comes from the view.
  const taskParameter = task.field("parameters").find({ rule: { pattern: "app_id" } });
  const taskParameterFlowsFrom = taskParameter.flowsFrom().map(step);

  const ends = on
    ? channels(root).map((end) => ({ side: end.side, key: end.key, file: short(end.node.file), matches: end.matches, open: end.open }))
    : [];

  return JSON.stringify({
    applied,
    viewCallers,
    viewCallees,
    taskCallers,
    mailCallers,
    digestCallers,
    runCallees,
    appIdFlowsTo,
    bodyFlowsTo,
    taskParameterFlowsFrom,
    channels: ends,
  });
}
