// Results view for react-19-act-migration: before and after counts, the diff, and what was committed.
export function render(output, el, { esc }) {
  const manual = output.after.manual.length ? output.after.manual : output.before.manual;
  el.innerHTML = `
    <div class="stats">
      <div class="stat"><b>${output.before.mechanical}</b><span>act imports before</span></div>
      <div class="stat"><b>${output.after.mechanical}</b><span>after</span></div>
      <div class="stat"><b>${output.changed.length}</b><span>files changed</span></div>
      <div class="stat"><b>${manual.length}</b><span>left for a person</span></div>
    </div>
    <p>${output.commit ? `Committed <code>${esc(output.commit)}</code> on <code>${esc(output.branch)}</code> from <code>${esc(output.base)}</code>.` : `Nothing to change on <code>${esc(output.base)}</code>.`}</p>
    <h3>Diff</h3>
    <table><thead><tr><th>File</th><th class="num">+</th><th class="num">−</th></tr></thead><tbody>
      ${output.diff.map((d) => `<tr><td class="mono">${esc(d.file)}</td><td class="num s-succeeded">${d.added}</td><td class="num s-failed">${d.removed}</td></tr>`).join("")}
    </tbody></table>
    ${manual.length ? `<h3>Not mechanical</h3><table><tbody>${manual.map((m) => `<tr><td class="mono">${esc(m.file)}</td><td class="mono">${esc(m.text)}</td></tr>`).join("")}</tbody></table>` : ""}`;
}
