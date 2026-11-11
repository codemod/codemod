// Plain DOM, no build. Routes: #/<workflow> and #/<workflow>/<run>.
const $ = (html) => {
  const t = document.createElement("template");
  t.innerHTML = html.trim();
  return t.content.firstElementChild;
};
export const esc = (v) =>
  String(v ?? "").replace(
    /[&<>"']/g,
    (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c],
  );
const api = async (path, init) => {
  const res = await fetch(`/api/${path}`, init);
  const body = await res.json();
  if (!res.ok) throw new Error(body.error ?? res.statusText);
  return body;
};
const ms = (n) =>
  n == null
    ? ""
    : n < 1000
      ? `${n} ms`
      : n < 60_000
        ? `${(n / 1000).toFixed(1)} s`
        : `${Math.floor(n / 60_000)}m ${Math.round((n % 60_000) / 1000)}s`;
const when = (t) =>
  new Date(t).toLocaleString([], {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
const pill = (status) => `<span class="pill s-${esc(status)}">${esc(status)}</span>`;

let workflows = [];
let timer;
let openStep = null;
const viewCache = new Map();

async function loadView(name) {
  if (!viewCache.has(name))
    viewCache.set(
      name,
      import(`/views/${name}`).catch(() => null),
    );
  return viewCache.get(name);
}

async function route() {
  clearTimeout(timer);
  const [, wf, runId] = location.hash.split("/");
  workflows = await api("workflows");
  renderNav(wf);
  if (!wf) {
    if (workflows[0]) location.hash = `#/${workflows[0].name}`;
    return;
  }
  const workflow = workflows.find((w) => w.name === wf);
  if (!workflow) return;
  await renderWorkflow(workflow, runId);
}

function renderNav(active) {
  const nav = document.getElementById("workflows");
  nav.replaceChildren(
    ...workflows.map((w) =>
      $(
        `<a href="#/${esc(w.name)}" class="${w.name === active ? "active" : ""}">${esc(w.title)}<small>${w.runs} run${w.runs === 1 ? "" : "s"}</small></a>`,
      ),
    ),
  );
}

async function renderWorkflow(workflow, runId) {
  const main = document.getElementById("main");
  const runs = await api(`runs/${workflow.name}`);
  const selected = runId ?? runs[0]?.id;
  const keep = main.querySelector(`[data-workflow="${workflow.name}"]`);
  const target = keep?.querySelector("[name=target]")?.value ?? workflow.target;
  const input = keep?.querySelector("[name=input]")?.value ?? workflow.input;

  const page = $(`<div data-workflow="${esc(workflow.name)}">
    <h2>${esc(workflow.title)}</h2>
    <div class="muted">${esc(workflow.description)} <code>workflows/${esc(workflow.name)}/workflow.ts</code></div>
    <div class="card"><form class="row">
      <label>Target<input name="target" value="${esc(target)}" /></label>
      ${workflow.input || input ? `<label>Input (JSON)<input name="input" value="${esc(input)}" /></label>` : ""}
      <button class="primary">Start run</button>
    </form></div>
    <h3>Runs</h3>
    <div class="card" style="padding:0">${runsTable(runs, workflow.name, selected)}</div>
    <div id="run"></div>
  </div>`);
  page.querySelector("form").onsubmit = async (e) => {
    e.preventDefault();
    const form = new FormData(e.target);
    const run = await api("runs", {
      method: "POST",
      body: JSON.stringify({
        workflow: workflow.name,
        target: form.get("target"),
        input: form.get("input") || undefined,
      }),
    });
    location.hash = `#/${workflow.name}/${run.id}`;
  };
  page
    .querySelectorAll("tr[data-run]")
    .forEach((tr) => (tr.onclick = () => (location.hash = `#/${workflow.name}/${tr.dataset.run}`)));
  main.replaceChildren(page);
  if (selected) await renderRun(workflow, selected, page.querySelector("#run"));
  if (runs.some((r) => r.status === "running")) timer = setTimeout(route, 1000);
}

function runsTable(runs, workflow, selected) {
  if (runs.length === 0) return `<p class="muted" style="padding:12px 16px">No runs yet.</p>`;
  return `<table><thead><tr><th>Run</th><th>Status</th><th>Started</th><th class="num">Duration</th><th>Target</th><th>Notes</th></tr></thead><tbody>
    ${runs
      .slice(0, 12)
      .map(
        (
          r,
        ) => `<tr class="clickable ${r.id === selected ? "selected" : ""}" data-run="${esc(r.id)}">
          <td class="mono">${esc(r.id)}</td><td>${pill(r.status)}</td><td>${when(r.startedAt)}</td>
          <td class="num">${ms((r.endedAt ?? Date.now()) - r.startedAt)}</td><td class="mono">${esc(r.target)}</td>
          <td class="muted">${r.resumeFrom ? `resumed from ${esc(r.resumeFrom)}` : ""}</td></tr>`,
      )
      .join("")}</tbody></table>`;
}

async function renderRun(workflow, id, el) {
  const run = await api(`runs/${workflow.name}/${id}`);
  const start = Math.min(run.startedAt, ...run.steps.map((s) => s.scheduledAt ?? Infinity));
  const end = Math.max(run.endedAt ?? Date.now(), ...run.steps.map((s) => s.endedAt ?? 0));
  const span = Math.max(end - start, 1);
  const pct = (t) => `${(((t - start) / span) * 100).toFixed(2)}%`;
  const width = (a, b) => `${Math.max(((b - a) / span) * 100, 0.4).toFixed(2)}%`;

  const node = $(`<div>
    <h3>Run <span class="mono">${esc(run.id)}</span> ${pill(run.status)}</h3>
    <div class="card">
      <div class="stats">
        <div class="stat"><b>${ms((run.endedAt ?? Date.now()) - run.startedAt)}</b><span>wall time</span></div>
        <div class="stat"><b>${run.steps.length}</b><span>steps</span></div>
        <div class="stat"><b>${run.steps.filter((s) => s.status === "replayed").length}</b><span>replayed</span></div>
        <div class="stat"><b>${run.eventCount}</b><span>events</span></div>
      </div>
      ${run.error ? `<pre class="logs s-failed">${esc(run.error)}</pre>` : ""}
      <div class="row" style="margin-top:10px">
        ${run.status === "running" ? `<button data-act="cancel">Cancel</button>` : ""}
        ${run.status !== "running" && run.status !== "succeeded" ? `<button data-act="resume">Resume from history</button>` : ""}
        ${run.status === "succeeded" ? `<button data-act="resume" title="A new run over this history: every step replays">Replay</button>` : ""}
      </div>
    </div>
    <h3>Steps</h3>
    <div class="card" style="padding:0"><table><thead><tr><th>Step</th><th>Kind</th><th>Status</th><th class="num">Queued</th><th class="num">Ran</th><th>Timeline</th></tr></thead><tbody>
      ${run.steps
        .map((s) => {
          const queued = s.startedAt && s.scheduledAt ? s.startedAt - s.scheduledAt : null;
          const ran = s.startedAt ? (s.endedAt ?? Date.now()) - s.startedAt : null;
          return `<tr class="clickable ${openStep === s.id ? "selected" : ""}" data-step="${esc(s.id)}">
            <td><b>${esc(s.id)}</b><div class="muted mono">${esc(s.operation)}</div></td>
            <td>${esc(s.kind)}</td><td>${pill(s.status)}</td>
            <td class="num">${ms(queued)}</td><td class="num">${ms(ran)}</td>
            <td><div class="bar s-${esc(s.status)}">
              ${s.scheduledAt && s.startedAt ? `<span class="wait" style="left:${pct(s.scheduledAt)};width:${width(s.scheduledAt, s.startedAt)}"></span>` : ""}
              ${s.startedAt || s.endedAt ? `<span style="left:${pct(s.startedAt ?? s.endedAt)};width:${width(s.startedAt ?? s.endedAt, s.endedAt ?? Date.now())}"></span>` : ""}
            </div></td></tr>
            ${openStep === s.id ? `<tr><td colspan="6">${s.error ? `<div class="s-failed">${esc(s.error)}</div>` : ""}<pre class="logs">${esc(s.logs || s.errorOutput || "(no output)")}</pre></td></tr>` : ""}`;
        })
        .join("")}
    </tbody></table></div>
    <h3>Results</h3>
    <div class="tabs"><button data-tab="view" class="active">View</button><button data-tab="json">JSON</button><button data-tab="stderr">Engine log</button></div>
    <div class="card" id="results"></div>
  </div>`);

  node.querySelectorAll("tr[data-step]").forEach(
    (tr) =>
      (tr.onclick = () => {
        openStep = openStep === tr.dataset.step ? null : tr.dataset.step;
        renderRun(workflow, id, el);
      }),
  );
  node.querySelector("[data-act=cancel]")?.addEventListener("click", async () => {
    await api(`runs/${workflow.name}/${id}/cancel`, { method: "POST" });
    route();
  });
  node.querySelector("[data-act=resume]")?.addEventListener("click", async () => {
    const next = await api("runs", {
      method: "POST",
      body: JSON.stringify({
        workflow: workflow.name,
        target: run.target,
        input: run.input ?? undefined,
        resumeFrom: run.id,
      }),
    });
    location.hash = `#/${workflow.name}/${next.id}`;
  });

  const results = node.querySelector("#results");
  const showTab = async (tab) => {
    node
      .querySelectorAll("[data-tab]")
      .forEach((b) => b.classList.toggle("active", b.dataset.tab === tab));
    if (tab === "json")
      results.replaceChildren(
        $(
          `<pre class="logs" style="max-height:520px">${esc(JSON.stringify(run.output, null, 2))}</pre>`,
        ),
      );
    else if (tab === "stderr")
      results.replaceChildren(
        $(`<pre class="logs" style="max-height:520px">${esc(run.stderr || "(empty)")}</pre>`),
      );
    else if (!run.output)
      results.replaceChildren(
        $(
          `<p class="muted">${run.status === "running" ? "Running…" : "No output: the run did not finish."}</p>`,
        ),
      );
    else {
      const view = workflow.hasView ? await loadView(workflow.name) : null;
      results.replaceChildren();
      if (view?.render) view.render(run.output, results, { esc });
      else
        results.replaceChildren(
          $(`<pre class="logs">${esc(JSON.stringify(run.output, null, 2))}</pre>`),
        );
    }
  };
  node.querySelectorAll("[data-tab]").forEach((b) => (b.onclick = () => showTab(b.dataset.tab)));
  el.replaceChildren(node);
  await showTab("view");
}

window.addEventListener("hashchange", () => {
  openStep = null;
  route();
});
route();
