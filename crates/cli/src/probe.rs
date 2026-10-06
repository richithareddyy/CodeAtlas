//! Ground truth for test selection, by observation.
//!
//! For each probed function, a `panic!` is inserted at the start of its
//! body, the test suite is built and run, and the tests that fail (and
//! passed without the probe) are recorded: they are the tests that
//! executed the function. The repository is copied to a scratch directory
//! first; the original is never modified.
//!
//! This builds and runs the repository's tests, i.e. executes its code.
//! Use it only on repositories you trust.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use codeatlas_analyzer::model::{SymbolKind, TargetKind};
use codeatlas_analyzer::parser::RustParser;
use codeatlas_analyzer::test_evaluation::{
    source_hash, Probe, ProbeStatus, TestGroundTruth, PROBE_METHOD, UNMAPPED,
};
use codeatlas_analyzer::RepositoryAnalysis;
use serde_json::Value;

pub struct ProbeOptions {
    /// Symbols to probe; when empty, `sample` functions are picked.
    pub symbols: Vec<String>,
    pub sample: usize,
    pub seed: u64,
    /// Per test binary run.
    pub timeout: Duration,
}

/// Inserted at the start of a probed body. Kept on the same line so that
/// line numbers do not change.
const PROBE: &str = " panic!(\"codeatlas probe\"); ";

struct TestBinary {
    executable: PathBuf,
    src_path: PathBuf,
    manifest_dir: PathBuf,
    target: String,
}

struct BinaryRun {
    /// Test name to whether it passed (ignored tests are left out).
    results: BTreeMap<String, bool>,
    complete: bool,
}

pub fn probe(analysis: &RepositoryAnalysis, options: &ProbeOptions) -> Result<TestGroundTruth> {
    let root = &analysis.repository.root;
    let scratch = tempfile::Builder::new()
        .prefix("codeatlas-probe-")
        .tempdir()
        .context("cannot create a scratch directory")?;
    let work = scratch.path().join("repo");
    copy_tree(root, &work)?;
    let target_dir = scratch.path().join("target");
    let started = Instant::now();

    eprintln!("Building and running the test suite (baseline)...");
    let Some(binaries) = build(&work, &target_dir)? else {
        bail!("the repository's tests do not build; see the cargo output above");
    };
    let mapper = TestIds::new(analysis, &work);
    let mut baseline: BTreeMap<String, bool> = BTreeMap::new();
    for binary in &binaries {
        let run = run_binary(binary, options.timeout)?;
        if !run.complete {
            bail!(
                "{} did not complete in the baseline run",
                binary.executable.display()
            );
        }
        for (name, passed) in run.results {
            baseline.insert(mapper.id(binary, &name), passed);
        }
    }
    eprintln!(
        "Baseline: {} tests in {} binaries ({:.0} s)",
        baseline.len(),
        binaries.len(),
        started.elapsed().as_secs_f64()
    );

    let targets = targets(analysis, &work, options)?;
    let mut probes = Vec::new();
    for (i, target) in targets.iter().enumerate() {
        let probe_started = Instant::now();
        let path = work.join(&target.file);
        let original = fs::read_to_string(&path)?;
        let mut mutated = original.clone();
        mutated.insert_str(target.offset, PROBE);
        fs::write(&path, &mutated)?;
        let outcome = (|| -> Result<Probe> {
            let Some(binaries) = build(&work, &target_dir)? else {
                return Ok(Probe {
                    symbol: target.symbol.clone(),
                    status: ProbeStatus::BuildFailed,
                    failing: Vec::new(),
                });
            };
            let mut failing = BTreeSet::new();
            let mut complete = true;
            for binary in &binaries {
                let run = run_binary(binary, options.timeout)?;
                complete &= run.complete;
                for (name, passed) in run.results {
                    let id = mapper.id(binary, &name);
                    if !passed && baseline.get(&id) == Some(&true) {
                        failing.insert(id);
                    }
                }
            }
            Ok(Probe {
                symbol: target.symbol.clone(),
                status: if complete {
                    ProbeStatus::Ok
                } else {
                    ProbeStatus::Incomplete
                },
                failing: failing.into_iter().collect(),
            })
        })();
        fs::write(&path, &original)?;
        let probe = outcome?;
        eprintln!(
            "[{}/{}] {} -> {:?}, {} failing ({:.0} s)",
            i + 1,
            targets.len(),
            probe.symbol,
            probe.status,
            probe.failing.len(),
            probe_started.elapsed().as_secs_f64()
        );
        probes.push(probe);
    }

    Ok(TestGroundTruth {
        repository: analysis.repository.name.clone(),
        commit: analysis.repository.head_sha.clone(),
        source_hash: source_hash(analysis),
        method: PROBE_METHOD.to_string(),
        toolchain: toolchain(),
        baseline_failures: baseline
            .iter()
            .filter(|(_, passed)| !**passed)
            .map(|(id, _)| id.clone())
            .collect(),
        tests: baseline.into_keys().collect(),
        probes,
    })
}

