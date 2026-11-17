use depq::{impact, parse, query, registry, sql};
use std::path::PathBuf;
use std::time::Instant;

const USAGE: &str = "usage:
  depq <repo> query '<pattern>'           e.g. 'ws:* ~> npm:react-beautiful-dnd'
  depq <repo> stuck '<pattern> refuses V'  adds a registry verdict per hit
  depq <repo> dupes [npm|cargo]            packages present in more than one version
  depq <repo> export                       the graph as JSON
  depq <repo> sql <file.sql | 'SELECT ...'>  variant B
  depq <repo> impact eco:name@V [--also a,b] [--ignore glob,glob]  variant C
flags: --offline (no registry), --cache DIR";

fn main() {
    if let Err(e) = real_main() {
        eprintln!("depq: {e}");
        std::process::exit(1);
    }
}

fn take_flag(args: &mut Vec<String>, flag: &str) -> Option<String> {
    let i = args.iter().position(|a| a == flag)?;
    args.remove(i);
    if i < args.len() { Some(args.remove(i)) } else { None }
}

fn real_main() -> Result<(), String> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let offline = args.iter().position(|a| a == "--offline").map(|i| args.remove(i)).is_some();
    let cache = take_flag(&mut args, "--cache").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or(".".into())).join(".cache/depq"));
    let also: Vec<String> = take_flag(&mut args, "--also").map(|s| s.split(',').map(String::from).collect()).unwrap_or_default();
    let ignore: Vec<String> = take_flag(&mut args, "--ignore").map(|s| s.split(',').map(String::from).collect()).unwrap_or_default();
    if args.len() < 2 {
        return Err(USAGE.into());
    }
    let reg = || -> Box<dyn registry::Registry + Send> { if offline { Box::new(registry::Offline) } else { Box::new(registry::Http { cache: cache.clone() }) } };
    let repo = PathBuf::from(&args[0]);
    let t = Instant::now();
    let mut g = parse::load_repo(&repo)?;
    eprintln!("graph: {} nodes, {} edges from {} in {:?}", g.nodes.len(), g.edges.len(), g.sources.join(" + "), t.elapsed());
    let t = Instant::now();
    let arg = args.get(2).cloned().unwrap_or_default();
    let out = match args[1].as_str() {
        "query" | "stuck" => {
            let p = query::parse_pattern(&arg)?;
            let r = reg();
            if p.cond.is_some() {
                query::fill_reqs(&mut g, &p.to.glob, r.as_ref());
            }
            let hits = query::run(&g, &p);
            if args[1] == "stuck" {
                let query::Hits::Edges(edges) = hits else { return Err("stuck needs a one-edge pattern with refuses V".into()) };
                let Some(query::Cond::Refuses(v)) = &p.cond else { return Err("stuck needs `refuses V`".into()) };
                let mut seen = std::collections::BTreeMap::new();
                for h in edges {
                    let (pkg, ver) = h.from.rsplit_once('@').unwrap();
                    let eco = g.nodes.iter().find(|n| n.name == pkg).map(|n| n.eco).unwrap();
                    let name = h.to.rsplit_once('@').map(|(n, _)| n).filter(|n| !n.is_empty()).unwrap_or(&h.to).to_string();
                    seen.entry(h.from.clone()).or_insert_with(|| query::first_release_accepting(r.as_ref(), eco, pkg, ver, &name, v));
                }
                serde_json::to_string_pretty(&seen).unwrap()
            } else {
                serde_json::to_string_pretty(&hits).unwrap()
            }
        }
        "dupes" => serde_json::to_string_pretty(&query::duplicates(&g, model_eco(&arg))).unwrap(),
        "export" => serde_json::to_string(&g).unwrap(),
        "sql" => {
            let script = std::fs::read_to_string(&arg).unwrap_or(arg);
            let db = sql::open(&g, reg()).map_err(|e| e.to_string())?;
            let mut buf = vec![];
            sql::run_script(&db, &script, &mut buf).map_err(|e| e.to_string())?;
            String::from_utf8(buf).unwrap()
        }
        "impact" => {
            let r = reg();
            let res = impact::impact(&mut g, &repo, &arg, &also, &ignore, r.as_ref())?;
            serde_json::to_string_pretty(&res).unwrap()
        }
        other => return Err(format!("unknown command `{other}`\n{USAGE}")),
    };
    eprintln!("{} in {:?}", args[1], t.elapsed());
    println!("{out}");
    Ok(())
}

fn model_eco(s: &str) -> Option<depq::model::Eco> {
    depq::model::Eco::parse(s)
}
