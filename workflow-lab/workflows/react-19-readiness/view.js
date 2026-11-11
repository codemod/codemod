// Results view for react-19-readiness: one row per package, what blocks React 19, and the declared versions.
const LABELS = {
  "legacy-root-api": "ReactDOM.render / hydrate",
  "find-dom-node": "findDOMNode",
  "test-utils": "react-dom/test-utils",
  "string-ref": "string refs",
  "default-props": "defaultProps",
  "prop-types": "propTypes",
  "legacy-context": "legacy context",
  "create-factory": "createFactory",
};
export function render(output, el, { esc }) {
  const cats = Object.keys(output.totals);
  const blocked = output.packages.filter((p) => p.total > 0);
  const range = (p, name) => p.deps[name] ?? "";
  el.innerHTML = `
    <div class="stats">
      <div class="stat"><b>${output.blocked}</b><span>packages with blockers</span></div>
      <div class="stat"><b>${output.ready}</b><span>packages clear</span></div>
      ${cats.map((c) => `<div class="stat"><b>${output.totals[c]}</b><span>${esc(LABELS[c] ?? c)}</span></div>`).join("")}
    </div>
    <h3>Packages with React 19 blockers</h3>
    <table><thead><tr><th>Package</th>${cats.map((c) => `<th class="num">${esc(LABELS[c] ?? c)}</th>`).join("")}<th>react</th><th>@material-ui/core</th></tr></thead><tbody>
      ${blocked
        .map(
          (
            p,
          ) => `<tr class="clickable" data-pkg="${esc(p.name)}"><td class="mono">${esc(p.name)}</td>${cats.map((c) => `<td class="num">${p.counts[c] ?? ""}</td>`).join("")}
          <td class="mono">${esc(range(p, "react"))}</td><td class="mono">${esc(range(p, "@material-ui/core"))}</td></tr>`,
        )
        .join("")}
    </tbody></table>
    <div id="pkg"></div>
    <details style="margin-top:14px"><summary>${output.ready} packages with no blockers</summary>
      <p class="mono muted">${output.packages
        .filter((p) => p.total === 0)
        .map((p) => esc(p.name))
        .join(", ")}</p></details>`;
  el.querySelectorAll("tr[data-pkg]").forEach(
    (tr) =>
      (tr.onclick = () => {
        const p = output.packages.find((x) => x.name === tr.dataset.pkg);
        el.querySelector("#pkg").innerHTML = `<h3>${esc(p.name)}</h3><table><tbody>${p.files
          .flatMap((f) =>
            f.hits.map(
              (h) =>
                `<tr><td class="mono">${esc(f.file)}:${h.line}</td><td>${esc(LABELS[h.category])}</td><td class="mono muted">${esc(h.text)}</td></tr>`,
            ),
          )
          .join("")}</tbody></table>`;
      }),
  );
}
