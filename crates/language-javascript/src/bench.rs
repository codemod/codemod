//! Benchmark for the data-flow and call-graph provider (performance track).
//!
//! Ignored tests, run in release:
//!
//! ```text
//! DATAFLOW_BENCH_DIR=/path/to/repos cargo test --release -p language-javascript --lib \
//!   bench::micro -- --ignored --nocapture --test-threads=1
//! DATAFLOW_BENCH_DIR=/path/to/repos cargo test --release -p language-javascript --lib \
//!   bench::workspace -- --ignored --nocapture --test-threads=1
//! ```
//!
//! `DATAFLOW_BENCH_SCALE=10` with `DATAFLOW_BENCH_COPY_DIR=/tmp/somewhere` (outside any
//! git repository) runs `workspace` on ten copies of the tree, `copy0` … `copy9`.
//!
//! `micro` times the per-file passes on one thread. `workspace` runs the analyzer the
//! way a codemod does and reports phase times, query latencies, peak RSS and a digest
//! of every answer, so an optimization can show it changed nothing.

use crate::accurate::AccurateAnalyzer;
use crate::dataflow::build_file_flow;
use crate::flow_query::FlowHost;
use crate::oxc_adapter::{analyze, parse_and_analyze};
use language_core::{
    ByteRange, CallGraphDirection, CallGraphOptions, CallGraphResult, DefinitionOptions,
    FlowDirection, FlowOptions, FlowResult,
};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Counts live heap bytes when `DATAFLOW_BENCH_HEAP=1`, so the bench can
/// report what the analyzer holds apart from what the allocator keeps (RSS).
/// Off by default: the shared counters slow down parallel phases.
struct CountingAllocator;

static COUNTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static LIVE_BYTES: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static PEAK_BYTES: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

