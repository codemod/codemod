import { setup } from "lib";

const a = setup();
const b = a;

a.stop();
b.stop();
