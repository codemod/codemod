import { setup } from "lib";

const a = hit("setup");
const b = a;

/*factory*/ a.stop();
b.stop();
