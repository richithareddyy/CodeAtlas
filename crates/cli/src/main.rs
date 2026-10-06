mod diff_view;
mod graph;
mod probe;
mod test_view;
mod views;

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use codeatlas_analyzer::diff::{analyze_diff, DiffOptions};
use codeatlas_analyzer::evaluation::{evaluate, Evaluation, GroundTruth, SetComparison};
use codeatlas_analyzer::graph::impact::ImpactOptions;
use codeatlas_analyzer::graph::CodeGraph;
use codeatlas_analyzer::ingest::{default_clone_dir, ingest, DiscoveryOptions, IngestOptions};
use codeatlas_analyzer::model::SymbolKind;
use codeatlas_analyzer::parser::RustParser;
use codeatlas_analyzer::test_evaluation::{evaluate_tests, source_hash, TestGroundTruth};
use codeatlas_analyzer::{analyze_source, RepoSource, RepositoryAnalysis};
use codeatlas_store::{prepare, StateDir};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "codeatlas",
    version,
    about = "Repository intelligence and change-impact analysis"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Analyse a local repository path or Git URL.
    Analyze {
        /// Local path or Git URL.
        source: String,
        #[arg(long, value_enum, default_value_t = Format::Summary)]
        format: Format,
        /// Write output to a file instead of stdout.
        #[arg(long, short)]
        output: Option<PathBuf>,
        /// Directory for cloned repositories.
        #[arg(long, env = "CODEATLAS_CLONE_DIR")]
        clone_dir: Option<PathBuf>,
        /// Also analyse files excluded by .gitignore.
        #[arg(long)]
        no_gitignore: bool,
        /// Maximum ambiguous / unresolved call sites listed by `--format resolution`.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Compare resolution against a ground-truth file (precision / recall).
    Evaluate {
        /// Repository to analyse.
        source: String,
        /// Ground truth JSON; defaults to `<source>/expected.json`.
        #[arg(long)]
        truth: Option<PathBuf>,
    },
    /// Analyse a repository and store its graph in Neo4j (replacing any
    /// previous index of the same repository).
    Index {
        /// Local path or Git URL.
        source: String,
        #[arg(long, env = "CODEATLAS_CLONE_DIR")]
        clone_dir: Option<PathBuf>,
        #[arg(long)]
        no_gitignore: bool,
        /// Rewrite the whole graph even if an incremental update is possible.
        #[arg(long)]
        full: bool,
        /// Where incremental state is kept between runs.
        #[arg(long, env = "CODEATLAS_STATE_DIR")]
        state_dir: Option<PathBuf>,
    },
    /// Compare two revisions: changed files and symbols, and what else the
    /// changes could affect. Works on a local clone; no database needed.
    Diff {
        /// Repository path (or Git URL, which is cloned first).
        #[arg(default_value = ".")]
        source: String,
        /// Base revision: branch, tag, SHA or expression such as `HEAD~1`.
        #[arg(long, short)]
        base: String,
        /// Head revision; defaults to the working tree (uncommitted changes
        /// and untracked files included).
        #[arg(long)]
        head: Option<String>,
        #[arg(long, value_enum, default_value_t = DiffFormat::Text)]
        format: DiffFormat,
        /// Maximum length of an impact chain.
        #[arg(long, default_value_t = 8)]
        depth: u32,
        /// Also follow ambiguous calls (reported as possible).
        #[arg(long)]
        include_ambiguous: bool,
        /// Entries shown per list in text and Markdown output.
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, short)]
        output: Option<PathBuf>,
        #[arg(long, env = "CODEATLAS_CLONE_DIR")]
        clone_dir: Option<PathBuf>,
    },
    /// Record which tests execute which functions, by making functions
    /// panic one at a time and running the test suite. Builds and runs the
    /// repository's tests (in a scratch copy): use only on repositories you
    /// trust. The result is the ground truth for `evaluate-tests`.
    ProbeTests {
        /// Local repository path.
        source: PathBuf,
        /// Number of functions to probe (a deterministic random sample of
        /// non-test functions and methods).
        #[arg(long, short = 'n', default_value_t = 20)]
        sample: usize,
        /// Seed of the sample.
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// Probe these symbol IDs instead of a sample (repeatable).
        #[arg(long = "symbol")]
        symbols: Vec<String>,
        /// Seconds allowed per test binary run.
        #[arg(long, default_value_t = 300)]
        timeout: u64,
        /// Where to write the ground truth (JSON).
        #[arg(long, short)]
        output: PathBuf,
    },
    /// Compare static test selection with a ground truth from `probe-tests`:
    /// precision, recall, and every miss.
    EvaluateTests {
        /// Repository path (the same state the ground truth was made from).
        source: PathBuf,
        #[arg(long)]
        truth: PathBuf,
        #[arg(long, default_value_t = 8)]
        depth: u32,
        /// Also select tests reached only through ambiguous calls.
        #[arg(long)]
        include_ambiguous: bool,
        /// Print JSON instead of text.
        #[arg(long)]
        json: bool,
        /// Also write the JSON result to this file.
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
    /// Delete an indexed repository's graph from Neo4j.
    Remove {
        /// Repository ID or name.
        repo: String,
        #[arg(long, env = "CODEATLAS_STATE_DIR")]
        state_dir: Option<PathBuf>,
    },
    /// Query an indexed repository.
    Query {
        /// Repository ID or name; optional when only one is indexed.
        #[arg(long, short, global = true)]
        repo: Option<String>,
        /// Print JSON instead of text.
        #[arg(long, global = true)]
        json: bool,
        #[command(subcommand)]
        query: graph::Query,
    },
    /// Print the tree-sitter syntax tree of a Rust file (for extending the extractor).
    Ast { file: PathBuf },
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Summary,
    /// Resolution breakdown with ambiguous and unresolved call sites.
    Resolution,
    Json,
}

