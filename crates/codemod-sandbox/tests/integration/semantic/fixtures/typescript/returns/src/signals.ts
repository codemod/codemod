// Shaped like Solid's createSignal: overloads, a helper that returns a bound
// function, and a tuple whose setter is bound and cast.
type Node = { value: unknown };

function read(node: Node) {
  return node.value;
}

export function setSignal(node: Node, value: unknown) {
  node.value = value;
}

export function accessor(node: Node) {
  const fn = read.bind(null, node);
  return fn;
}

export function createSignal<T>(): [() => T, (v: T) => void];
export function createSignal<T>(value: T): [() => T, (v: T) => void];
export function createSignal<T>(value?: T) {
  const node: Node = { value };
  return [accessor(node), setSignal.bind(null, node) as (v: T) => void];
}

export function createStore() {
  const state = { count: 0 };
  return {
    get: () => state.count,
    set(next: number) {
      state.count = next;
    },
    get label() {
      return "count";
    },
    get handler() {
      return onEvent;
    },
  };
}

function onEvent() {
  return "event";
}

export function makeList() {
  const list: Array<() => number> = [];
  list.push(() => 1);
  return list;
}
