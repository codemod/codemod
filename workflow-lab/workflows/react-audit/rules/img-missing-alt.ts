// Generated from the jssg React/Next.js audit (devagrawal09/nextjs-codemods):
// the same detector, reporting findings as step output instead of metrics.
import type { SgNode, SgRoot } from "codemod:ast-grep";
import type { Finding } from "./finding.ts";
import type TSX from "codemod:ast-grep/langs/tsx";

// Demo apps, examples and OG images rendered by Satori aren't shipped UI.
const EXAMPLE_PATH =
  /(^|\/)(examples?|demos?|playgrounds?|sandbox(es)?)\/|[-_.](demo|example|dummy)s?[-_./]/i;
const OG_IMAGE = /from\s+["'](next\/og|@vercel\/og|satori)["']|new ImageResponse\(/;

export const detect = async (
  root: SgRoot<TSX>,
  options: { params?: Record<string, unknown> },
  found: Finding[],
) => {
  // --param includeExamples=1 keeps example and demo code, which is sometimes copied into apps.
  if (
    !options.params?.includeExamples &&
    (EXAMPLE_PATH.test(root.filename().replace(/\\/g, "/")) || OG_IMAGE.test(root.root().text()))
  )
    return null;
  const report = (node: SgNode<TSX>, dims: Record<string, string>) => {
    found.push({ rule: "img-missing-alt", line: node.range().start.line + 1, dims });
  };

  const program = root.root();
  const importsNextImage = program.has({
    rule: {
      kind: "import_statement",
      has: { kind: "string", regex: "^[\"']next/(legacy/)?image[\"']$" },
    },
  });
  const elementName = importsNextImage ? "^(img|Image)$" : "^img$";

  const images = program.findAll({
    rule: {
      any: [{ kind: "jsx_opening_element" }, { kind: "jsx_self_closing_element" }],
      has: { field: "name", regex: elementName },
    },
  });

  for (const image of images) {
    const direct = {
      stopBy: "neighbor" as const,
      kind: image.is("jsx_opening_element")
        ? ("jsx_opening_element" as const)
        : ("jsx_self_closing_element" as const),
    };
    const hasAlt = image.has({
      rule: {
        kind: "jsx_attribute",
        has: { kind: "property_identifier", regex: "^alt$" },
        inside: direct,
      },
    });
    const hasSpread = image.has({
      rule: { kind: "jsx_expression", has: { kind: "spread_element" }, inside: direct },
    });
    // Hidden or presentational images are skipped by screen readers anyway.
    const hidden = image.has({
      rule: {
        kind: "jsx_attribute",
        inside: direct,
        any: [
          { regex: '^aria-hidden(=(\\{true\\}|"true"))?$' },
          { regex: "^role=[\"'](presentation|none)[\"']$" },
        ],
      },
    });
    if (!hasAlt && !hasSpread && !hidden)
      report(image, { element: image.field("name")?.text() ?? "img" });
  }
  return null;
};
