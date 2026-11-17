// Dev's jssg-audit detection pack as a TS workflow: the 14 detections run
// as one parallel group. Each returns its findings as typed output instead
// of bumping a metric (see ported/metrics-shim.ts); the detection code is
// otherwise the published codemod. Names are literals and transforms only
// use imports, as the build step requires (see bundle/build.ts).
import { jssg, parallel } from "@codemod.com/orchestration";
import { drain } from "./ported/metrics-shim.ts";
import d0 from "./ported/click-on-non-interactive.detect.ts";
import d1 from "./ported/db-query-in-loop.detect.ts";
import d2 from "./ported/derived-state-effect.detect.ts";
import d3 from "./ported/effect-missing-cleanup.detect.ts";
import d4 from "./ported/exhaustive-deps-suppressed.detect.ts";
import d5 from "./ported/floating-async-iteration.detect.ts";
import d6 from "./ported/img-missing-alt.detect.ts";
import d7 from "./ported/index-as-key.detect.ts";
import d8 from "./ported/nested-component-definition.detect.ts";
import d9 from "./ported/oversized-component.detect.ts";
import d10 from "./ported/route-entry-directive.detect.ts";
import d11 from "./ported/server-action-without-auth.detect.ts";
import d12 from "./ported/unsanitized-inner-html.detect.ts";
import d13 from "./ported/unstable-context-value.detect.ts";

const files = {
  include: ["**/*.tsx", "**/*.jsx", "**/*.ts", "**/*.js"],
  exclude: ["**/node_modules/**", "**/dist/**", "**/build/**", "**/.next/**"],
};

export default parallel(
  jssg({ name: "click-on-non-interactive", language: "tsx", ...files, async transform(root, options) {
    await d0(root, options);
    const findings = drain();
    return findings.length ? { output: findings } : null;
  } })(),
  jssg({ name: "db-query-in-loop", language: "tsx", ...files, async transform(root, options) {
    await d1(root, options);
    const findings = drain();
    return findings.length ? { output: findings } : null;
  } })(),
  jssg({ name: "derived-state-effect", language: "tsx", ...files, async transform(root, options) {
    await d2(root, options);
    const findings = drain();
    return findings.length ? { output: findings } : null;
  } })(),
  jssg({ name: "effect-missing-cleanup", language: "tsx", ...files, async transform(root, options) {
    await d3(root, options);
    const findings = drain();
    return findings.length ? { output: findings } : null;
  } })(),
  jssg({ name: "exhaustive-deps-suppressed", language: "tsx", ...files, async transform(root, options) {
    await d4(root, options);
    const findings = drain();
    return findings.length ? { output: findings } : null;
  } })(),
  jssg({ name: "floating-async-iteration", language: "tsx", ...files, async transform(root, options) {
    await d5(root, options);
    const findings = drain();
    return findings.length ? { output: findings } : null;
  } })(),
  jssg({ name: "img-missing-alt", language: "tsx", ...files, async transform(root, options) {
    await d6(root, options);
    const findings = drain();
    return findings.length ? { output: findings } : null;
  } })(),
  jssg({ name: "index-as-key", language: "tsx", ...files, async transform(root, options) {
    await d7(root, options);
    const findings = drain();
    return findings.length ? { output: findings } : null;
  } })(),
  jssg({ name: "nested-component-definition", language: "tsx", ...files, async transform(root, options) {
    await d8(root, options);
    const findings = drain();
    return findings.length ? { output: findings } : null;
  } })(),
  jssg({ name: "oversized-component", language: "tsx", ...files, async transform(root, options) {
    await d9(root, options);
    const findings = drain();
    return findings.length ? { output: findings } : null;
  } })(),
  jssg({ name: "route-entry-directive", language: "tsx", ...files, async transform(root, options) {
    await d10(root, options);
    const findings = drain();
    return findings.length ? { output: findings } : null;
  } })(),
  jssg({ name: "server-action-without-auth", language: "tsx", ...files, async transform(root, options) {
    await d11(root, options);
    const findings = drain();
    return findings.length ? { output: findings } : null;
  } })(),
  jssg({ name: "unsanitized-inner-html", language: "tsx", ...files, async transform(root, options) {
    await d12(root, options);
    const findings = drain();
    return findings.length ? { output: findings } : null;
  } })(),
  jssg({ name: "unstable-context-value", language: "tsx", ...files, async transform(root, options) {
    await d13(root, options);
    const findings = drain();
    return findings.length ? { output: findings } : null;
  } })(),
);
