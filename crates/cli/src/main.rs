use std::fs;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use codeatlas_analyzer::evaluation::{evaluate, Evaluation, GroundTruth, SetComparison};
use codeatlas_analyzer::ingest::{default_clone_dir, DiscoveryOptions, IngestOptions};
use codeatlas_analyzer::model::SymbolKind;
use codeatlas_analyzer::parser::RustParser;
use codeatlas_analyzer::{analyze_source, RepoSource, RepositoryAnalysis};
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

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("warn,codeatlas_analyzer=info")),
        )
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
            let options = IngestOptions {
                clone_dir: clone_dir.unwrap_or_else(default_clone_dir),
                discovery: DiscoveryOptions {
                    respect_gitignore: !no_gitignore,
                    ..Default::default()
                },
            };
            let analysis = analyze_source(&RepoSource::parse(&source), &options)
                .with_context(|| format!("failed to analyse {source}"))?;
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
        Command::Ast { file } => {
            let src = fs::read_to_string(&file)
                .with_context(|| format!("failed to read {}", file.display()))?;
            let tree = RustParser::new()?.parse(&src, &file.to_string_lossy())?;
            writeln!(std::io::stdout(), "{}", tree.root_node().to_sexp())?;
        }
    }
    Ok(())
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
