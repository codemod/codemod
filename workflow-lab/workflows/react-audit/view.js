// Results view for react-audit: hits per rule per repo, then the findings of one cell.
export function render(output, el, { esc }) {
  const rules = [...new Set(output.repos.flatMap((r) => Object.keys(r.byRule)))].sort();
  const max = Math.max(1, ...output.repos.flatMap((r) => Object.values(r.byRule)));
  const shade = (n) =>
    n
      ? `background: color-mix(in srgb, var(--accent) ${Math.round(8 + (n / max) * 50)}%, transparent)`
      : "";
  el.innerHTML = `
    <div class="stats">
      <div class="stat"><b>${output.total}</b><span>findings</span></div>
      <div class="stat"><b>${output.repos.length}</b><span>repos</span></div>
      <div class="stat"><b>${output.repos.reduce((n, r) => n + r.sourceFiles, 0).toLocaleString()}</b><span>source files</span></div>
      <div class="stat"><b>${rules.length}</b><span>rules with hits</span></div>
    </div>
    <h3>Hits per rule (click a cell)</h3>
    <table><thead><tr><th>Rule</th>${output.repos.map((r) => `<th class="num">${esc(r.repo)}<div class="muted mono">${esc(r.commit)}</div></th>`).join("")}<th class="num">Total</th></tr></thead>
    <tbody>${rules
      .map(
        (rule) =>
          `<tr><td class="mono">${esc(rule)}</td>${output.repos
            .map(
              (r) =>
                `<td class="heat clickable" data-repo="${esc(r.repo)}" data-rule="${esc(rule)}" style="${shade(r.byRule[rule] ?? 0)}">${r.byRule[rule] ?? ""}</td>`,
            )
            .join(
              "",
            )}<td class="num"><b>${output.repos.reduce((n, r) => n + (r.byRule[rule] ?? 0), 0)}</b></td></tr>`,
      )
      .join("")}
      <tr><td><b>Total</b></td>${output.repos.map((r) => `<td class="num"><b>${r.total}</b></td>`).join("")}<td class="num"><b>${output.total}</b></td></tr>
    </tbody></table>
    <div id="cell"></div>`;
  el.querySelectorAll("td[data-rule]").forEach(
    (td) =>
      (td.onclick = () => {
        const repo = output.repos.find((r) => r.repo === td.dataset.repo);
        const rows = repo.findings.filter((f) => f.rule === td.dataset.rule);
        el.querySelector("#cell").innerHTML =
          `<h3>${esc(td.dataset.rule)} in ${esc(repo.repo)} (${rows.length})</h3>
          <table><thead><tr><th>File</th><th class="num">Line</th><th>Details</th></tr></thead><tbody>${rows
            .slice(0, 25)
            .map(
              (f) =>
                `<tr><td class="mono">${esc(f.file)}</td><td class="num">${f.line}</td><td class="muted mono">${esc(
                  Object.entries(f.dims)
                    .map(([k, v]) => `${k}=${v}`)
                    .join(" "),
                )}</td></tr>`,
            )
            .join(
              "",
            )}</tbody></table>${rows.length > 25 ? `<p class="muted">and ${rows.length - 25} more in the JSON tab</p>` : ""}`;
      }),
  );
}
