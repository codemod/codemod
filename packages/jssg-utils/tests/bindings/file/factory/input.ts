import { setup } from "lib";

const server = setup();
const bus = {
  stop() {},
};
const inner = app.listen(3000);

server.stop();
bus.stop();
inner.listen();

function shadowed() {
  const server = app.listen(3000);
  server.stop();
}
