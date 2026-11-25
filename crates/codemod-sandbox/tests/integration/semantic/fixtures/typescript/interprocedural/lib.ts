export function normalize(value: string) {
  return value.trim();
}

export function query(sql: string, limit: number) {
  return run(sql, limit);
}
