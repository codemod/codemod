import { handler } from "./handlers/a";
import { register } from "./bus";

register(handler);
