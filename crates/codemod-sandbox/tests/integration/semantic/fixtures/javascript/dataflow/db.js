function connect() {
  return globalThis.pool;
}

export function query(sql, limit) {
  return connect().run(sql, limit);
}

export function save(row) {
  return connect().insert(row);
}
