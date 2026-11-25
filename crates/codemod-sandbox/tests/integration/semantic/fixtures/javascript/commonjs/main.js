const lib = require("./lib");
const { parse } = require("./lib");
const Db = require("./db");
const store = require("./store");
const again = require("./reexport");
const { persist } = require("./picks");
const pad = require("left-pad");
const plugin = require(process.env.PLUGIN);

function run(input) {
  const db = new Db(lib.format(input));
  db.query(parse(input));
  store.save(input);
  store.load();
  again.format(input);
  persist(input);
  pad(input);
  plugin.init();
}

module.exports = { run };
