import { parse, type Edit } from "codemod/ast-grep";
import type Xml from "codemod/ast-grep/langs/xml";
import { forEachFile, step } from "codemod/workflow";

// The files the transform runs on.
const files = {
  include: ["**/*.{xml,csproj,props,targets,config,resx,xaml}"],
  exclude: ["**/node_modules/**", "**/dist/**", "**/build/**"],
};

export default async function main() {
  await step("Replace TargetFrameworkVersion", forEachFile(files, (file) => {
    const sgRoot = parse<Xml>(file, { language: "xml" });
    const rootNode = sgRoot.root();

    const legacyFrameworkNodes = rootNode.findAll({
      rule: {
        pattern: "<TargetFrameworkVersion>$VALUE</TargetFrameworkVersion>",
      },
    });

    const edits = legacyFrameworkNodes
      .map((node) => {
        const rawValue = node.getMatch("VALUE")?.text()?.trim();
        if (!rawValue) return null;
        return node.replace(`<TargetFramework>${rawValue}</TargetFramework>`);
      })
      .filter(Boolean) as Edit[];

    if (edits.length === 0) return;
    file.write(rootNode.commitEdits(edits));
  }));
}
