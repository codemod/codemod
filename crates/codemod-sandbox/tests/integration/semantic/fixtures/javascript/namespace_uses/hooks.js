export const hooks = {
  list: [],
  register(fn) {
    this.list.push(fn);
  },
};
