import { ok as assert } from "assert";
import { parse } from "codemod:ast-grep";
import type JS from "@codemod.com/jssg-types/langs/javascript";
import type TS from "@codemod.com/jssg-types/langs/typescript";
import type TSX from "@codemod.com/jssg-types/langs/tsx";
import type { SgNode } from "@codemod.com/jssg-types/main";
import {
  findShadowingBinding,
  isRuntimeImportBinding,
} from "../src/javascript/exports/bindings.ts";

type Language = JS | TS | TSX;

function parseProgram<T extends Language>(lang: string, src: string) {
  const root = parse<T>(lang, src);
  return root.root();
}

function requireNode<T>(node: T | null | undefined, message: string): T {
  assert(node != null, message);
  return node;
}

function findIdentifierWithAncestorKind(
  program: SgNode<Language, "program">,
  name: string,
  ancestorKind: string,
) {
  return (
    program
      .findAll({
        rule: {
          kind: "identifier",
          pattern: name,
        },
      })
      .find((node) =>
        node.ancestors().some((ancestor) => String(ancestor.kind()) === ancestorKind),
      ) ?? null
  );
}

function testFindShadowingBindingReturnsLocalVariable() {
  const program = parseProgram(
    "tsx",
    [
      "import { Grid } from '@mui/material';",
      "function render() {",
      "  const Grid = localFactory();",
      "  return Grid;",
      "}",
    ].join("\n"),
  );

  const usage = program.find({
    rule: {
      kind: "identifier",
      pattern: "Grid",
      inside: {
        kind: "return_statement",
      },
    },
  });

  const resolvedUsage = requireNode(usage, "Should find local usage");
  const shadow = findShadowingBinding(resolvedUsage, "Grid");
  const resolvedShadow = requireNode(shadow, "Should find local shadowing binding");
  assert(resolvedShadow.text() === "Grid", "Shadowing binding should be the local identifier");
}

function testIsRuntimeImportBindingRejectsTypeOnlyUsage() {
  const program = parseProgram(
    "tsx",
    ["import type { Grid } from '@mui/material';", "type Gridish = Grid;"].join("\n"),
  );

  const usage = program.find({
    rule: {
      kind: "identifier",
      pattern: "Grid",
      inside: {
        kind: "import_specifier",
      },
    },
  });

  const resolvedUsage = requireNode(usage, "Should find type usage");
  assert(
    !isRuntimeImportBinding(resolvedUsage, {
      type: "named",
      name: "Grid",
      from: "@mui/material",
    }),
    "Type-only import usage should not be treated as runtime",
  );
}

function testIsRuntimeImportBindingRejectsShadowedUsage() {
  const program = parseProgram(
    "tsx",
    [
      "import { Grid } from '@mui/material';",
      "function render() {",
      "  const Grid = localFactory();",
      "  return Grid;",
      "}",
    ].join("\n"),
  );

  const usage = program.find({
    rule: {
      kind: "identifier",
      pattern: "Grid",
      inside: {
        kind: "return_statement",
      },
    },
  });

  const resolvedUsage = requireNode(usage, "Should find local usage");
  assert(
    !isRuntimeImportBinding(resolvedUsage, {
      type: "named",
      name: "Grid",
      from: "@mui/material",
    }),
    "Shadowed local usage should not be treated as runtime import usage",
  );
}

function testIsRuntimeImportBindingAcceptsUnshadowedRuntimeUsage() {
  const program = parseProgram(
    "tsx",
    [
      "import { Grid as MuiGrid } from '@mui/material';",
      "function render() {",
      "  return MuiGrid;",
      "}",
    ].join("\n"),
  );

  const usage = program.find({
    rule: {
      kind: "identifier",
      pattern: "MuiGrid",
      inside: {
        kind: "return_statement",
      },
    },
  });

  const resolvedUsage = requireNode(usage, "Should find runtime usage");
  assert(
    isRuntimeImportBinding(resolvedUsage, {
      type: "named",
      name: "Grid",
      from: "@mui/material",
    }),
    "Unshadowed runtime usage should be accepted",
  );
}

function testFindShadowingBindingHandlesHoistedVarFromNestedBlock() {
  const program = parseProgram(
    "javascript",
    [
      "import { Grid } from '@mui/material';",
      "function render() {",
      "  if (ready) {",
      "    var Grid = makeLocalGrid();",
      "  }",
      "  return Grid;",
      "}",
    ].join("\n"),
  );

  const usage = program.find({
    rule: {
      kind: "identifier",
      pattern: "Grid",
      inside: {
        kind: "return_statement",
      },
    },
  });

  const resolvedUsage = requireNode(usage, "Should find hoisted var usage");
  const shadow = findShadowingBinding(resolvedUsage, "Grid");
  const resolvedShadow = requireNode(shadow, "Should find hoisted var declaration");
  assert(resolvedShadow.text() === "Grid", "Hoisted var shadow should resolve to Grid");
  assert(
    !isRuntimeImportBinding(resolvedUsage, {
      type: "named",
      name: "Grid",
      from: "@mui/material",
    }),
    "Hoisted var should shadow the imported binding",
  );
}

