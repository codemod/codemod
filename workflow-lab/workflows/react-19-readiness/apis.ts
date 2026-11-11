import type { SgNode, SgRoot } from "codemod:ast-grep";
import type TSX from "codemod:ast-grep/langs/tsx";

/** React 19 removed or changed these (react.dev/blog/2024/04/25/react-19-upgrade-guide). */
export type Category =
  | "legacy-root-api"
  | "find-dom-node"
  | "test-utils"
  | "string-ref"
  | "default-props"
  | "prop-types"
  | "legacy-context"
  | "create-factory";

export interface Hit {
  category: Category;
  line: number;
  text: string;
}

const line = (node: SgNode<TSX>) => node.range().start.line + 1;
const snippet = (node: SgNode<TSX>) => node.text().split("\n")[0]!.slice(0, 100);

export function scan(root: SgRoot<TSX>): Hit[] {
  const program = root.root();
  const hits: Hit[] = [];
  const add = (category: Category, nodes: SgNode<TSX>[]) =>
    nodes.forEach((n) => hits.push({ category, line: line(n), text: snippet(n) }));

  add(
    "legacy-root-api",
    program.findAll({
      rule: {
        any: [
          { pattern: "ReactDOM.render($$$)" },
          { pattern: "ReactDOM.hydrate($$$)" },
          { pattern: "ReactDOM.unmountComponentAtNode($$$)" },
          {
            kind: "import_specifier",
            has: { field: "name", regex: "^(render|hydrate|unmountComponentAtNode)$" },
            inside: {
              stopBy: "end",
              kind: "import_statement",
              has: { field: "source", regex: "^['\"]react-dom['\"]$" },
            },
          },
        ],
      },
    }),
  );
  add("find-dom-node", program.findAll({ rule: { pattern: "$$$.findDOMNode($$$)" } }));
  add("find-dom-node", program.findAll({ rule: { pattern: "findDOMNode($$$)" } }));
  add(
    "test-utils",
    program.findAll({
      rule: {
        kind: "import_statement",
        has: { field: "source", regex: "^['\"]react-dom/test-utils['\"]$" },
      },
    }),
  );
  add(
    "string-ref",
    program.findAll({
      rule: {
        kind: "jsx_attribute",
        all: [
          { has: { kind: "property_identifier", regex: "^ref$" } },
          { has: { kind: "string" } },
        ],
      },
    }),
  );
  add("default-props", program.findAll({ rule: { pattern: "$C.defaultProps = $$$" } }));
  add(
    "prop-types",
    program.findAll({
      rule: { any: [{ pattern: "$C.propTypes = $$$" }, { pattern: "React.PropTypes" }] },
    }),
  );
  add(
    "legacy-context",
    program.findAll({
      rule: {
        any: [
          { pattern: "$C.contextTypes = $$$" },
          { pattern: "$C.childContextTypes = $$$" },
          { kind: "method_definition", has: { field: "name", regex: "^getChildContext$" } },
        ],
      },
    }),
  );
  add("create-factory", program.findAll({ rule: { pattern: "React.createFactory($$$)" } }));
  return hits;
}

const WATCHED = [
  "react",
  "react-dom",
  "@types/react",
  "@types/react-dom",
  "@testing-library/react",
  "@material-ui/core",
  "@mui/material",
];

/** The React-related dependency ranges one package.json declares. */
export function reactDeps(text: string): Record<string, string> {
  let manifest: Record<string, Record<string, string> | undefined>;
  try {
    manifest = JSON.parse(text);
  } catch {
    return {};
  }
  const out: Record<string, string> = {};
  for (const field of ["dependencies", "devDependencies", "peerDependencies"]) {
    for (const [name, range] of Object.entries(manifest[field] ?? {})) {
      if (WATCHED.includes(name)) out[name] = range;
    }
  }
  return out;
}