/// Maps a test binary's test names to symbol IDs.
struct TestIds {
    /// Crate root file (relative to the repository) to crate name.
    crates: BTreeMap<String, String>,
    tests: HashSet<String>,
    work: PathBuf,
}

impl TestIds {
    fn new(analysis: &RepositoryAnalysis, work: &Path) -> Self {
        Self {
            crates: analysis
                .crates
                .iter()
                .map(|c| (c.root_file.clone(), c.name.clone()))
                .collect(),
            tests: analysis
                .files
                .iter()
                .flat_map(|f| &f.symbols)
                .filter(|s| s.is_test)
                .map(|s| s.id.to_string())
                .collect(),
            work: work.to_path_buf(),
        }
    }

    fn id(&self, binary: &TestBinary, name: &str) -> String {
        let crate_name = binary
            .src_path
            .strip_prefix(&self.work)
            .ok()
            .and_then(|rel| self.crates.get(&rel.to_string_lossy().replace('\\', "/")))
            .cloned()
            .unwrap_or_else(|| binary.target.replace('-', "_"));
        let id = format!("fn:{crate_name}::{name}");
        if self.tests.contains(&id) {
            id
        } else {
            format!("{UNMAPPED}{crate_name}::{name}")
        }
    }
}

struct Target {
    symbol: String,
    file: String,
    /// Byte offset just after the body's opening brace.
    offset: usize,
}

/// The functions to probe: those requested, or a deterministic sample of
/// non-test functions and methods in library and binary crates.
fn targets(
    analysis: &RepositoryAnalysis,
    work: &Path,
    options: &ProbeOptions,
) -> Result<Vec<Target>> {
    let production: HashSet<&str> = analysis
        .crates
        .iter()
        .filter(|c| matches!(c.kind, TargetKind::Lib | TargetKind::Bin))
        .map(|c| c.name.as_str())
        .collect();
    let mut candidates: Vec<(&str, &codeatlas_analyzer::model::Symbol)> = analysis
        .files
        .iter()
        .flat_map(|f| f.symbols.iter().map(move |s| (f.crate_name.as_str(), s)))
        .filter(|(_, s)| {
            if options.symbols.is_empty() {
                matches!(s.kind, SymbolKind::Function | SymbolKind::Method)
                    && !s.is_test
                    && !s.cfg_test
            } else {
                options.symbols.iter().any(|wanted| wanted == s.id.as_str())
            }
        })
        .filter(|(krate, _)| !options.symbols.is_empty() || production.contains(krate))
        .collect();
    candidates.sort_by(|a, b| a.1.id.cmp(&b.1.id));
    if options.symbols.is_empty() {
        shuffle(&mut candidates, options.seed);
    } else if candidates.len() < options.symbols.len() {
        let found: HashSet<&str> = candidates.iter().map(|(_, s)| s.id.as_str()).collect();
        let missing: Vec<&String> = options
            .symbols
            .iter()
            .filter(|s| !found.contains(s.as_str()))
            .collect();
        bail!("unknown symbols: {missing:?}");
    }

    let mut parser = RustParser::new()?;
    let limit = if options.symbols.is_empty() {
        options.sample
    } else {
        usize::MAX
    };
    let mut out = Vec::new();
    for (_, symbol) in candidates {
        if out.len() >= limit {
            break;
        }
        let src = fs::read_to_string(work.join(&symbol.file))?;
        // Trait declarations without a body, items generated by macros:
        // nothing to probe.
        if let Some(offset) = body_offset(&mut parser, &src, symbol)? {
            out.push(Target {
                symbol: symbol.id.to_string(),
                file: symbol.file.clone(),
                offset,
            });
        }
    }
    out.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    Ok(out)
}

