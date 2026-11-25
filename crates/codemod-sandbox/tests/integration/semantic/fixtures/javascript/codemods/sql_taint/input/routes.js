export function getUser(req, db) {
  const id = req.params.id;
  const sql = "select * from users where id = " + id;
  return db.query(sql);
}

export function search(req, db) {
  const term = req.query.q;
  return db.query(`select * from items where name = ${term} limit 10`);
}

export function rename(req, db) {
  const { name } = req.body;
  return db.query("update users set name = " + name + " where id = " + req.params.id);
}

export function count(db, table) {
  const fixed = "users";
  return db.query("select count(*) from " + fixed);
}
