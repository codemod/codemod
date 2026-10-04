import { setup } from "lib";

hit("setup");

function load(setup: () => void) {
  return setup();
}
