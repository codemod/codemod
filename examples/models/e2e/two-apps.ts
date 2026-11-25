/**
 * The questions the proof asks of the two-app fixture, as a JSSG transform.
 * It is run on `api/app/api/users/[id]/route.ts` and returns its answers as
 * JSON; `model_tests.rs` checks them.
 *
 * `options.params.models`: "on" (default) runs the models first; "off" asks
 * the same questions of the bare analysis.
 */

import { applyModels, channels, describeFacts, exportManifest, runModel } from "@jssg/models";
import httpClient from "../http-client/model.ts";
import nextAppRoutes from "../next-app-routes/model.ts";

const firstLine = (text: string) => text.split("\n")[0];

export default function transform(root: any, options: any) {
  const filename: string = root.filename();
  const base = filename.slice(0, filename.indexOf("/api/app/"));
  const short = (path: string) => path.slice(base.length + 1);
  const open = (relative: string) => root.openFile(`${base}/${relative}`);
  const fn = (inRoot: any, name: string) =>
    inRoot
      .root()
      .findAll({ rule: { any: [{ kind: "function_declaration" }, { kind: "variable_declarator" }] } })
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
  const step = (s: any) => ({
    via: s.via,
    kind: s.kind,
    name: s.name,
    depth: s.depth,
    file: short(s.root.filename()),
    text: firstLine(s.node.text()),
    models: s.models,
    channel: s.channel,
    tags: s.tags,
  });

  const on = options?.params?.models !== "off";
  const applied = on ? applyModels(root, [nextAppRoutes, httpClient], { root: base }) : null;

  // 1. Who calls the route handler?
  const handler = fn(root, "GET");
  const handlerCallers = handler.callers().map(caller);
  const handlerCallersWithoutModels = handler.callers({ models: false }).map(caller);

  // 2. Who, in the end, calls the database function behind it?
  const findUser = fn(open("api/lib/db.ts"), "findUser");
  const findUserCallers = findUser.callers({ transitive: true }).map(caller);

  // 3. Where does what the user typed go?
  const page = open("web/src/users/UserLookup.tsx");
  const typed = page.root().find({ rule: { pattern: "form.elements.userId.value" } });
  const typedFlowsTo = typed.flowsTo().map(step);
  const typedFlowsToWithoutModels = typed.flowsTo({ models: false }).map(step);

  // 4. What does the form's submit handler call?
  const onSubmit = fn(page, "onSubmit");
  const onSubmitCallees = onSubmit
    .callees()
    .filter((c: any) => c.resolved)
    .map((c: any) => ({ name: c.name, via: c.via, file: short(c.root.filename()), models: c.models, channel: c.channel }));

  // 5. Where does the user object the page shows come from?
  const shown = page.root().find({ rule: { pattern: "showUser($ARG)" } }).getMatch("ARG");
  const shownFlowsFrom = shown.flowsFrom().map(step);

  // 6. Which requests found a route, and which did not?
  const ends = on
    ? channels(root).map((end) => ({
        side: end.side,
        key: end.key,
        file: short(end.node.file),
        model: end.model,
        open: end.open,
        matches: end.matches,
      }))
    : [];

  // The model harness: this file's facts, as text.
  const facts = runModel(nextAppRoutes, root, { path: filename, relative: short(filename) });
  const described = describeFacts(facts, { [filename]: root.root().text() });

  return JSON.stringify({
    applied,
    handlerTags: fn(root, "GET").field("parameters").find({ rule: { pattern: "params" } })?.tags() ?? [],
    handlerCallers,
    handlerCallersWithoutModels,
    findUserCallers,
    typedFlowsTo,
    typedFlowsToWithoutModels,
    onSubmitCallees,
    shownFlowsFrom,
    channels: ends,
    facts: described,
    manifest: on ? exportManifest(root, base, { repo: "two-apps" }) : null,
  });
}
