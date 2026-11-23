// Type tests for `codemod/deps`. Checked with `pnpm --filter codemod typecheck`;
// nothing here runs.

import { dependencyGraph } from "codemod/deps";
import type { DependencyEdge, PackageNode } from "codemod/deps";
import { step } from "codemod/workflow";

type Equal<A, B> =
  (<T>() => T extends A ? 1 : 2) extends <T>() => T extends B ? 1 : 2 ? true : false;
function expectType<T extends true>(_: T) {}

export default async function main() {
  // Options are optional; nodes are recorded as `name@version`.
  const chains = await step(dependencyGraph((g) => g.chains({ to: g.nodes("moment") })));
  expectType<Equal<typeof chains, readonly (readonly string[])[]>>(true);

  const users = await step(
    dependencyGraph({ lockfiles: "package-lock.json" }, (g) =>
      g.chains({ to: g.nodes("moment") }).map((chain) => ({
        dir: chain[0].dir,
        local: chain[0].local,
        via: chain.slice(1, -1).map((node) => node.name),
      })),
    ),
  );
  expectType<Equal<(typeof users)[number]["dir"], string | null>>(true);
  expectType<Equal<(typeof users)[number]["local"], boolean>>(true);

  await step(
    dependencyGraph((g) => {
      const moment = g.node("moment", "2.29.4");
      expectType<Equal<typeof moment, PackageNode | undefined>>(true);
      // @ts-expect-error renamed to installedBy
      moment?.dependents();
      // @ts-expect-error folded into g.chains({ to })
      moment?.workspacesReaching();
      // @ts-expect-error renamed to dir
      const _workspace = moment?.workspace;
      // @ts-expect-error renamed to local
      const _isWorkspace = moment?.isWorkspace;
      // @ts-expect-error includeThirdParty is a boolean
      g.dependents("moment", { includeThirdParty: "yes" });
      const declared = g.dependents("moment", { kinds: "prod", includeThirdParty: true });
      expectType<Equal<typeof declared, DependencyEdge[]>>(true);
      return moment
        ?.installedBy({ kinds: "prod", includeThirdParty: false })
        .map((edge) => edge.from);
    }),
  );
}
