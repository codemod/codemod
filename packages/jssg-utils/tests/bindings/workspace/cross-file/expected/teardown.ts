import { mocks } from "./browser";

const bus = {
  stop() {},
};

/*factory*/ mocks.stop();
bus.stop();
