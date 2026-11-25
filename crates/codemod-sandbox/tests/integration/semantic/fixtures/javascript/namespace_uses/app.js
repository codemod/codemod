import * as utils from "./utils.js";
import { hooks } from "./hooks.js";

export const handlers = { round: utils.oldRound };
hooks.register(utils.registered);

export function run() {
  return utils?.["called"]();
}