fn count(delta: isize) {
    let live = LIVE_BYTES.fetch_add(delta, std::sync::atomic::Ordering::Relaxed) + delta;
    if delta > 0 {
        PEAK_BYTES.fetch_max(live, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Start counting if `DATAFLOW_BENCH_HEAP=1`.
fn start_heap_count() {
    if std::env::var("DATAFLOW_BENCH_HEAP").is_ok_and(|v| v == "1") {
        COUNTING.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

unsafe impl std::alloc::GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        let pointer = unsafe { std::alloc::System.alloc(layout) };
        if !pointer.is_null() && COUNTING.load(std::sync::atomic::Ordering::Relaxed) {
            count(layout.size() as isize);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: std::alloc::Layout) {
        unsafe { std::alloc::System.dealloc(pointer, layout) };
        if COUNTING.load(std::sync::atomic::Ordering::Relaxed) {
            count(-(layout.size() as isize));
        }
    }

    unsafe fn realloc(
        &self,
        pointer: *mut u8,
        layout: std::alloc::Layout,
        new_size: usize,
    ) -> *mut u8 {
        let moved = unsafe { std::alloc::System.realloc(pointer, layout, new_size) };
        if !moved.is_null() && COUNTING.load(std::sync::atomic::Ordering::Relaxed) {
            count(new_size as isize - layout.size() as isize);
        }
        moved
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Live heap now and at its peak, in MB (0 unless counting).
fn heap_mb() -> (f64, f64) {
    (
        LIVE_BYTES.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6,
        PEAK_BYTES.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6,
    )
}

const EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx", "mjs", "cjs"];
/// Files sampled for queries, spread evenly over the sorted file list.
const SAMPLE_FILES: usize = 60;

fn walk(root: &Path, keep: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_exclude(true)
        .build()
        .flatten()
        .map(|entry| entry.into_path())
        .filter(|path| path.is_file() && keep(path))
        .collect();
    files.sort();
    files
}

fn is_source(path: &Path) -> bool {
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    EXTENSIONS.contains(&extension)
}

fn is_config(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name == "package.json"
        || name == "jsconfig.json"
        || (name.starts_with("tsconfig") && name.ends_with(".json"))
}

/// Peak resident set size of this process so far, in MB.
fn peak_rss_mb() -> f64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    let raw = usage.ru_maxrss as f64;
    // Bytes on macOS, kilobytes on Linux.
    if cfg!(target_os = "macos") {
        raw / 1e6
    } else {
        raw * 1024.0 / 1e6
    }
}

/// Resident set size right now, in MB.
fn rss_mb() -> f64 {
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output();
    output
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map(|kb| kb * 1024.0 / 1e6)
        .unwrap_or(0.0)
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e3
}

fn percentiles(mut times: Vec<Duration>) -> (f64, f64, f64) {
    if times.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    times.sort();
    let at = |q: usize| ms(times[(times.len() * q / 100).min(times.len() - 1)]);
    (at(50), at(95), ms(*times.last().unwrap()))
}

#[test]
#[ignore]
fn micro() {
    let Ok(dir) = std::env::var("DATAFLOW_BENCH_DIR") else {
        return;
    };
    let files: Vec<(PathBuf, String)> = walk(Path::new(&dir), is_source)
        .into_iter()
        .filter_map(|path| Some((path.clone(), std::fs::read_to_string(&path).ok()?)))
        .collect();
    let bytes: usize = files.iter().map(|(_, content)| content.len()).sum();

    let best = |run: &dyn Fn() -> usize| {
        let mut best = Duration::MAX;
        let mut count = 0;
        for _ in 0..3 {
            let started = Instant::now();
            count = run();
            best = best.min(started.elapsed());
        }
        (best, count)
    };
    let (symbols, symbol_files) = best(&|| {
        files
            .iter()
            .filter(|(path, content)| parse_and_analyze(path, content).is_ok())
            .count()
    });
    let (facts, fact_files) = best(&|| {
        files
            .iter()
            .filter(|(path, content)| build_file_flow(path, content).is_ok())
            .count()
    });
    let (both, _) = best(&|| {
        files
            .iter()
            .filter(|(path, content)| {
                parse_and_analyze(path, content).is_ok() && build_file_flow(path, content).is_ok()
            })
            .count()
    });
    let (shared, _) = best(&|| {
        files
            .iter()
            .filter(|(path, content)| analyze(path, content, true).is_ok())
            .count()
    });

    println!(
        "BENCH micro files={} mb={:.1} symbols_ms={:.0} symbol_files={symbol_files} facts_ms={:.0} fact_files={fact_files} symbols_plus_facts_ms={:.0} ratio={:.2} one_parse_ms={:.0} one_parse_ratio={:.2}",
        files.len(),
        bytes as f64 / 1e6,
        ms(symbols),
        ms(facts),
        ms(both),
        ms(both) / ms(symbols),
        ms(shared),
        ms(shared) / ms(symbols),
    );
}

/// Ten copies of the tree's sources and package/tsconfig files under `copy_dir`.
fn materialize_copies(source: &Path, copy_dir: &Path, copies: usize) -> PathBuf {
    let marker = copy_dir.join(format!(".complete-{copies}"));
    if marker.exists() {
        return copy_dir.to_path_buf();
    }
    let _ = std::fs::remove_dir_all(copy_dir);
    let files = walk(source, |path| is_source(path) || is_config(path));
    for copy in 0..copies {
        for file in &files {
            let relative = file.strip_prefix(source).unwrap();
            let target = copy_dir.join(format!("copy{copy}")).join(relative);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::copy(file, &target).unwrap();
        }
    }
    std::fs::write(&marker, "").unwrap();
    copy_dir.to_path_buf()
}

/// An answer's shape, without file contents and independent of the order
/// files and entries were found in.
fn digest_flow(result: &FlowResult, hasher: &mut DefaultHasher) {
    result.truncated.hash(hasher);
    for file in &result.files {
        file.file_path.hash(hasher);
        format!("{:?}", file.steps).hash(hasher);
    }
}

fn digest_call_graph(result: &CallGraphResult, hasher: &mut DefaultHasher) {
    result.truncated.hash(hasher);
    let mut files: Vec<(String, Vec<String>)> = result
        .files
        .iter()
        .map(|file| {
            let mut entries: Vec<String> = file.entries.iter().map(|e| format!("{e:?}")).collect();
            entries.sort();
            (file.file_path.display().to_string(), entries)
        })
        .collect();
    files.sort();
    files.hash(hasher);
    let mut unparsed: Vec<_> = result.unparsed.iter().map(|u| &u.file_path).collect();
    unparsed.sort();
    unparsed.hash(hasher);
}

struct Sites {
    forward: Vec<(PathBuf, ByteRange)>,
    backward: Vec<(PathBuf, ByteRange)>,
    functions: Vec<(PathBuf, ByteRange)>,
    /// Identifier references, for `definition()`.
    references: Vec<(PathBuf, ByteRange)>,
    /// Exported declarations, for `references()`.
    exports: Vec<(PathBuf, ByteRange)>,
}

/// A fixed sample: per sampled file, the first bindings that flow somewhere,
/// the first call arguments, and the first named functions.
fn pick_sites(analyzer: &AccurateAnalyzer, files: &[PathBuf]) -> Sites {
    let mut sites = Sites {
        forward: Vec::new(),
        backward: Vec::new(),
        functions: Vec::new(),
        references: Vec::new(),
        exports: Vec::new(),
    };
    let stride = (files.len() / SAMPLE_FILES).max(1);
    for path in files.iter().step_by(stride) {
        let Some(flow) = analyzer.flow(path) else {
            continue;
        };
        let bindings = flow
            .nodes
            .iter()
            .enumerate()
            .filter(|(index, node)| node.binding && !flow.out_edges[*index].is_empty());
        for (_, node) in bindings.take(2) {
            sites.forward.push((path.clone(), node.range));
        }
        let arguments = flow.calls.iter().flat_map(|call| call.args.iter());
        for argument in arguments.take(2) {
            sites
                .backward
                .push((path.clone(), flow.nodes[*argument as usize].range));
        }
        let named = flow.functions.iter().filter(|f| f.name.is_some());
        for function in named.take(2) {
            sites.functions.push((path.clone(), function.range));
        }
        if let Some((symbols, _)) = analyzer.cache().get(path) {
            for reference in symbols.references.iter().take(3) {
                sites.references.push((path.clone(), reference.range));
            }
            let exported = symbols
                .exports
                .iter()
                .filter_map(|e| e.local_symbol_id)
                .filter_map(|id| symbols.find_symbol_by_id(id));
            for symbol in exported.take(2) {
                sites.exports.push((path.clone(), symbol.range));
            }
        }
    }
    sites
}

/// The tree to run on: `DATAFLOW_BENCH_DIR`, or copies of it at `DATAFLOW_BENCH_SCALE`.
fn bench_root() -> Option<(PathBuf, usize)> {
    let dir = std::env::var("DATAFLOW_BENCH_DIR").ok()?;
    let scale: usize = std::env::var("DATAFLOW_BENCH_SCALE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let root = if scale > 1 {
        let copy_dir = std::env::var("DATAFLOW_BENCH_COPY_DIR")
            .expect("DATAFLOW_BENCH_COPY_DIR is needed for DATAFLOW_BENCH_SCALE > 1");
        let started = Instant::now();
        let root = materialize_copies(Path::new(&dir), Path::new(&copy_dir), scale);
        eprintln!("corpus ready in {:?}", started.elapsed());
        root
    } else {
        PathBuf::from(&dir)
    };
    Some((root.canonicalize().unwrap(), scale))
}

/// What a codemod pays for its first `callers()` query on a fresh analyzer:
/// symbols, facts and the table together. Its own test (process), so peak
/// RSS is this scenario's alone.
#[test]
#[ignore]
fn cold() {
    let Some((root, scale)) = bench_root() else {
        return;
    };
    start_heap_count();
    let files = walk(&root, is_source);
    let analyzer = AccurateAnalyzer::new(root.clone());
    // The first named function of the first file that has one.
    let site = files.iter().find_map(|path| {
        let content = std::fs::read_to_string(path).ok()?;
        let flow = build_file_flow(path, &content).ok()?;
        let function = flow.functions.iter().find(|f| f.name.is_some())?;
        Some((path.clone(), content, function.range))
    });
    let (path, content, range) = site.unwrap();
    let started = Instant::now();
    let result = analyzer
        .call_graph(
            &path,
            &content,
            range,
            CallGraphOptions {
                direction: CallGraphDirection::Callers,
                ..Default::default()
            },
        )
        .unwrap();
    let elapsed = started.elapsed();
    let mut hasher = DefaultHasher::new();
    digest_call_graph(&result, &mut hasher);
    println!(
        "BENCH cold scale={scale} threads={} first_callers_ms={:.0} rss_mb={:.0} peak_rss_mb={:.0} heap_mb={:.0} peak_heap_mb={:.0} digest={:016x}",
        rayon::current_num_threads(),
        ms(elapsed),
        rss_mb(),
        peak_rss_mb(),
        heap_mb().0,
        heap_mb().1,
        hasher.finish()
    );
}

/// Heap bytes held by facts, by part, estimated from capacities (allocator
/// overhead not counted). `(name, bytes)`.
fn flow_bytes(flow: &crate::dataflow::FileFlow) -> Vec<(&'static str, usize)> {
    use crate::dataflow::{CallFact, CalleeFact, FlowNode, FunctionFact, MutationFact, Name};
    use std::mem::size_of;
    // Names are shared: count each distinct one once (16 bytes of Arc header).
    let mut seen = std::collections::HashSet::new();
    let mut name_bytes = 0usize;
    let mut name = |name: Option<&Name>| {
        if let Some(name) = name {
            if seen.insert(std::sync::Arc::as_ptr(name) as *const u8 as usize) {
                name_bytes += name.len() + 16;
            }
        }
    };
    for node in &flow.nodes {
        name(node.name.as_ref());
    }
    for function in &flow.functions {
        name(function.name.as_ref());
    }
    for mutation in &flow.mutations {
        name(mutation.name.as_ref());
    }
    let mut args = 0;
    for call in &flow.calls {
        name(Some(&call.callee_text));
        if let CalleeFact::Member { property, .. } = &call.callee {
            name(Some(property));
        }
        args += call.args.len() * 4;
    }
    let map = |cap: usize, entry: usize| cap * (entry + 1);
    let mut members = map(flow.object_members.capacity(), 32)
        + map(flow.class_methods.capacity(), 32)
        + map(flow.class_fields.capacity(), 32);
    for list in flow
        .object_members
        .values()
        .chain(flow.class_methods.values())
        .chain(flow.class_fields.values())
    {
        members += list.capacity() * size_of::<(Name, u32)>();
        for (member, _) in list {
            name(Some(member));
        }
    }
    vec![
        ("nodes", flow.nodes.capacity() * size_of::<FlowNode>()),
        ("names", name_bytes),
        ("edges", flow.out_edges.bytes() + flow.in_edges.bytes()),
        (
            "span index + id maps",
            flow.span_index_bytes()
                + map(flow.fn_by_node.capacity(), 8)
                + map(flow.call_by_node.capacity(), 8),
        ),
        (
            "calls",
            flow.calls.capacity() * size_of::<CallFact>() + args,
        ),
        (
            "functions",
            flow.functions.capacity() * size_of::<FunctionFact>(),
        ),
        (
            "mutations",
            flow.mutations.capacity() * size_of::<MutationFact>(),
        ),
        ("members", members),
    ]
}

/// Heap bytes held by the symbol cache, by part, from capacities.
fn symbol_bytes(analyzer: &AccurateAnalyzer) -> Vec<(&'static str, usize)> {
    use crate::cache::{ExportedSymbol, ImportedSymbol, Symbol, SymbolReference};
    use std::mem::size_of;
    let (mut symbols, mut names, mut references, mut imports, mut exports, mut content) =
        (0, 0, 0, 0, 0, 0);
    let string = |s: &String| s.capacity();
    let option = |s: &Option<String>| s.as_ref().map_or(0, String::capacity);
    for path in analyzer.cache().files() {
        let Some((file, text)) = analyzer.cache().get(&path) else {
            continue;
        };
        symbols += file.symbols.capacity() * size_of::<Symbol>();
        names += file.symbols.iter().map(|s| string(&s.name)).sum::<usize>();
        references += file.references.capacity() * size_of::<SymbolReference>();
        imports += file.imports.capacity() * size_of::<ImportedSymbol>()
            + file
                .imports
                .iter()
                .map(|i| {
                    string(&i.local_name) + option(&i.imported_name) + string(&i.module_specifier)
                })
                .sum::<usize>();
        exports += file.exports.capacity() * size_of::<ExportedSymbol>()
            + file
                .exports
                .iter()
                .map(|e| string(&e.name) + option(&e.re_export_from) + option(&e.local_name))
                .sum::<usize>();
        content += text.len();
    }
    vec![
        ("symbols", symbols),
        ("symbol names", names),
        ("references", references),
        ("imports", imports),
        ("exports", exports),
        ("content", content),
    ]
}

#[test]
#[ignore]
fn workspace() {
    let Some((root, scale)) = bench_root() else {
        return;
    };
    start_heap_count();
    let rss_start = rss_mb();
    let analyzer = AccurateAnalyzer::new(root.clone());

    // 1. Index every file (what the first workspace query does). Once a
    //    callers query is coming, this builds the facts in the same parse.
    let started = Instant::now();
    let mut files = analyzer.workspace_files();
    files.sort();
    let index_time = started.elapsed();
    let rss_index = rss_mb();
    let heap_index = heap_mb().0;

    // 2. Facts for every file.
    let started = Instant::now();
    let mut stats = (0usize, 0usize, 0usize, 0usize);
    let mut parts: Vec<(&'static str, usize)> = Vec::new();
    for path in &files {
        if let Some(flow) = analyzer.flow(path) {
            for (index, (name, bytes)) in flow_bytes(&flow).into_iter().enumerate() {
                match parts.get_mut(index) {
                    Some(part) => part.1 += bytes,
                    None => parts.push((name, bytes)),
                }
            }
            stats.0 += flow.nodes.len();
            stats.1 += flow.out_edges.edge_count();
            stats.2 += flow.functions.len();
            stats.3 += flow.calls.len();
        }
    }
    let facts_time = started.elapsed();
    let rss_facts = rss_mb();
    let heap_facts = heap_mb().0;

    let sites = pick_sites(&analyzer, &files);
    let content_of = |path: &Path| std::fs::read_to_string(path).unwrap();

    // 3. Callers table: the first callers query builds it.
    let (first_path, first_range) = sites.functions[0].clone();
    let started = Instant::now();
    let _ = analyzer
        .call_graph(
            &first_path,
            &content_of(&first_path),
            first_range,
            CallGraphOptions {
                direction: CallGraphDirection::Callers,
                ..Default::default()
            },
        )
        .unwrap();
    let callers_table_time = started.elapsed();
    let rss_table = rss_mb();
    let heap_table = heap_mb().0;

    // 4. Queries. Each reads its file first, as the provider does.
    let mut hasher = DefaultHasher::new();
    let mut flow_query = |sites: &[(PathBuf, ByteRange)], direction: FlowDirection| {
        let mut times = Vec::new();
        for (path, range) in sites {
            let started = Instant::now();
            let content = content_of(path);
            let result = analyzer
                .data_flow(
                    path,
                    &content,
                    *range,
                    FlowOptions {
                        direction,
                        ..Default::default()
                    },
                )
                .unwrap();
            times.push(started.elapsed());
            digest_flow(&result, &mut hasher);
        }
        times
    };
    let flows_from = flow_query(&sites.forward, FlowDirection::Forward);
    let flows_to = flow_query(&sites.backward, FlowDirection::Backward);
    let mut call_query = |direction: CallGraphDirection, transitive: bool| {
        let mut times = Vec::new();
        let mut entries = 0usize;
        let mut truncated = 0usize;
        for (path, range) in &sites.functions {
            let started = Instant::now();
            let content = content_of(path);
            let result = analyzer
                .call_graph(
                    path,
                    &content,
                    *range,
                    CallGraphOptions {
                        direction,
                        transitive,
                        ..Default::default()
                    },
                )
                .unwrap();
            times.push(started.elapsed());
            entries += result.files.iter().map(|f| f.entries.len()).sum::<usize>();
            truncated += result.truncated as usize;
            digest_call_graph(&result, &mut hasher);
        }
        (times, entries, truncated)
    };
    let (callees, callee_entries, callee_truncated) = call_query(CallGraphDirection::Callees, true);
    let (callers, caller_entries, caller_truncated) =
        call_query(CallGraphDirection::Callers, false);

    // definition() and references(), with their own digest.
    let mut symbol_hasher = DefaultHasher::new();
    let mut definitions = Vec::new();
    for (path, range) in &sites.references {
        let started = Instant::now();
        let content = content_of(path);
        let result = analyzer
            .get_definition(path, &content, *range, DefinitionOptions::default())
            .ok()
            .flatten();
        definitions.push(started.elapsed());
        let shape = result.map(|mut definition| {
            definition.content.clear();
            format!("{definition:?}")
        });
        shape.hash(&mut symbol_hasher);
    }
    let mut references = Vec::new();
    let mut reference_count = 0usize;
    for (path, range) in &sites.exports {
        let started = Instant::now();
        let content = content_of(path);
        let result = analyzer.find_references(path, &content, *range);
        references.push(started.elapsed());
        let mut files: Vec<(String, Vec<String>)> = result
            .map(|result| result.files)
            .unwrap_or_default()
            .iter()
            .map(|file| {
                let mut locations: Vec<String> =
                    file.locations.iter().map(|l| format!("{l:?}")).collect();
                locations.sort();
                reference_count += locations.len();
                (file.file_path.display().to_string(), locations)
            })
            .collect();
        files.sort();
        files.hash(&mut symbol_hasher);
    }
    let rss_end = rss_mb();
    let heap_end = heap_mb().0;

    let row = |name: &str, times: Vec<Duration>| {
        let n = times.len();
        let (p50, p95, max) = percentiles(times);
        println!("BENCH query {name} n={n} p50_ms={p50:.3} p95_ms={p95:.3} max_ms={max:.3}");
    };
    println!(
        "BENCH workspace scale={scale} files={} nodes={} edges={} functions={} calls={}",
        files.len(),
        stats.0,
        stats.1,
        stats.2,
        stats.3
    );
    println!(
        "BENCH phases index_ms={:.0} facts_ms={:.0} callers_table_ms={:.0}",
        ms(index_time),
        ms(facts_time),
        ms(callers_table_time)
    );
    row("flowsFrom", flows_from);
    row("flowsTo", flows_to);
    row("callees_transitive", callees);
    row("callers", callers);
    row("definition", definitions);
    row("references", references);
    println!(
        "BENCH entries callees={callee_entries} (truncated {callee_truncated}) callers={caller_entries} (truncated {caller_truncated})"
    );
    println!(
        "BENCH rss_mb start={rss_start:.0} after_index={rss_index:.0} after_facts={rss_facts:.0} after_table={rss_table:.0} end={rss_end:.0} peak={:.0}",
        peak_rss_mb()
    );
    println!(
        "BENCH heap_mb after_index={heap_index:.0} after_facts={heap_facts:.0} after_table={heap_table:.0} end={heap_end:.0} peak={:.0}",
        heap_mb().1
    );
    let symbol_parts = symbol_bytes(&analyzer);
    let symbol_total: usize = symbol_parts.iter().map(|(_, bytes)| bytes).sum();
    let symbol_parts: Vec<String> = symbol_parts
        .iter()
        .map(|(name, bytes)| format!("{name}={:.1}", *bytes as f64 / 1e6))
        .collect();
    println!(
        "BENCH symbols_mb total={:.1} {}",
        symbol_total as f64 / 1e6,
        symbol_parts.join(" ")
    );
    let total: usize = parts.iter().map(|(_, bytes)| bytes).sum();
    let parts: Vec<String> = parts
        .iter()
        .map(|(name, bytes)| format!("{name}={:.1}", *bytes as f64 / 1e6))
        .collect();
    println!(
        "BENCH facts_mb total={:.1} {}",
        total as f64 / 1e6,
        parts.join(" ")
    );
    println!("BENCH digest {:016x}", hasher.finish());
    println!(
        "BENCH symbols_digest {:016x} (references found {reference_count})",
        symbol_hasher.finish()
    );
}
