class Db {
  constructor(url) {
    this.url = url;
  }
  query(sql) {
    return sql;
  }
}
module.exports = Db;