#[derive(Clone, Copy, ValueEnum)]
enum DiffFormat {
    Text,
    /// For pull-request comments.
    Markdown,
    Json,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Neo4j settings may live in `.env`; real environment variables win.
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new("warn,codeatlas_analyzer=info,codeatlas_store=info")
        }))
        .with_writer(std::io::stderr)
        .init();

    match Cli::parse().command {
        Command::Analyze {
            source,
            format,
            output,
            clone_dir,
            no_gitignore,
            limit,
        } => {
            let analysis = analyze(&source, clone_dir, no_gitignore)?;
            let rendered = match format {
                Format::Summary => summary(&analysis),
                Format::Resolution => resolution_report(&analysis, limit),
                Format::Json => serde_json::to_string_pretty(&analysis)?,
            };
            match output {
                Some(path) => fs::write(&path, rendered)
                    .with_context(|| format!("failed to write {}", path.display()))?,
                None => writeln!(std::io::stdout(), "{rendered}")?,
            }
        }
        Command::Evaluate { source, truth } => {
            let truth_path = truth.unwrap_or_else(|| PathBuf::from(&source).join("expected.json"));
            let truth: GroundTruth = serde_json::from_str(
                &fs::read_to_string(&truth_path)
                    .with_context(|| format!("failed to read {}", truth_path.display()))?,
            )
            .with_context(|| format!("invalid ground truth in {}", truth_path.display()))?;
            let analysis = analyze_source(&RepoSource::parse(&source), &IngestOptions::default())
                .with_context(|| format!("failed to analyse {source}"))?;
            let evaluation = evaluate(&analysis.resolution, &truth);
            writeln!(std::io::stdout(), "{}", evaluation_report(&evaluation))?;
            if !evaluation.is_exact() {
                std::process::exit(1);
            }
        }
        Command::Index {
            source,
            clone_dir,
            no_gitignore,
            full,
            state_dir,
        } => {
            let states = StateDir::new(state_dir.unwrap_or_else(StateDir::default_dir));
            let repo = ingest(
                &RepoSource::parse(&source),
                &ingest_options(clone_dir, no_gitignore),
            )
            .with_context(|| format!("failed to read {source}"))?;
            let prepared =
                prepare(&repo, &states).with_context(|| format!("failed to analyse {source}"))?;
            writeln!(std::io::stdout(), "{}", summary(&prepared.analysis))?;
            writeln!(
                std::io::stdout(),
                "{}",
                graph::index(prepared, &states, full).await?
            )?;
        }
        Command::Diff {
            source,
            base,
            head,
            format,
            depth,
            include_ambiguous,
            limit,
            output,
            clone_dir,
        } => {
            let options = DiffOptions {
                impact: ImpactOptions {
                    max_depth: depth,
                    include_ambiguous,
                    ..Default::default()
                },
                ingest: IngestOptions {
                    clone_dir: clone_dir.unwrap_or_else(default_clone_dir),
                    ..Default::default()
                },
            };
            let report = analyze_diff(
                &RepoSource::parse(&source),
                &base,
                head.as_deref(),
                &options,
            )
            .with_context(|| {
                format!(
                    "failed to compare {base} with {}",
                    head.as_deref().unwrap_or("the working tree")
                )
            })?;
            let rendered = match format {
                DiffFormat::Text => diff_view::text(&report, limit),
                DiffFormat::Markdown => diff_view::markdown(&report, limit),
                DiffFormat::Json => serde_json::to_string_pretty(&report)?,
            };
            match output {
                Some(path) => fs::write(&path, rendered)
                    .with_context(|| format!("failed to write {}", path.display()))?,
                None => writeln!(std::io::stdout(), "{rendered}")?,
            }
        }
        Command::ProbeTests {
            source,
            sample,
            seed,
            symbols,
            timeout,
            output,
        } => {
            let analysis = analyze_source(
                &RepoSource::Local(source.clone()),
                &IngestOptions::default(),
            )
            .with_context(|| format!("failed to analyse {}", source.display()))?;
            let truth = probe::probe(
                &analysis,
                &probe::ProbeOptions {
                    symbols,
                    sample,
                    seed,
                    timeout: std::time::Duration::from_secs(timeout),
                },
            )?;
            fs::write(&output, serde_json::to_string_pretty(&truth)? + "\n")
                .with_context(|| format!("failed to write {}", output.display()))?;
            writeln!(
                std::io::stdout(),
                "Wrote {} probes over {} tests to {}",
                truth.probes.len(),
                truth.tests.len(),
                output.display()
            )?;
        }
        Command::EvaluateTests {
            source,
            truth,
            depth,
            include_ambiguous,
            json,
            output,
        } => {
            let truth: TestGroundTruth = serde_json::from_str(
                &fs::read_to_string(&truth)
                    .with_context(|| format!("failed to read {}", truth.display()))?,
            )
            .context("invalid ground truth")?;
            let analysis = analyze_source(
                &RepoSource::Local(source.clone()),
                &IngestOptions::default(),
            )
            .with_context(|| format!("failed to analyse {}", source.display()))?;
            if truth.source_hash != source_hash(&analysis) {
                eprintln!(
                    "warning: the sources differ from those the ground truth was recorded on"
                );
            }
            let graph = CodeGraph::from_analysis(&analysis);
            let options = ImpactOptions {
                max_depth: depth,
                include_ambiguous,
                ..Default::default()
            };
            let evaluation = evaluate_tests(&graph, &truth, options);
            let rendered = serde_json::to_string_pretty(&evaluation)?;
            if let Some(path) = output {
                fs::write(&path, rendered.clone() + "\n")
                    .with_context(|| format!("failed to write {}", path.display()))?;
            }
            let text = if json {
                rendered
            } else {
                test_view::evaluation(&evaluation)
            };
            writeln!(std::io::stdout(), "{text}")?;
        }
        Command::Remove { repo, state_dir } => {
            let states = StateDir::new(state_dir.unwrap_or_else(StateDir::default_dir));
            writeln!(
                std::io::stdout(),
                "{}",
                graph::remove(&repo, &states).await?
            )?;
        }
        Command::Query { repo, json, query } => {
            let out = graph::query(repo.as_deref(), json, query).await?;
            writeln!(std::io::stdout(), "{out}")?;
        }
        Command::Ast { file } => {
            let src = fs::read_to_string(&file)
                .with_context(|| format!("failed to read {}", file.display()))?;
            let tree = RustParser::new()?.parse(&src, &file.to_string_lossy())?;
            writeln!(std::io::stdout(), "{}", tree.root_node().to_sexp())?;
        }
    }
    Ok(())
}

