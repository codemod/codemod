export function register(fn: () => string) {
  return fn;
}
export function emit(event: string) {}
export function on(event: string, fn: () => void) {}

export function save() {
  emit("saved");
}
on("saved", () => {});
