import { Base } from "./base.js";

class Sub extends Base {
  constructor() {
    super(1);
  }
  describe() {
    return "sub " + super.describe();
  }
  run() {
    this.greet();
    this.describe();
  }
}

class Leaf extends Sub {}

class Mixed extends withLogging(Base) {
  go() {
    this.greet();
  }
}

export function main() {
  const leaf = new Leaf();
  leaf.greet();
  new Mixed().go();
}