fn ingest_options(clone_dir: Option<PathBuf>, no_gitignore: bool) -> IngestOptions {
    IngestOptions {
        clone_dir: clone_dir.unwrap_or_else(default_clone_dir),
        discovery: DiscoveryOptions {
            respect_gitignore: !no_gitignore,
            ..Default::default()
        },
    }
}

fn analyze(
    source: &str,
    clone_dir: Option<PathBuf>,
    no_gitignore: bool,
) -> Result<RepositoryAnalysis> {
    analyze_source(
        &RepoSource::parse(source),
        &ingest_options(clone_dir, no_gitignore),
    )
    .with_context(|| format!("failed to analyse {source}"))
}

fn summary(a: &RepositoryAnalysis) -> String {
    let repo = &a.repository;
    let s = &a.stats;
    let count = |kind| s.symbols.get(&kind).copied().unwrap_or(0);
    let head = match (&repo.branch, &repo.head_sha) {
        (Some(branch), Some(sha)) => format!("{branch} @ {}", &sha[..sha.len().min(10)]),
        (None, Some(sha)) => format!("detached @ {}", &sha[..sha.len().min(10)]),
        (Some(branch), None) => format!("{branch} (no commits)"),
        _ => "not a git repository".to_string(),
    };
    let languages = repo
        .languages
        .iter()
        .map(|l| format!("{:?} {} files / {} LOC", l.language, l.files, l.loc))
        .collect::<Vec<_>>()
        .join(", ");
    let crates = a
        .crates
        .iter()
        .map(|c| format!("{} ({:?})", c.name, c.kind).to_lowercase())
        .collect::<Vec<_>>()
        .join(", ");

    let mut out = String::new();
    let mut line = |label: &str, value: String| {
        out.push_str(&format!("{label:<12}{value}\n"));
    };
    line(
        "Repository",
        format!("{}  ({})", repo.name, repo.root.display()),
    );
    line("Revision", head);
    line(
        "Languages",
        if languages.is_empty() {
            "none".into()
        } else {
            languages
        },
    );
    line("Crates", crates);
    line(
        "Symbols",
        format!(
            "{} modules, {} structs, {} enums, {} traits, {} functions, {} methods ({} tests)",
            count(SymbolKind::Module),
            count(SymbolKind::Struct),
            count(SymbolKind::Enum),
            count(SymbolKind::Trait),
            count(SymbolKind::Function),
            count(SymbolKind::Method),
            s.tests
        ),
    );
    line(
        "References",
        format!(
            "{} call sites ({} inside macros), {} imports, {} impl blocks",
            s.call_sites, s.macro_call_sites, s.imports, s.impl_blocks
        ),
    );
    let calls = &a.resolution.stats.calls;
    line(
        "Calls",
        format!(
            "{} resolved, {} ambiguous, {} unresolved; {} external, {} constructors, {} local; resolution rate {}",
            calls.resolved,
            calls.ambiguous,
            calls.unresolved,
            calls.external,
            calls.constructors,
            calls.local,
            percent(calls.resolution_rate)
        ),
    );
    let imports = &a.resolution.stats.imports;
    line(
        "Imports",
        format!(
            "{} resolved, {} external, {} unresolved",
            imports.resolved, imports.external, imports.unresolved
        ),
    );
    line(
        "Parsing",
        format!(
            "{} files, {} LOC, {} with syntax errors, parse {:.1} ms, resolve {:.1} ms, total {:.1} ms",
            s.files_analyzed,
            s.loc_analyzed,
            s.files_with_syntax_errors,
            s.parse_ms,
            s.resolve_ms,
            s.total_ms
        ),
    );
    out.trim_end().to_string()
}

