export function getUser(req, db) {
  const id = req.params.id;
  const sql = "select * from users where id = ?";
  return db.query(sql, [id]);
}

export function search(req, db) {
  const term = req.query.q;
  return db.query("select * from items where name = ? limit 10", [term]);
}

export function rename(req, db) {
  const { name } = req.body;
  return db.query("update users set name = ? where id = ?", [name, req.params.id]);
}

export function count(db, table) {
  const fixed = "users";
  return db.query("select count(*) from " + fixed);
}
