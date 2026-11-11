import type { SgRoot } from "codemod:ast-grep";
import type TSX from "codemod:ast-grep/langs/tsx";
import type { Finding } from "./finding.ts";
import { detect as clickOnNonInteractive } from "./click-on-non-interactive.ts";
import { detect as derivedStateEffect } from "./derived-state-effect.ts";
import { detect as effectMissingCleanup } from "./effect-missing-cleanup.ts";
import { detect as exhaustiveDepsSuppressed } from "./exhaustive-deps-suppressed.ts";
import { detect as imgMissingAlt } from "./img-missing-alt.ts";
import { detect as indexAsKey } from "./index-as-key.ts";
import { detect as nestedComponentDefinition } from "./nested-component-definition.ts";
import { detect as oversizedComponent } from "./oversized-component.ts";
import { detect as unstableContextValue } from "./unstable-context-value.ts";

export type { Finding } from "./finding.ts";

const DETECTORS = [
  clickOnNonInteractive,
  derivedStateEffect,
  effectMissingCleanup,
  exhaustiveDepsSuppressed,
  imgMissingAlt,
  indexAsKey,
  nestedComponentDefinition,
  oversizedComponent,
  unstableContextValue,
];

/** Every detector over one parsed file; one parse instead of nine. */
export async function detectAll(root: SgRoot<TSX>, options: { params?: Record<string, unknown> }) {
  const found: Finding[] = [];
  for (const detect of DETECTORS) await detect(root, options, found);
  return found;
}
