import type { Codemod, SgNode } from "codemod:ast-grep";
import type TSX from "codemod:ast-grep/langs/tsx";
import { useMetricAtom } from "codemod:metrics";

const findings = useMetricAtom("server-action-without-auth");

// Calls that read the caller's identity or check access. Anything matching counts as a check.
const AUTH_CALL = /auth|session|user|permission|access|verify|guard|protect|role|owner|member|token|signature|apikey|admin|require|assert|ensure/i;
// next-safe-action style clients whose name says they run auth middleware.
const AUTHED_CLIENT = /auth|protected|user|org|team|workspace|admin|member|account/i;

const isUseServerFile = (program: SgNode<TSX>) => {
  const first = program.children().find((c) => c.isNamed() && c.kind() !== "comment");
  return first?.kind() === "expression_statement" && /^["']use server["']$/.test(first.children().find((c) => c.isNamed())?.text() ?? "");
};

const checksAuth = (fn: SgNode<TSX>) =>
  fn.has({
    rule: {
      kind: "call_expression",
      has: { field: "function", regex: AUTH_CALL.source },
    },
  }) || fn.has({ rule: { kind: "identifier", regex: "^(session|user|userId|currentUser)$", inside: { kind: "variable_declarator", field: "name" } } });

// Actions that only bust the Next.js cache are low impact; everything else can read or write data.
const effectOf = (fn: SgNode<TSX>) => {
  const callees = fn.findAll({ rule: { kind: "call_expression" } }).map((c) => c.field("function")?.text() ?? "");
  return callees.length > 0 && callees.every((c) => /^(revalidatePath|revalidateTag|unstable_cache)$/.test(c)) ? "cache-revalidation" : "reads-or-writes";
};

const codemod: Codemod<TSX> = async (root, options) => {
  const report = (node: SgNode<TSX>, dims: Record<string, string>) => {
    findings.increment(dims);
    if (options.params?.locations) {
      console.log(`FINDING\tserver-action-without-auth\t${root.filename()}:${node.range().start.line + 1}\t${JSON.stringify(dims)}`);
    }
  };

  const program = root.root();
  if (!isUseServerFile(program)) return null;

  for (const exported of program.findAll({ rule: { kind: "export_statement" } })) {
    const declaration = exported.field("declaration");
    if (!declaration) continue;

    if (declaration.kind() === "function_declaration") {
      if (/^async\b/.test(declaration.text()) && !checksAuth(declaration)) report(declaration, { kind: "async-function", effect: effectOf(declaration) });
      continue;
    }

    for (const declarator of declaration.findAll({ rule: { kind: "variable_declarator" } })) {
      const value = declarator.field("value");
      if (!value) continue;
      if (value.kind() === "arrow_function" || value.kind() === "function_expression") {
        if (/^async\b/.test(value.text()) && !checksAuth(value)) report(declarator, { kind: "async-function", effect: effectOf(value) });
        continue;
      }
      // actionClient.schema(...).action(async () => ...): judge by the client the chain starts from.
      if (value.kind() === "call_expression" && /\.action$/.test(value.field("function")?.text() ?? "")) {
        const client = value.text().match(/^[A-Za-z_$][\w$]*/)?.[0] ?? "";
        if (!AUTHED_CLIENT.test(client) && !checksAuth(value)) report(declarator, { kind: "action-client", effect: effectOf(value) });
      }
    }
  }
  return null;
};

export default codemod;
