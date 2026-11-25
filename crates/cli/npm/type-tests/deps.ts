// Type tests for `codemod/deps`. Checked with `pnpm --filter codemod typecheck`;
// nothing here runs.

import { dependencies, install, semver } from "codemod/deps";
import type {
  Declaration,
  DependencyEdge,
  Dependencies,
  Edit,
  InstallResult,
  LocalNode,
  Package,
  PackageNode,
} from "codemod/deps";
import { step } from "codemod/workflow";

type Equal<A, B> =
  (<T>() => T extends A ? 1 : 2) extends <T>() => T extends B ? 1 : 2 ? true : false;
function expectType<T extends true>(_: T) {}

export default async function main() {
  // Manifest reads; packages, declarations and nodes are recorded as their JSON.
  const users = await step(() =>
    dependencies()
      .declarations("moment")
      .map((d) => d.package.dir),
  );
  expectType<Equal<typeof users, readonly string[]>>(true);

  const packages = await step(() => dependencies().packages());
  expectType<Equal<typeof packages, readonly string[]>>(true);

  const chains = await step(() => dependencies().chains({ to: "moment" }));
  expectType<Equal<typeof chains, readonly (readonly string[])[]>>(true);

  const declared = await step(() => dependencies().declarations("react")[0]);
  expectType<Equal<(typeof declared)["resolved"], string | null>>(true);

  const edits = await step(() => dependencies().bump("react", "^19.0.0"));
  expectType<
    Equal<
      (typeof edits)[number]["kind"],
      "prod" | "dev" | "peer" | "optional" | "build" | "catalog"
    >
  >(true);

  const installed = await step(install());
  expectType<Equal<typeof installed, { readonly [K in keyof InstallResult]: InstallResult[K] }>>(
    true,
  );
  await step(install({ dir: "packages/web", lockfileOnly: false }));
  // @ts-expect-error lockfileOnly is a boolean
  install({ lockfileOnly: "yes" });

  await step(() => {
    const deps: Dependencies = dependencies({ lockfiles: "**/pnpm-lock.yaml" });
    const pkg = deps.packageOf("apps/web/src/page.tsx");
    expectType<Equal<typeof pkg, Package | undefined>>(true);
    const declaration = pkg?.declaration("next");
    expectType<Equal<typeof declaration, Declaration | undefined>>(true);
    const local = declaration?.local;
    expectType<Equal<typeof local, Package | null | undefined>>(true);
    expectType<Equal<typeof deps.packageManager, import("codemod/deps").PackageManager | null>>(
      true,
    );

    // Edits.
    const added: readonly Edit[] = deps.add("vitest", "^3.0.0", {
      kind: "dev",
      to: "packages/web",
    });
    deps.remove("chalk", { in: ["packages/a", "packages/b"], kinds: ["prod", "dev"] });
    deps.rename("react-router-dom", "react-router", { range: "^7.0.0" });
    deps.bump("lodash", (d) => (semver.major(d.resolved ?? "") === 3 ? "^4.17.21" : undefined));
    // @ts-expect-error kinds are named
    deps.add("x", "1", { kind: "devDependencies" });
    // @ts-expect-error a range is a string or a function
    deps.bump("x", 1);
    void added;

    // The graph: `local` narrows `dir`.
    const node = deps.node("moment", "2.29.4");
    expectType<Equal<typeof node, PackageNode | undefined>>(true);
    const show = (n: PackageNode) => (n.local ? n.dir : n.toString());
    expectType<Equal<ReturnType<typeof show>, string>>(true);
    const roots: LocalNode[] = deps.workspaces();
    expectType<Equal<(typeof roots)[number]["dir"], string>>(true);
    const dependents = deps.dependents("moment", { kinds: "prod", includeThirdParty: true });
    expectType<Equal<typeof dependents, DependencyEdge[]>>(true);
    // @ts-expect-error includeThirdParty is a boolean
    deps.dependents("moment", { includeThirdParty: "yes" });
    deps.chains({ from: pkg ?? "web", to: ["moment", ...deps.nodes("dayjs")] });
    const versions: string[] = deps.installed("react");

    // Semver helpers.
    const min: string | null = semver.minVersion("^18.2.0");
    const major: number | null = semver.major("^18.2.0");
    const coerced: string | null = semver.coerce("pnpm@10.1.3");
    const highest: string | null = semver.maxSatisfying(versions, "^18");
    const readable: boolean = semver.validRange("latest") || semver.gte("1.0.0", "1.0.0");
    return { min, major, coerced, highest, readable, needs: deps.needsInstall() };
  });
}
