// Parity: three JSSG commands in sequence (structured output, selector skip, rename).
import { sequence } from "../../../src/index.ts";
import { guarded, rename, replace } from "../jssg/transforms.ts";

export default sequence(replace(), guarded(), rename());
