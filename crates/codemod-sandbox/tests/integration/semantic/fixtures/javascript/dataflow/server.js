import { createUser } from "./services/index.js";
import { query } from "./db.js";

export function handler(req, res) {
  const { name, email } = req.body;
  const id = req.params.id;
  const label = `user:${name}`;
  let limit = 10;
  if (req.query.limit) {
    limit = Number(req.query.limit);
  }
  const user = createUser(label, email);
  query("select * from users where id = " + id, limit);
  log(user);
  res.json(user);
}
