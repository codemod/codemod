/**
 * Celery tasks (Python).
 *
 *     @shared_task(queue="app_run")
 *     def run_app_task(app_id, inputs): ...          # tasks/run_app.py
 *
 *     run_app_task.delay(app_id, inputs)              # somewhere else
 *     celery.send_task("tasks.mail.send_invite", args=[email])
 *
 * A worker calls the task function: an entry point, and the consumer of
 * the channel `celery <task name>`. `.delay()`, `.apply_async()` and
 * `send_task()` are its producers. The arguments sent arrive in the task's
 * parameters, by position.
 *
 * A task's name is its `name=` if it has one, else its module path plus the
 * function name. The module path depends on where Python's import root is,
 * which a file does not say, so an unnamed task is stated under every
 * suffix of its path (`api.tasks.mail.send`, `tasks.mail.send`, `mail.send`).
 */

import { constantString, defineModel, python } from "@jssg/models";
import type { ChannelKey, SgNode } from "@jssg/models";

const TASK_DECORATORS = /(^|\.)(shared_task|task)$/;

/** Dotted names are compared whole: one key part. */
const taskKey = (name: string): ChannelKey => ({ protocol: "celery", parts: [name], text: name });

/** `api/tasks/mail.py` → `["api.tasks.mail", "tasks.mail", "mail"]`. */
function modulePaths(relative: string): string[] {
  const segments = relative.replace(/(\/__init__)?\.py$/, "").split("/");
  return segments.map((_, index) => segments.slice(index).join("."));
}

export default defineModel({
  name: "celery-tasks",
  version: "0.1.0",
  files: ["**/*.py"],
  contains: ["shared_task", ".task", ".delay(", ".apply_async(", "send_task("],
  model({ root, file, entryPoint, consume, produce }) {
    // Tasks: the consumers.
    for (const { decorator, fn } of python.decorated(root.root())) {
      const call = decorator.kind() === "call" ? decorator : null;
      const name = (call ? call.field("function") : decorator).text();
      if (!TASK_DECORATORS.test(name)) continue;
      const fnName = fn.field("name").text();
      const explicit = call && python.keyword(call, "name");
      const explicitName = explicit && constantString(explicit);
      const names = explicitName ? [explicitName] : modulePaths(file.relative).map((module) => `${module}.${fnName}`);
      const receives: Record<string, SgNode> = {};
      // A bound task's first parameter is the task itself.
      const bound = call && python.keyword(call, "bind")?.text() === "True";
      python.parameters(fn).slice(bound ? 1 : 0).forEach((parameter, index) => {
        receives[`arg${index}`] = parameter;
        receives[`kw:${parameter.text()}`] = parameter;
      });
      entryPoint(fn, { kind: "task", label: names[0] });
      for (const taskName of names) consume(taskKey(taskName), fn, { receives });
    }

    // What a name used in this file was imported as: `from tasks.mail import send`.
    const imported = python.imports(root.root());
    const here = modulePaths(file.relative)[0];

    for (const call of root.root().findAll({ rule: { kind: "call", has: { field: "function", kind: "attribute" } } })) {
      const callee = call.field("function");
      const method = callee.field("attribute").text();
      const sends: Record<string, SgNode> = {};
      const send = (values: SgNode[]) => values.forEach((value, index) => (sends[`arg${index}`] = value));
      const sendKeywords = (dictionary: SgNode | undefined) => {
        for (const pair of dictionary?.kind() === "dictionary" ? dictionary.findAll({ rule: { kind: "pair" } }) : []) {
          const key = constantString(pair.field("key"));
          if (key) sends[`kw:${key}`] = pair.field("value");
        }
      };

      if (method === "delay" || method === "apply_async") {
        // `task.delay(a, b)`: the task is named by what `task` is.
        const receiver = callee.field("object").text();
        const taskName = imported.get(receiver) ?? `${here}.${receiver}`;
        if (method === "delay") {
          send(python.positional(call));
          for (const [name, value] of python.keywords(call)) sends[`kw:${name}`] = value;
        } else {
          const list = python.keyword(call, "args") ?? python.positional(call)[0];
          send(list ? python.elements(list) : []);
          sendKeywords(python.keyword(call, "kwargs"));
        }
        produce(taskKey(taskName), call, { sends });
      } else if (method === "send_task") {
        // `app.send_task("tasks.mail.send_invite", args=[email])`.
        const [nameNode, positionalArgs] = python.positional(call);
        const taskName = nameNode && constantString(nameNode);
        const list = python.keyword(call, "args") ?? positionalArgs;
        send(list ? python.elements(list) : []);
        sendKeywords(python.keyword(call, "kwargs"));
        produce(taskName ? taskKey(taskName) : { protocol: "celery", parts: [{ unknown: true }], text: "?" }, call, { sends });
      }
    }
  },
});
