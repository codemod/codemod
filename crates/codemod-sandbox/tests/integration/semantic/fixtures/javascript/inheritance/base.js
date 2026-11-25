function setup(value) {
  return value;
}

export class Base {
  constructor(value) {
    this.value = setup(value);
  }
  greet() {
    return this.describe();
  }
  describe() {
    return "base";
  }
}
