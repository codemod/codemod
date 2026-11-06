// Parity: JSSG commands awaited from a dynamic body, two of them concurrently,
// with invocation targets and a shell step reading the result.
import { dynamic, shell } from "../../../src/index.ts";
import { guarded, rename, replace } from "../jssg/transforms.ts";

const listing = shell({
  name: "listing",
  command: "find . -name '*.ts' -not -path './.git/*' | LC_ALL=C sort && cat a.ts",
});

export default dynamic(async () => {
  const first = await replace({ target: { include: ["a.ts"] } });
  const [second, third] = await Promise.all([
    guarded({ target: { exclude: ["sub/**"] } }),
    rename({ target: { include: ["sub/**"] } }),
  ]);
  const { stdout } = await listing();
  return { first, second, third, stdout };
});