function testFindShadowingBindingHandlesDestructuredParameters() {
  const program = parseProgram(
    "javascript",
    [
      "import { Grid } from '@mui/material';",
      "function render({ Grid }) {",
      "  return Grid;",
      "}",
    ].join("\n"),
  );

  const usage = program.find({
    rule: {
      kind: "identifier",
      pattern: "Grid",
      inside: {
        kind: "return_statement",
      },
    },
  });

  const resolvedUsage = requireNode(usage, "Should find destructured parameter usage");
  const shadow = findShadowingBinding(resolvedUsage, "Grid");
  const resolvedShadow = requireNode(shadow, "Should treat destructured parameter as shadowing");
  assert(resolvedShadow.text() === "Grid", "Destructured parameter shadow should resolve to Grid");
  assert(
    !isRuntimeImportBinding(resolvedUsage, {
      type: "named",
      name: "Grid",
      from: "@mui/material",
    }),
    "Destructured parameter should shadow the imported binding",
  );
}

function testDefaultParameterInitializerUsageIsNotTreatedAsBinding() {
  const program = parseProgram(
    "javascript",
    [
      "import { Grid } from '@mui/material';",
      "function render(local = Grid) {",
      "  return local;",
      "}",
    ].join("\n"),
  );

  const usage = program.find({
    rule: {
      kind: "identifier",
      pattern: "Grid",
      inside: {
        kind: "assignment_pattern",
      },
    },
  });

  const resolvedUsage = requireNode(usage, "Should find default-parameter initializer usage");
  assert(
    findShadowingBinding(resolvedUsage, "Grid") === null,
    "Default-parameter initializer usage should not be treated as a binding",
  );
  assert(
    isRuntimeImportBinding(resolvedUsage, {
      type: "named",
      name: "Grid",
      from: "@mui/material",
    }),
    "Default-parameter initializer usage should still resolve as the imported runtime binding",
  );
}

function testFindShadowingBindingHandlesCatchParameters() {
  const program = parseProgram(
    "javascript",
    [
      "import { Grid } from '@mui/material';",
      "try {",
      "  run();",
      "} catch (Grid) {",
      "  console.log(Grid);",
      "}",
    ].join("\n"),
  );

  const usage = findIdentifierWithAncestorKind(program, "Grid", "call_expression");

  const resolvedUsage = requireNode(usage, "Should find catch parameter usage");
  const shadow = findShadowingBinding(resolvedUsage, "Grid");
  const resolvedShadow = requireNode(shadow, "Should treat catch parameter as shadowing");
  assert(resolvedShadow.text() === "Grid", "Catch parameter shadow should resolve to Grid");
  assert(
    !isRuntimeImportBinding(resolvedUsage, {
      type: "named",
      name: "Grid",
      from: "@mui/material",
    }),
    "Catch parameter should shadow the imported binding",
  );
}

function testFindShadowingBindingHandlesFunctionDeclarationNames() {
  const program = parseProgram(
    "javascript",
    [
      "import { Grid } from '@mui/material';",
      "function render() {",
      "  function Grid() {}",
      "  return Grid;",
      "}",
    ].join("\n"),
  );

  const usage = program.find({
    rule: {
      kind: "identifier",
      pattern: "Grid",
      inside: {
        kind: "return_statement",
      },
    },
  });

  const resolvedUsage = requireNode(usage, "Should find function declaration usage");
  const shadow = findShadowingBinding(resolvedUsage, "Grid");
  const resolvedShadow = requireNode(shadow, "Should treat function declaration name as shadowing");
  assert(resolvedShadow.text() === "Grid", "Function declaration shadow should resolve to Grid");
  assert(
    !isRuntimeImportBinding(resolvedUsage, {
      type: "named",
      name: "Grid",
      from: "@mui/material",
    }),
    "Function declaration should shadow the imported binding",
  );
}

