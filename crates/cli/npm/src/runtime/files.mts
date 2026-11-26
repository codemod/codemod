import { current } from "./context.mts";

function target() {
  const api = current("files").files;
  if (!api) throw new Error("files is only available in workflow callbacks");
  return api;
}

export const files = Object.freeze({
  read: (path: string) => target().read(path),
  exists: (path: string) => target().exists(path),
  list: (glob: string) => target().list(glob),
  create: (path: string, text: string) => target().create(path, text),
});