fn percent(value: Option<f64>) -> String {
    value.map_or_else(|| "n/a".to_string(), |v| format!("{:.1}%", v * 100.0))
}

fn resolution_report(a: &RepositoryAnalysis, limit: usize) -> String {
    let r = &a.resolution;
    let c = &r.stats.calls;
    let mut out = format!(
        "Call sites: {}\n  resolved     {:>6}\n  ambiguous    {:>6}\n  unresolved   {:>6}\n  external     {:>6}\n  constructors {:>6}\n  local        {:>6}\nResolution rate: {} (resolved / (resolved + ambiguous + unresolved))\n",
        c.total, c.resolved, c.ambiguous, c.unresolved, c.external, c.constructors, c.local,
        percent(c.resolution_rate)
    );
    out.push_str("\nResolved by\n");
    for (method, n) in &c.resolved_by {
        out.push_str(&format!("  {:<24}{n:>6}\n", method.as_str()));
    }
    out.push_str("\nAmbiguous by\n");
    for (reason, n) in &c.ambiguous_by {
        out.push_str(&format!("  {:<24}{n:>6}\n", reason.as_str()));
    }
    out.push_str("\nUnresolved by\n");
    for (reason, n) in &c.unresolved_by {
        out.push_str(&format!("  {:<24}{n:>6}\n", reason.as_str()));
    }

    out.push_str(&format!("\nUnresolved call sites (first {limit})\n"));
    for call in r.unresolved_calls.iter().take(limit) {
        out.push_str(&format!(
            "  {}:{}  {}  [{}]\n",
            call.caller,
            call.line,
            call.callee,
            call.reason.as_str()
        ));
    }
    out.push_str(&format!("\nAmbiguous call sites (first {limit})\n"));
    for call in r.ambiguous_calls.iter().take(limit) {
        out.push_str(&format!(
            "  {}:{}  {}  [{}, {} candidates]\n",
            call.caller,
            call.line,
            call.callee,
            call.reason.as_str(),
            call.candidate_count
        ));
    }
    out.trim_end().to_string()
}

fn evaluation_report(e: &Evaluation) -> String {
    let mut out = String::new();
    for (label, c) in [
        ("calls", &e.calls),
        ("implements", &e.implements),
        ("ambiguous", &e.ambiguous),
        ("unresolved", &e.unresolved),
    ] {
        out.push_str(&comparison(label, c));
    }
    out.push_str(if e.is_exact() {
        "result: matches ground truth"
    } else {
        "result: differs from ground truth"
    });
    out
}

fn comparison(label: &str, c: &SetComparison) -> String {
    let mut out = format!(
        "{label:<11} expected {:>3}  actual {:>3}  precision {:>6}  recall {:>6}\n",
        c.expected,
        c.actual,
        percent(c.precision()),
        percent(c.recall())
    );
    for m in &c.missing {
        out.push_str(&format!("  missing    {m}\n"));
    }
    for u in &c.unexpected {
        out.push_str(&format!("  unexpected {u}\n"));
    }
    out
}
