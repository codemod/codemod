export function loadUser() {
  return runSql("select * from users");
}

function runSql(sql: string) {
  return audit(sql);
}

function audit(sql: string) {
  return sql;
}
