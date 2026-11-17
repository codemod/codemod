import type { Codemod, SgNode } from "codemod:ast-grep";
import type TSX from "codemod:ast-grep/langs/tsx";
import { useMetricAtom } from "codemod:metrics";

const findings = useMetricAtom("img-missing-alt");

const codemod: Codemod<TSX> = async (root, options) => {
  const report = (node: SgNode<TSX>, dims: Record<string, string>) => {
    findings.increment(dims);
    if (options.params?.locations) {
      console.log(`FINDING\timg-missing-alt\t${root.filename()}:${node.range().start.line + 1}\t${JSON.stringify(dims)}`);
    }
  };

  const program = root.root();
  const importsNextImage = program.has({
    rule: { kind: "import_statement", has: { kind: "string", regex: "^[\"']next/(legacy/)?image[\"']$" } },
  });
  const elementName = importsNextImage ? "^(img|Image)$" : "^img$";

  const images = program.findAll({
    rule: {
      any: [{ kind: "jsx_opening_element" }, { kind: "jsx_self_closing_element" }],
      has: { field: "name", regex: elementName },
    },
  });

  for (const image of images) {
    const direct = { stopBy: "neighbor" as const, kind: image.is("jsx_opening_element") ? ("jsx_opening_element" as const) : ("jsx_self_closing_element" as const) };
    const hasAlt = image.has({ rule: { kind: "jsx_attribute", has: { kind: "property_identifier", regex: "^alt$" }, inside: direct } });
    const hasSpread = image.has({ rule: { kind: "jsx_expression", has: { kind: "spread_element" }, inside: direct } });
    if (!hasAlt && !hasSpread) report(image, { element: image.field("name")?.text() ?? "img" });
  }
  return null;
};

export default codemod;
