import * as db from "../db.js";
import { validate } from "./validate.js";

function normalize(field) {
  return String(field).trim();
}

export const createUser = (name, email) => {
  const fields = [name, email];
  fields.forEach(normalize);
  validate(email);
  return db.save({ name, email });
};
