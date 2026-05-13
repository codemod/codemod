import type { Codemod, Edit } from "codemod:ast-grep";
import type Xml from "codemod:ast-grep/langs/xml";

const codemod: Codemod<Xml> = async (root) => {
  const rootNode = root.root();

  const legacyFrameworkNodes = rootNode.findAll({
    rule: {
      pattern: "<TargetFrameworkVersion>$VALUE</TargetFrameworkVersion>"
    }
  });

  const edits = legacyFrameworkNodes
    .map(node => {
      const rawValue = node.getMatch("VALUE")?.text()?.trim();
      if (!rawValue) {
        return null;
      }

      const tfm = rawValue.replace(
        /^v(?<major>\d)\.(?<minor>\d)(?:\.(?<patch>\d))?$/,
        "net$<major>$<minor>$<patch>"
      );

      return node.replace(`<TargetFramework>${tfm}</TargetFramework>`);
    })
    .filter(Boolean) as Edit[];

  if (edits.length === 0) {
    return null;
  }

  return rootNode.commitEdits(edits);
};

export default codemod;
