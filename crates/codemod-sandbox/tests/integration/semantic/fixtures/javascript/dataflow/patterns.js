// Destructuring assignment and class fields.
export function track(entries, obj, req) {
  let first, second, rest;
  [first, second = "none"] = pair(req.query.id);
  ({ name: rest } = obj);
  for ([first] of entries) send(first);
  send(second);
  send(rest);
}

export class Counter {
  count = 0;
  #step = config.step;
  log = [];
  onTick = () => this.#advance();
  #advance() {
    this.count += this.#step;
    this.report();
  }
  report() {
    emit(this.count);
  }
  start() {
    this.onTick();
  }
  dispatch(name) {
    this[name]();
  }
}
