//! `codeatlas bench`: reproducible measurements on one repository.
//!
//! Measures analysis (each run in a fresh process, with its peak memory),
//! full and incremental indexing, store query latency and in-memory
//! analyses, and, given a ground truth, test-selection precision and
//! recall. Everything is written as JSON with the environment it was
//! measured in. Indexing runs on a scratch copy under a repository ID of
//! its own, which is deleted afterwards; the source is never modified.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use codeatlas_analyzer::git::Git;
use codeatlas_analyzer::graph::architecture::{self, Level};
use codeatlas_analyzer::graph::impact::{impact_of_symbol, ImpactOptions};
use codeatlas_analyzer::graph::test_selection::select_tests;
use codeatlas_analyzer::graph::CodeGraph;
use codeatlas_analyzer::ingest::{ingest, IngestOptions, IngestedRepository};
use codeatlas_analyzer::model::{EdgeKind, SymbolId};
use codeatlas_analyzer::test_evaluation::{evaluate_tests, TestGroundTruth, TestImpactSummary};
use codeatlas_analyzer::{analyze, RepoSource, RepositoryAnalysis};
use codeatlas_store::{prepare, DeltaStats, Direction, GraphStore, IndexMode, StateDir};
use serde::{Deserialize, Serialize};

use crate::probe::copy_tree;

/// Version of the result layout.
const SCHEMA: u32 = 1;

pub struct BenchOptions {
    pub runs: usize,
    pub query_runs: usize,
    pub targets: usize,
    pub seed: u64,
    /// Revision to measure incremental indexing from (to `HEAD` and back).
    pub base: Option<String>,
    pub test_truth: Option<PathBuf>,
    /// Measure indexing and queries (needs Neo4j).
    pub database: bool,
}

/// One analysis, measured by a child process (`bench-analysis`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisSample {
    /// Discovery, reading and analysis, as seen by the child.
    pub total_ms: f64,
    pub parse_ms: f64,
    pub resolve_ms: f64,
    /// Peak resident memory of the child process (`None` where unknown).
    pub peak_rss_bytes: Option<u64>,
}

/// Runs inside the child process.
pub fn analysis_sample(source: &Path) -> Result<AnalysisSample> {
    let started = Instant::now();
    let repo = ingest(
        &RepoSource::Local(source.to_path_buf()),
        &IngestOptions::default(),
    )?;
    let analysis = analyze(&repo)?;
    Ok(AnalysisSample {
        total_ms: ms(started),
        parse_ms: analysis.stats.parse_ms,
        resolve_ms: analysis.stats.resolve_ms,
        peak_rss_bytes: peak_rss_bytes(),
    })
}

#[cfg(unix)]
fn peak_rss_bytes() -> Option<u64> {
    use nix::sys::resource::{getrusage, UsageWho};
    let max = u64::try_from(getrusage(UsageWho::RUSAGE_SELF).ok()?.max_rss()).ok()?;
    // macOS reports bytes, Linux and the BSDs kilobytes.
    Some(if cfg!(target_os = "macos") {
        max
    } else {
        max * 1024
    })
}

#[cfg(not(unix))]
fn peak_rss_bytes() -> Option<u64> {
    None
}

#[derive(Debug, Clone, Serialize)]
pub struct Stat {
    pub n: usize,
    pub min: f64,
    pub median: f64,
    pub p95: f64,
    pub max: f64,
    pub mean: f64,
}

fn stat(samples: &[f64]) -> Option<Stat> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
    Some(Stat {
        n: sorted.len(),
        min: round(sorted[0]),
        median: round(at(0.5)),
        p95: round(at(0.95)),
        max: round(sorted[sorted.len() - 1]),
        mean: round(sorted.iter().sum::<f64>() / sorted.len() as f64),
    })
}

