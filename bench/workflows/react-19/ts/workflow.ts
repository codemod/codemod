// react-19-migration-recipe (registry 0.1.5) as a TS workflow: the five
// nested codemods become five jssg steps run in order, importing the same
// transform files and copying each package's own step options.
import { jssg, sequence } from "@codemod.com/orchestration";
import reactDomRender from "../pkgs/replace-reactdom-render/scripts/codemod.port.ts";
import stringRef from "../pkgs/replace-string-ref/scripts/codemod.port.ts";
import actImport from "../pkgs/replace-act-import/scripts/codemod.port.ts";
import useFormState from "../pkgs/replace-use-form-state/scripts/codemod.port.ts";
import propTypes from "../pkgs/prop-types-typescript/scripts/codemod.port.ts";

const testFiles = {
  include: [
    "**/*.test.ts", "**/*.test.tsx", "**/*.spec.ts", "**/*.spec.tsx",
    "**/__tests__/**/*.ts", "**/__tests__/**/*.tsx", "**/tests/**/*.ts", "**/tests/**/*.tsx",
  ],
  exclude: [
    "**/node_modules/**", "**/dist/**", "**/build/**", "**/.next/**", "**/out/**",
    "**/coverage/**", "**/.git/**", "**/tests/**/expected.ts", "**/tests/**/expected.tsx",
  ],
};

export default sequence(
  jssg({ name: "replace-reactdom-render", language: "tsx", transform: reactDomRender })(),
  jssg({ name: "replace-string-ref", language: "tsx", transform: stringRef })(),
  jssg({ name: "replace-act-import", language: "tsx", ...testFiles, transform: actImport })(),
  jssg({ name: "replace-use-form-state", language: "tsx", semanticAnalysis: "file", transform: useFormState })(),
  jssg({ name: "prop-types-typescript", language: "tsx", transform: propTypes })(),
);