function testFindShadowingBindingHandlesClassDeclarationNames() {
  const program = parseProgram(
    "javascript",
    [
      "import { Grid } from '@mui/material';",
      "function render() {",
      "  class Grid {}",
      "  return Grid;",
      "}",
    ].join("\n"),
  );

  const usage = program.find({
    rule: {
      kind: "identifier",
      pattern: "Grid",
      inside: {
        kind: "return_statement",
      },
    },
  });

  const resolvedUsage = requireNode(usage, "Should find class declaration usage");
  const shadow = findShadowingBinding(resolvedUsage, "Grid");
  const resolvedShadow = requireNode(shadow, "Should treat class declaration name as shadowing");
  assert(resolvedShadow.text() === "Grid", "Class declaration shadow should resolve to Grid");
  assert(
    !isRuntimeImportBinding(resolvedUsage, {
      type: "named",
      name: "Grid",
      from: "@mui/material",
    }),
    "Class declaration should shadow the imported binding",
  );
}

function testImportedBindingIsNotTreatedAsShadow() {
  const program = parseProgram(
    "javascript",
    "import { makeStyles } from '@material-ui/core/styles';\nconst useStyles = makeStyles();\n",
  );

  const usage = program.find({
    rule: {
      kind: "identifier",
      pattern: "makeStyles",
      inside: {
        kind: "call_expression",
      },
    },
  });

  const resolvedUsage = requireNode(usage, "Should find imported helper usage");
  assert(
    findShadowingBinding(resolvedUsage, "makeStyles") === null,
    "Import definitions should not be treated as shadowing bindings",
  );
  assert(
    isRuntimeImportBinding(resolvedUsage, {
      type: "named",
      name: "makeStyles",
      from: "@material-ui/core/styles",
    }),
    "Imported helper usage should still resolve as a runtime import binding",
  );
}

function testIsRuntimeImportBindingAcceptsJsxDefaultImportUsage() {
  const program = parseProgram(
    "tsx",
    [
      "import Grid from '@mui/material/Grid';",
      "function Example() {",
      "  return <Grid xs={12} />;",
      "}",
    ].join("\n"),
  );

  const usage = program.find({
    rule: {
      kind: "identifier",
      pattern: "Grid",
      inside: {
        kind: "jsx_self_closing_element",
      },
    },
  });

  const resolvedUsage = requireNode(usage, "Should find JSX Grid usage");
  assert(
    isRuntimeImportBinding(resolvedUsage, {
      type: "default",
      from: "@mui/material/Grid",
    }),
    "JSX tag identifier should resolve to the runtime default import binding",
  );
}

function testIsRuntimeImportBindingRejectsJsxShadowedUsage() {
  const program = parseProgram(
    "tsx",
    [
      "import Grid from '@mui/material/Grid';",
      "function Example() {",
      "  const Grid = Wrapper;",
      "  return <Grid xs={12} />;",
      "}",
    ].join("\n"),
  );

  const usage = program.find({
    rule: {
      kind: "identifier",
      pattern: "Grid",
      inside: {
        kind: "jsx_self_closing_element",
      },
    },
  });

  const resolvedUsage = requireNode(usage, "Should find shadowed JSX Grid usage");
  assert(
    !isRuntimeImportBinding(resolvedUsage, {
      type: "default",
      from: "@mui/material/Grid",
    }),
    "Shadowed JSX tag identifier should not resolve to the runtime import binding",
  );
}

function testIsRuntimeImportBindingAcceptsJsxNamedAliasUsage() {
  const program = parseProgram(
    "tsx",
    [
      "import { Grid as JoyGrid } from '@mui/joy/Grid';",
      "function Example() {",
      "  return <JoyGrid xs={12} />;",
      "}",
    ].join("\n"),
  );

  const usage = program.find({
    rule: {
      kind: "identifier",
      pattern: "JoyGrid",
      inside: {
        kind: "jsx_self_closing_element",
      },
    },
  });

  const resolvedUsage = requireNode(usage, "Should find JSX JoyGrid usage");
  assert(
    isRuntimeImportBinding(resolvedUsage, {
      type: "named",
      name: "Grid",
      from: "@mui/joy/Grid",
    }),
    "JSX tag identifier should resolve to a named aliased runtime import binding",
  );
}

testFindShadowingBindingReturnsLocalVariable();
testIsRuntimeImportBindingRejectsTypeOnlyUsage();
testIsRuntimeImportBindingRejectsShadowedUsage();
testIsRuntimeImportBindingAcceptsUnshadowedRuntimeUsage();
testFindShadowingBindingHandlesHoistedVarFromNestedBlock();
testFindShadowingBindingHandlesDestructuredParameters();
testDefaultParameterInitializerUsageIsNotTreatedAsBinding();
testFindShadowingBindingHandlesCatchParameters();
testFindShadowingBindingHandlesFunctionDeclarationNames();
testFindShadowingBindingHandlesClassDeclarationNames();
testImportedBindingIsNotTreatedAsShadow();
testIsRuntimeImportBindingAcceptsJsxDefaultImportUsage();
testIsRuntimeImportBindingRejectsJsxShadowedUsage();
testIsRuntimeImportBindingAcceptsJsxNamedAliasUsage();
