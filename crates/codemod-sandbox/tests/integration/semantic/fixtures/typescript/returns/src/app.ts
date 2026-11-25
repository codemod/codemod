import { createSignal, createStore, makeList } from "./index.js";
import { render } from "testing-library";

export function app() {
  const [count, setCount] = createSignal(0);
  setCount(count() + 1);

  const store = createStore();
  store.set(2);
  store.handler();
  log(store.label);
  const { get } = createStore();
  get();

  const [first] = makeList();
  first();

  const { getByRole } = render();
  getByRole("button");
}