fn body_offset(
    parser: &mut RustParser,
    src: &str,
    symbol: &codeatlas_analyzer::model::Symbol,
) -> Result<Option<usize>> {
    let tree = parser.parse(src, &symbol.file)?;
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.kind() == "function_item"
            && node.start_position().row as u32 + 1 == symbol.span.start_line
            && node
                .child_by_field_name("name")
                .is_some_and(|n| src[n.byte_range()] == symbol.name)
        {
            return Ok(node
                .child_by_field_name("body")
                .map(|body| body.start_byte() + 1));
        }
        let mut cursor = node.walk();
        stack.extend(node.children(&mut cursor));
    }
    Ok(None)
}

/// Builds the tests; `None` when the build fails.
fn build(work: &Path, target_dir: &Path) -> Result<Option<Vec<TestBinary>>> {
    let output = Command::new("cargo")
        .args(["test", "--workspace", "--no-run", "--message-format=json"])
        .current_dir(work)
        .env("CARGO_TARGET_DIR", target_dir)
        .env("CARGO_TERM_COLOR", "never")
        // A probe makes the rest of the body unreachable; lints denied by
        // the repository must not turn that into a build failure.
        .env("RUSTFLAGS", "--cap-lints=warn")
        .stderr(Stdio::piped())
        .output()
        .context("cannot run cargo")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(5).collect();
        tracing::debug!(output = %tail.join("\n"), "build failed");
        return Ok(None);
    }
    let mut seen = HashSet::new();
    let mut binaries = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if message["reason"] != "compiler-artifact" || message["profile"]["test"] != true {
            continue;
        }
        let Some(executable) = message["executable"].as_str() else {
            continue;
        };
        if !seen.insert(executable.to_string()) {
            continue;
        }
        let manifest = PathBuf::from(message["manifest_path"].as_str().unwrap_or_default());
        binaries.push(TestBinary {
            executable: executable.into(),
            src_path: message["target"]["src_path"]
                .as_str()
                .unwrap_or_default()
                .into(),
            manifest_dir: manifest.parent().map(Path::to_path_buf).unwrap_or_default(),
            target: message["target"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        });
    }
    binaries.sort_by(|a, b| a.executable.cmp(&b.executable));
    Ok(Some(binaries))
}

fn run_binary(binary: &TestBinary, timeout: Duration) -> Result<BinaryRun> {
    let mut child = Command::new(&binary.executable)
        .current_dir(&binary.manifest_dir)
        .env("CARGO_MANIFEST_DIR", &binary.manifest_dir)
        .env("RUST_BACKTRACE", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("cannot run {}", binary.executable.display()))?;
    let mut stdout = child.stdout.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let text = reader.join().unwrap_or_default();
    let mut results = BTreeMap::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("test ") else {
            continue;
        };
        let Some((name, outcome)) = rest.split_once(" ... ") else {
            continue;
        };
        if outcome.starts_with("ok") {
            results.insert(name.to_string(), true);
        } else if outcome.starts_with("FAILED") {
            results.insert(name.to_string(), false);
        }
    }
    // libtest exits with 101 when tests fail; anything else (a signal, a
    // timeout) means results may be missing.
    let complete = status.is_some_and(|s| s.success() || s.code() == Some(101));
    Ok(BinaryRun { results, complete })
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git" || name == "target" {
            continue;
        }
        let target = to.join(&name);
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &target)?;
        } else if kind.is_symlink() {
            // Copy what the link points to, if anything.
            if let Ok(meta) = fs::metadata(entry.path()) {
                if meta.is_file() {
                    fs::copy(entry.path(), &target)?;
                } else if meta.is_dir() {
                    copy_tree(&entry.path(), &target)?;
                }
            }
        }
    }
    Ok(())
}

fn toolchain() -> String {
    Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Deterministic Fisher-Yates shuffle (xorshift64*).
fn shuffle<T>(items: &mut [T], seed: u64) {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    for i in (1..items.len()).rev() {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let r = state.wrapping_mul(0x2545_F491_4F6C_DD1D);
        items.swap(i, (r % (i as u64 + 1)) as usize);
    }
}

#[cfg(test)]
mod tests {
    use super::shuffle;

    #[test]
    fn shuffles_deterministically() {
        let mut a: Vec<u32> = (0..20).collect();
        let mut b = a.clone();
        shuffle(&mut a, 7);
        shuffle(&mut b, 7);
        assert_eq!(a, b);
        assert_ne!(a, (0..20).collect::<Vec<_>>());
        let mut c: Vec<u32> = (0..20).collect();
        shuffle(&mut c, 8);
        assert_ne!(a, c);
    }
}