#[derive(Debug, Serialize)]
pub struct BenchReport {
    pub schema: u32,
    pub codeatlas: String,
    /// `release` or `debug`; debug builds are much slower.
    pub build: String,
    pub measured_at: String,
    pub repository: RepositoryInfo,
    pub environment: Environment,
    pub parameters: Parameters,
    pub corpus: Corpus,
    pub analysis: Analysis,
    pub indexing: Option<Indexing>,
    pub queries: Vec<QueryLatency>,
    pub test_impact: Option<TestImpact>,
    /// What was not measured, and why.
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct RepositoryInfo {
    pub name: String,
    pub origin_url: Option<String>,
    pub commit: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Environment {
    pub os: String,
    pub arch: String,
    pub cpu: Option<String>,
    pub logical_cpus: usize,
    pub neo4j: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Parameters {
    pub runs: usize,
    pub query_runs: usize,
    pub targets: usize,
    pub seed: u64,
    pub base: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Corpus {
    pub source_files: u32,
    pub rust_files: u32,
    pub loc: u64,
    pub rust_loc: u64,
    pub symbols: BTreeMap<String, u32>,
    pub tests: u32,
    pub call_sites: u32,
    /// Resolved relationships by kind, and ambiguous-call candidates.
    pub relationships: BTreeMap<String, usize>,
    pub resolution_rate: Option<f64>,
    pub calls_resolved: u32,
    pub calls_ambiguous: u32,
    pub calls_unresolved: u32,
    /// Nodes and relationships stored in Neo4j.
    pub stored_nodes: Option<usize>,
    pub stored_relationships: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct Analysis {
    pub samples: Vec<AnalysisSample>,
    pub total_ms: Option<Stat>,
    pub parse_ms: Option<Stat>,
    pub resolve_ms: Option<Stat>,
    pub peak_rss_mb: Option<Stat>,
    /// Rust LOC per second of the whole analysis (median total time).
    pub loc_per_second: Option<f64>,
    pub files_per_second: Option<f64>,
    /// Rust LOC per second of parsing alone (median parse time).
    pub parse_loc_per_second: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IndexSample {
    pub mode: String,
    pub ingest_ms: f64,
    pub analysis_ms: f64,
    pub write_ms: f64,
    pub total_ms: f64,
    pub files_parsed: u32,
    pub delta: DeltaStats,
}

#[derive(Debug, Serialize)]
pub struct IndexSeries {
    pub description: String,
    pub samples: Vec<IndexSample>,
    pub total_ms: Option<Stat>,
    pub write_ms: Option<Stat>,
}

#[derive(Debug, Serialize)]
pub struct Indexing {
    pub full: IndexSeries,
    pub unchanged: IndexSeries,
    pub one_file: IndexSeries,
    pub range_forward: Option<IndexSeries>,
    pub range_back: Option<IndexSeries>,
}

#[derive(Debug, Serialize)]
pub struct QueryLatency {
    pub name: String,
    /// `neo4j` for store queries, `memory` for analyses on the loaded graph.
    pub runs_on: String,
    pub latency_ms: Option<Stat>,
}

#[derive(Debug, Serialize)]
pub struct TestImpact {
    pub truth: String,
    pub resolved_only: TestImpactSummary,
    pub with_ambiguous: TestImpactSummary,
}

pub async fn bench(source: &Path, options: &BenchOptions) -> Result<BenchReport> {
    if options.runs == 0 {
        bail!("--runs must be at least 1");
    }
    let mut notes = Vec::new();
    if cfg!(debug_assertions) {
        notes.push("debug build: timings are not representative".to_string());
    }
    eprintln!("Analysing (reference run)...");
    let repo = ingest(
        &RepoSource::Local(source.to_path_buf()),
        &IngestOptions::default(),
    )?;
    let analysis = analyze(&repo)?;
    let graph = CodeGraph::from_analysis(&analysis);
    let mut corpus = corpus(&analysis);

    eprintln!("Analysing in {} fresh processes...", options.runs);
    let exe = std::env::current_exe()?;
    let mut samples = Vec::new();
    for _ in 0..options.runs {
        let output = Command::new(&exe)
            .arg("bench-analysis")
            .arg(&analysis.repository.root)
            .env("RUST_LOG", "warn")
            .output()
            .context("cannot run the analysis process")?;
        if !output.status.success() {
            bail!(
                "analysis process failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        samples.push(serde_json::from_slice::<AnalysisSample>(&output.stdout)?);
    }
    let analysis_stats = analysis_series(samples, &corpus);
    if analysis_stats
        .samples
        .iter()
        .any(|s| s.peak_rss_bytes.is_none())
    {
        notes.push("peak memory is not available on this platform".into());
    }

    let mut indexing = None;
    let mut queries = Vec::new();
    let mut neo4j = None;
    if options.database {
        let store = crate::graph::connect().await?;
        neo4j = store.server_version().await.ok();
        let (series, stored) = index_series(&store, &analysis, options).await?;
        corpus.stored_nodes = Some(stored.0);
        corpus.stored_relationships = Some(stored.1);
        indexing = Some(series);
        queries = query_latencies(&store, &analysis, &graph, options).await?;
    } else {
        notes.push("indexing and query latency not measured (--no-database)".into());
    }

    let test_impact = match &options.test_truth {
        Some(path) => {
            let truth: TestGroundTruth = serde_json::from_str(
                &fs::read_to_string(path)
                    .with_context(|| format!("failed to read {}", path.display()))?,
            )?;
            let evaluate = |include_ambiguous| {
                evaluate_tests(
                    &graph,
                    &truth,
                    ImpactOptions {
                        include_ambiguous,
                        ..Default::default()
                    },
                )
                .summary
            };
            Some(TestImpact {
                truth: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                resolved_only: evaluate(false),
                with_ambiguous: evaluate(true),
            })
        }
        None => {
            notes.push("test-impact precision and recall not measured (no --test-truth)".into());
            None
        }
    };

    Ok(BenchReport {
        schema: SCHEMA,
        codeatlas: env!("CARGO_PKG_VERSION").to_string(),
        build: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
        .to_string(),
        measured_at: analysis.repository.analyzed_at.to_rfc3339(),
        repository: RepositoryInfo {
            name: analysis.repository.name.clone(),
            origin_url: analysis.repository.origin_url.clone(),
            commit: analysis.repository.head_sha.clone(),
        },
        environment: Environment {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            cpu: cpu_name(),
            logical_cpus: std::thread::available_parallelism().map_or(1, usize::from),
            neo4j,
        },
        parameters: Parameters {
            runs: options.runs,
            query_runs: options.query_runs,
            targets: options.targets,
            seed: options.seed,
            base: options.base.clone(),
        },
        corpus,
        analysis: analysis_stats,
        indexing,
        queries,
        test_impact,
        notes,
    })
}

fn corpus(a: &RepositoryAnalysis) -> Corpus {
    let mut relationships: BTreeMap<String, usize> = BTreeMap::new();
    for edge in &a.resolution.edges {
        let kind = match edge.kind {
            EdgeKind::Calls => "calls",
            EdgeKind::Imports => "imports",
            EdgeKind::Implements => "implements",
        };
        *relationships.entry(kind.to_string()).or_default() += 1;
    }
    relationships.insert(
        "ambiguous_candidates".into(),
        a.resolution
            .ambiguous_calls
            .iter()
            .map(|c| c.candidates.len())
            .sum(),
    );
    let calls = &a.resolution.stats.calls;
    Corpus {
        source_files: a.repository.source_files,
        rust_files: a.stats.files_analyzed,
        loc: a.repository.loc,
        rust_loc: a.stats.loc_analyzed,
        symbols: a
            .stats
            .symbols
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), *v))
            .collect(),
        tests: a.stats.tests,
        call_sites: a.stats.call_sites,
        relationships,
        resolution_rate: calls.resolution_rate.map(|r| round(r * 1000.0) / 1000.0),
        calls_resolved: calls.resolved,
        calls_ambiguous: calls.ambiguous,
        calls_unresolved: calls.unresolved,
        stored_nodes: None,
        stored_relationships: None,
    }
}

fn analysis_series(samples: Vec<AnalysisSample>, corpus: &Corpus) -> Analysis {
    let total: Vec<f64> = samples.iter().map(|s| s.total_ms).collect();
    let total_ms = stat(&total);
    let parse_ms = stat(&samples.iter().map(|s| s.parse_ms).collect::<Vec<_>>());
    let parse_loc_per_second = parse_ms
        .as_ref()
        .filter(|p| p.median > 0.0)
        .map(|p| round(corpus.rust_loc as f64 / (p.median / 1000.0)));
    let per_second = |count: f64| {
        total_ms
            .as_ref()
            .filter(|t| t.median > 0.0)
            .map(|t| round(count / (t.median / 1000.0)))
    };
    Analysis {
        parse_ms,
        parse_loc_per_second,
        resolve_ms: stat(&samples.iter().map(|s| s.resolve_ms).collect::<Vec<_>>()),
        peak_rss_mb: stat(
            &samples
                .iter()
                .filter_map(|s| s.peak_rss_bytes)
                .map(|b| b as f64 / (1024.0 * 1024.0))
                .collect::<Vec<_>>(),
        ),
        loc_per_second: per_second(corpus.rust_loc as f64),
        files_per_second: per_second(f64::from(corpus.rust_files)),
        total_ms,
        samples,
    }
}

/// A scratch copy indexed under its own ID.
struct Scratch {
    dir: tempfile::TempDir,
    root: PathBuf,
    id: String,
}

fn scratch_copy(from: &Path, name: &str, id: String) -> Result<Scratch> {
    let dir = tempfile::Builder::new()
        .prefix("codeatlas-bench-")
        .tempdir()?;
    // Keep the repository's directory name: it is part of its identity.
    let root = dir.path().join(name);
    copy_tree(from, &root)?;
    Ok(Scratch { dir, root, id })
}

fn ingest_as(root: &Path, id: &str) -> Result<(IngestedRepository, f64)> {
    let started = Instant::now();
    let mut repo = ingest(
        &RepoSource::Local(root.to_path_buf()),
        &IngestOptions::default(),
    )?;
    repo.info.id = id.to_string();
    Ok((repo, ms(started)))
}

/// Ingests, analyses (with `states`) and writes one index.
async fn index_once(
    store: &GraphStore,
    root: &Path,
    id: &str,
    states: &StateDir,
    full: bool,
) -> Result<(IndexSample, (usize, usize))> {
    let started = Instant::now();
    let (repo, ingest_ms) = ingest_as(root, id)?;
    let prepared = prepare(&repo, states)?;
    let report = store.index_prepared(prepared, states, full).await?;
    Ok((
        IndexSample {
            mode: match report.mode {
                IndexMode::Incremental => "incremental".into(),
                IndexMode::Full(_) => "full".into(),
            },
            ingest_ms: round(ingest_ms),
            analysis_ms: round(report.analysis_ms),
            write_ms: round(report.summary.write_ms),
            total_ms: round(ms(started)),
            files_parsed: report.reuse.parsed,
            delta: report.delta,
        },
        (report.summary.nodes, report.summary.relationships),
    ))
}

fn series(description: &str, samples: Vec<IndexSample>) -> IndexSeries {
    IndexSeries {
        description: description.to_string(),
        total_ms: stat(&samples.iter().map(|s| s.total_ms).collect::<Vec<_>>()),
        write_ms: stat(&samples.iter().map(|s| s.write_ms).collect::<Vec<_>>()),
        samples,
    }
}

async fn index_series(
    store: &GraphStore,
    analysis: &RepositoryAnalysis,
    options: &BenchOptions,
) -> Result<(Indexing, (usize, usize))> {
    let name = &analysis.repository.name;
    let id = format!("bench-{name}-{}", std::process::id());
    let scratch = scratch_copy(&analysis.repository.root, name, id.clone())?;
    let states_root = scratch.dir.path().join("state");
    let result = async {
        eprintln!("Indexing in full {} times...", options.runs);
        let mut full = Vec::new();
        let mut stored = (0, 0);
        for i in 0..options.runs {
            // A fresh state directory each time: nothing to reuse.
            let states = StateDir::new(states_root.join(format!("full-{i}")));
            let (sample, counts) = index_once(store, &scratch.root, &scratch.id, &states, true).await?;
            full.push(sample);
            stored = counts;
        }

        // Seed the state used by the incremental runs.
        let states = StateDir::new(states_root.join("incremental"));
        index_once(store, &scratch.root, &scratch.id, &states, true).await?;
        eprintln!("Re-indexing without changes {} times...", options.runs);
        let mut unchanged = Vec::new();
        for _ in 0..options.runs {
            unchanged.push(index_once(store, &scratch.root, &scratch.id, &states, false).await?.0);
        }

        let file = largest_file(analysis).context("no Rust files to edit")?;
        let path = scratch.root.join(&file);
        let original = fs::read_to_string(&path)?;
        eprintln!("Re-indexing after editing {file} {} times...", options.runs);
        let mut one_file = Vec::new();
        for i in 0..options.runs {
            // Alternately insert a line at the top (shifting every line of
            // the file) and remove it again.
            let text = if i % 2 == 0 {
                format!("// codeatlas bench edit\n{original}")
            } else {
                original.clone()
            };
            fs::write(&path, text)?;
            one_file.push(index_once(store, &scratch.root, &scratch.id, &states, false).await?.0);
        }
        fs::write(&path, &original)?;

        let (range_forward, range_back) = match &options.base {
            Some(base) => {
                let (forward, back) = range_series(store, analysis, base, options).await?;
                (Some(forward), Some(back))
            }
            None => (None, None),
        };
        Ok::<_, anyhow::Error>((
            Indexing {
                full: series("full index without saved state (parse everything, write everything)", full),
                unchanged: series("incremental re-index, nothing changed", unchanged),
                one_file: series(
                    &format!("incremental re-index after inserting a line at the top of {file} (the largest file), alternately added and removed"),
                    one_file,
                ),
                range_forward,
                range_back,
            },
            stored,
        ))
    }
    .await;
    store.delete_repository(&scratch.id).await?;
    result
}

/// Incremental indexing between `base` and `HEAD`, from exported commits.
async fn range_series(
    store: &GraphStore,
    analysis: &RepositoryAnalysis,
    base: &str,
    options: &BenchOptions,
) -> Result<(IndexSeries, IndexSeries)> {
    let root = &analysis.repository.root;
    let git = Git::discover(root)?.context("--base needs a Git repository")?;
    let prefix = Git::prefix_of(root)?;
    let base_sha = git.resolve_commit(base)?;
    let head_sha = git.resolve_commit("HEAD")?;
    let dir = tempfile::Builder::new()
        .prefix("codeatlas-bench-range-")
        .tempdir()?;
    let work = dir.path().join(&analysis.repository.name);
    let export = |sha: &str| -> Result<()> {
        let _ = fs::remove_dir_all(&work);
        fs::create_dir_all(&work)?;
        git.export(sha, &prefix, &work)?;
        Ok(())
    };
    let id = format!(
        "bench-{}-{}-range",
        analysis.repository.name,
        std::process::id()
    );
    let states = StateDir::new(dir.path().join("state"));
    let result = async {
        export(&base_sha)?;
        index_once(store, &work, &id, &states, true).await?;
        eprintln!(
            "Re-indexing {base}..HEAD and back {} times...",
            options.runs
        );
        let (mut forward, mut back) = (Vec::new(), Vec::new());
        for _ in 0..options.runs {
            export(&head_sha)?;
            forward.push(index_once(store, &work, &id, &states, false).await?.0);
            export(&base_sha)?;
            back.push(index_once(store, &work, &id, &states, false).await?.0);
        }
        Ok::<_, anyhow::Error>((
            series(
                &format!("incremental re-index from {base} to HEAD"),
                forward,
            ),
            series(
                &format!("incremental re-index from HEAD back to {base}"),
                back,
            ),
        ))
    }
    .await;
    store.delete_repository(&id).await?;
    result
}

fn largest_file(a: &RepositoryAnalysis) -> Option<String> {
    a.files
        .iter()
        .max_by(|x, y| x.loc.cmp(&y.loc).then(y.path.cmp(&x.path)))
        .map(|f| f.path.clone())
}

/// Half the most-called functions and methods, half a seeded sample of the
/// rest (tests excluded).
fn targets(graph: &CodeGraph, count: usize, seed: u64) -> Vec<usize> {
    let mut callables: Vec<usize> = (0..graph.len())
        .filter(|&n| {
            let s = graph.symbol(n);
            s.kind.is_callable() && !s.is_test
        })
        .collect();
    callables.sort_by_key(|&n| {
        (
            std::cmp::Reverse(graph.incoming(n).len()),
            graph.symbol(n).id.clone(),
        )
    });
    let top = count / 2;
    let mut chosen: Vec<usize> = callables.iter().copied().take(top).collect();
    let mut rest: Vec<usize> = callables.into_iter().skip(top).collect();
    crate::probe::shuffle(&mut rest, seed);
    chosen.extend(rest.into_iter().take(count - chosen.len().min(count)));
    chosen
}

async fn query_latencies(
    store: &GraphStore,
    analysis: &RepositoryAnalysis,
    graph: &CodeGraph,
    options: &BenchOptions,
) -> Result<Vec<QueryLatency>> {
    let name = &analysis.repository.name;
    let id = format!("bench-{name}-{}-queries", std::process::id());
    let scratch = scratch_copy(&analysis.repository.root, name, id.clone())?;
    let states = StateDir::new(scratch.dir.path().join("state"));
    let result = async {
        index_once(store, &scratch.root, &id, &states, true).await?;
        let nodes = targets(graph, options.targets, options.seed);
        let ids: Vec<SymbolId> = nodes.iter().map(|&n| graph.symbol(n).id.clone()).collect();
        eprintln!(
            "Running queries ({} targets, {} runs each)...",
            ids.len(),
            options.query_runs
        );
        let mut out = Vec::new();
        let mut time = |name: &str, runs_on: &str, samples: Vec<f64>| {
            out.push(QueryLatency {
                name: name.to_string(),
                runs_on: runs_on.to_string(),
                latency_ms: stat(&samples),
            });
        };
        macro_rules! per_target {
            ($name:expr, |$sid:ident, $i:ident| $call:expr) => {{
                let mut samples = Vec::new();
                for _ in 0..options.query_runs {
                    for ($i, $sid) in ids.iter().enumerate() {
                        let started = Instant::now();
                        let _ = $call;
                        samples.push(ms(started));
                    }
                }
                time($name, "neo4j", samples);
            }};
        }
        per_target!("symbol by ID", |sid, _i| store
            .symbol(&id, sid.as_str())
            .await?);
        per_target!("search by name prefix", |sid, _i| store
            .search(&id, &prefix(graph, sid), None, 20)
            .await?);
        per_target!("callers, depth 1", |sid, _i| store
            .callers(&id, sid.as_str(), 1)
            .await?);
        per_target!("callers, depth 3", |sid, _i| store
            .callers(&id, sid.as_str(), 3)
            .await?);
        per_target!("callees, depth 2", |sid, _i| store
            .callees(&id, sid.as_str(), 2)
            .await?);
        per_target!("shortest path between two targets", |sid, i| store
            .shortest_path(&id, sid.as_str(), ids[(i + 1) % ids.len()].as_str(), 6)
            .await?);
        per_target!("file dependents", |sid, _i| store
            .file_dependencies(&id, &file_of(graph, sid), Direction::Dependents)
            .await?);

        let mut samples = Vec::new();
        let mut loaded = None;
        for _ in 0..options.query_runs {
            let started = Instant::now();
            loaded = Some(store.load_graph(&id).await?);
            samples.push(ms(started));
        }
        time("load the whole graph", "neo4j", samples);
        let loaded = loaded.context("graph not loaded")?;

        let mut per_target_memory = |name: &str, f: &dyn Fn(&SymbolId)| {
            let mut samples = Vec::new();
            for _ in 0..options.query_runs {
                for sid in &ids {
                    let started = Instant::now();
                    f(sid);
                    samples.push(ms(started));
                }
            }
            time(name, "memory", samples);
        };
        per_target_memory("impact of a symbol", &|sid| {
            let _ = impact_of_symbol(&loaded, sid, ImpactOptions::default());
        });
        per_target_memory("test selection for a symbol", &|sid| {
            if let Some(node) = loaded.node(sid) {
                let _ = select_tests(&loaded, &[node], ImpactOptions::default());
            }
        });
        for (name, f) in [
            (
                "module cycles",
                Box::new(|| {
                    let _ = architecture::cycles(&loaded, Level::Module);
                }) as Box<dyn Fn()>,
            ),
            (
                "function hotspots (betweenness)",
                Box::new(|| {
                    let _ = architecture::hotspots(&loaded, Level::Function, 15);
                }),
            ),
        ] {
            let mut samples = Vec::new();
            for _ in 0..options.query_runs {
                let started = Instant::now();
                f();
                samples.push(ms(started));
            }
            time(name, "memory", samples);
        }
        Ok::<_, anyhow::Error>(out)
    }
    .await;
    store.delete_repository(&id).await?;
    result
}

fn prefix(graph: &CodeGraph, id: &SymbolId) -> String {
    let name = graph
        .node(id)
        .map(|n| graph.symbol(n).name.clone())
        .unwrap_or_default();
    name.chars().take(4).collect()
}

fn file_of(graph: &CodeGraph, id: &SymbolId) -> String {
    graph
        .node(id)
        .map(|n| graph.symbol(n).file.clone())
        .unwrap_or_default()
}

fn cpu_name() -> Option<String> {
    if cfg!(target_os = "macos") {
        let out = Command::new("sysctl")
            .args(["-n", "machdep.cpu.brand_string"])
            .output()
            .ok()?;
        let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!name.is_empty()).then_some(name)
    } else {
        fs::read_to_string("/proc/cpuinfo")
            .ok()?
            .lines()
            .find_map(|l| {
                l.strip_prefix("model name")
                    .and_then(|r| r.split_once(':'))
                    .map(|(_, v)| v.trim().to_string())
            })
    }
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

fn round(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn fmt_ms(v: f64) -> String {
    if v < 10.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.0}")
    }
}

fn fmt_stat(s: &Option<Stat>) -> String {
    match s {
        Some(s) if s.n > 1 => format!(
            "{} ms (min {}, max {}, n={})",
            fmt_ms(s.median),
            fmt_ms(s.min),
            fmt_ms(s.max),
            s.n
        ),
        Some(s) => format!("{} ms", fmt_ms(s.median)),
        None => "n/a".into(),
    }
}

fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A readable summary of a report.
pub fn text(r: &BenchReport) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let c = &r.corpus;
    let _ = writeln!(
        out,
        "Benchmark   {} @ {} (codeatlas {}, {} build)",
        r.repository.name,
        r.repository
            .commit
            .as_deref()
            .map_or("no commit", |c| &c[..c.len().min(10)]),
        r.codeatlas,
        r.build
    );
    let _ = writeln!(
        out,
        "Machine     {} {}, {}, {} logical CPUs{}",
        r.environment.os,
        r.environment.arch,
        r.environment.cpu.as_deref().unwrap_or("unknown CPU"),
        r.environment.logical_cpus,
        r.environment
            .neo4j
            .as_deref()
            .map_or(String::new(), |v| format!("; {v}"))
    );
    let symbols: u32 = c.symbols.values().sum();
    let _ = writeln!(
        out,
        "Corpus      {} Rust files, {} LOC; {} symbols ({} tests), {} call sites",
        c.rust_files,
        thousands(c.rust_loc),
        thousands(u64::from(symbols)),
        c.tests,
        thousands(u64::from(c.call_sites))
    );
    let _ = writeln!(
        out,
        "Relations   {}{}",
        c.relationships
            .iter()
            .map(|(k, v)| format!("{k} {}", thousands(*v as u64)))
            .collect::<Vec<_>>()
            .join(", "),
        match (c.stored_nodes, c.stored_relationships) {
            (Some(n), Some(rel)) => format!(
                "; stored {} nodes, {} relationships",
                thousands(n as u64),
                thousands(rel as u64)
            ),
            _ => String::new(),
        }
    );
    let _ = writeln!(
        out,
        "Resolution  {} of resolvable call sites ({} resolved, {} ambiguous, {} unresolved)",
        c.resolution_rate
            .map_or("n/a".into(), |r| format!("{:.1}%", r * 100.0)),
        c.calls_resolved,
        c.calls_ambiguous,
        c.calls_unresolved
    );
    let a = &r.analysis;
    let _ = writeln!(out, "\nAnalysis (each run in a fresh process)");
    let _ = writeln!(out, "  total       {}", fmt_stat(&a.total_ms));
    let _ = writeln!(out, "  parsing     {}", fmt_stat(&a.parse_ms));
    let _ = writeln!(out, "  resolution  {}", fmt_stat(&a.resolve_ms));
    if let Some(m) = &a.peak_rss_mb {
        let _ = writeln!(
            out,
            "  peak memory {:.0} MB (min {:.0}, max {:.0})",
            m.median, m.min, m.max
        );
    }
    if let (Some(loc), Some(files)) = (a.loc_per_second, a.files_per_second) {
        let _ = writeln!(
            out,
            "  throughput  {} LOC/s, {:.0} files/s for the whole analysis; {} LOC/s parsing (medians)",
            thousands(loc as u64),
            files,
            a.parse_loc_per_second
                .map_or("n/a".into(), |p| thousands(p as u64))
        );
    }
    if let Some(i) = &r.indexing {
        let _ = writeln!(
            out,
            "\nIndexing (end to end: discovery, analysis, Neo4j write)"
        );
        let mut row = |label: &str, s: &IndexSeries| {
            let delta = s.samples.last().map(|x| x.delta).unwrap_or_default();
            let parsed = s.samples.last().map_or(0, |x| x.files_parsed);
            let _ = writeln!(
                out,
                "  {label:<22} {}; write {}; {} files parsed; nodes +{} -{} ~{}, relationships +{} -{} ~{}",
                fmt_stat(&s.total_ms),
                fmt_stat(&s.write_ms),
                parsed,
                delta.nodes_added,
                delta.nodes_removed,
                delta.nodes_changed,
                delta.relationships_added,
                delta.relationships_removed,
                delta.relationships_changed
            );
        };
        row("full", &i.full);
        row("unchanged", &i.unchanged);
        row("one file edited", &i.one_file);
        if let (Some(f), Some(b)) = (&i.range_forward, &i.range_back) {
            row("range forward", f);
            row("range back", b);
        }
    }
    if !r.queries.is_empty() {
        let _ = writeln!(out, "\nQueries (median / p95 / max, ms)");
        for q in &r.queries {
            if let Some(s) = &q.latency_ms {
                let _ = writeln!(
                    out,
                    "  {:<36} {:>8.2} {:>8.2} {:>8.2}  {} (n={})",
                    q.name, s.median, s.p95, s.max, q.runs_on, s.n
                );
            }
        }
    }
    if let Some(t) = &r.test_impact {
        let ratio = |v: Option<f64>| v.map_or("n/a".into(), |v| format!("{v:.3}"));
        let _ = writeln!(out, "\nTest impact ({})", t.truth);
        for (label, s) in [
            ("resolved calls", &t.resolved_only),
            ("with ambiguous", &t.with_ambiguous),
        ] {
            let _ = writeln!(
                out,
                "  {label:<16} precision {}, recall {} ({} counting only tests with a symbol), {} probes",
                ratio(s.precision),
                ratio(s.recall),
                ratio(s.recall_known_tests),
                s.probes
            );
        }
    }
    if !r.notes.is_empty() {
        let _ = writeln!(out, "\nNotes");
        for n in &r.notes {
            let _ = writeln!(out, "  {n}");
        }
    }
    out.trim_end().to_string()
}
