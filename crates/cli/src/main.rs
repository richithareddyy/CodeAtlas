use std::fs;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
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
    },
    /// Print the tree-sitter syntax tree of a Rust file (for extending the extractor).
    Ast { file: PathBuf },
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Summary,
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
                Format::Json => serde_json::to_string_pretty(&analysis)?,
            };
            match output {
                Some(path) => fs::write(&path, rendered)
                    .with_context(|| format!("failed to write {}", path.display()))?,
                None => writeln!(std::io::stdout(), "{rendered}")?,
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
    line(
        "Parsing",
        format!(
            "{} files, {} LOC, {} with syntax errors, parse {:.1} ms, total {:.1} ms",
            s.files_analyzed, s.loc_analyzed, s.files_with_syntax_errors, s.parse_ms, s.total_ms
        ),
    );
    out.trim_end().to_string()
}
