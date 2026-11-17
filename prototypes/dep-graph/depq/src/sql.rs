//! Variant B: the same graph as two SQLite tables, queried with plain SQL.
//! Range checks and registry lookups come in as SQL functions the host provides.

use crate::model::Graph;
use crate::query::{first_release_accepting, Verdict};
use crate::ranges::satisfies;
use crate::registry::Registry;
use rusqlite::{functions::FunctionFlags, params, types::Value, Connection};

pub fn open(g: &Graph, registry: Box<dyn Registry + Send>) -> rusqlite::Result<Connection> {
    let db = Connection::open_in_memory()?;
    db.execute_batch(
        "CREATE TABLE nodes (id INTEGER PRIMARY KEY, eco TEXT, name TEXT, version TEXT, workspace TEXT);
         CREATE TABLE edges (src INTEGER, dst INTEGER, name TEXT, req TEXT, kind TEXT);
         CREATE INDEX edges_src ON edges(src); CREATE INDEX edges_dst ON edges(dst); CREATE INDEX edges_name ON edges(name);
         CREATE INDEX nodes_name ON nodes(name);",
    )?;
    let tx = db.unchecked_transaction()?;
    {
        let mut ins = tx.prepare("INSERT INTO nodes VALUES (?1, ?2, ?3, ?4, ?5)")?;
        for (i, n) in g.nodes.iter().enumerate() {
            ins.execute(params![i as i64, n.eco.as_str(), n.name, n.version, n.workspace])?;
        }
        let mut ins = tx.prepare("INSERT INTO edges VALUES (?1, ?2, ?3, ?4, ?5)")?;
        for e in &g.edges {
            ins.execute(params![e.from as i64, e.to.map(|t| t as i64), e.name, e.req, e.kind.as_str()])?;
        }
    }
    tx.commit()?;

    db.create_scalar_function("satisfies", 3, FunctionFlags::SQLITE_DETERMINISTIC, |ctx| {
        let (eco, req, v): (String, Option<String>, String) = (ctx.get(0)?, ctx.get(1)?, ctx.get(2)?);
        let eco = crate::model::Eco::parse(&eco).unwrap_or(crate::model::Eco::Npm);
        Ok(req.and_then(|r| satisfies(eco, &r, &v)).map(i64::from))
    })?;
    // first_release_accepting(eco, pkg, current, dep, target) -> version, 'none', or NULL when unknown
    db.create_scalar_function("first_release_accepting", 5, FunctionFlags::empty(), move |ctx| {
        let a: Vec<String> = (0..5).map(|i| ctx.get::<String>(i)).collect::<Result<_, _>>()?;
        let eco = crate::model::Eco::parse(&a[0]).unwrap_or(crate::model::Eco::Npm);
        Ok(match first_release_accepting(registry.as_ref(), eco, &a[1], &a[2], &a[3], &a[4]) {
            Verdict::Fixable { version, .. } => Some(version),
            Verdict::Stuck { .. } => Some("none".to_string()),
            Verdict::LocalOverride => Some("override".to_string()),
            Verdict::Unknown => None,
        })
    })?;
    Ok(db)
}

/// Runs a script; prints each SELECT's rows as tab-separated lines with a header.
pub fn run_script(db: &Connection, script: &str, out: &mut dyn std::io::Write) -> rusqlite::Result<()> {
    let mut batch = rusqlite::Batch::new(db, script);
    while let Some(mut st) = batch.next()? {
        let cols: Vec<String> = st.column_names().iter().map(|s| s.to_string()).collect();
        if cols.is_empty() {
            st.execute([])?;
            continue;
        }
        writeln!(out, "{}", cols.join("\t")).ok();
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            let cells: Vec<String> = (0..cols.len())
                .map(|i| match r.get::<_, Value>(i) {
                    Ok(Value::Null) => "".into(),
                    Ok(Value::Integer(n)) => n.to_string(),
                    Ok(Value::Real(f)) => f.to_string(),
                    Ok(Value::Text(t)) => t,
                    Ok(Value::Blob(_)) => "<blob>".into(),
                    Err(_) => "?".into(),
                })
                .collect();
            writeln!(out, "{}", cells.join("\t")).ok();
        }
        writeln!(out).ok();
    }
    Ok(())
}
