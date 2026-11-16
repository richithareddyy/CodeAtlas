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
    source_hash, Probe, ProbeStatus, RunSeconds, TestGroundTruth, WorkspacePackage, PROBE_METHOD,
    UNMAPPED,
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
    package: String,
}

struct BinaryRun {
    /// Test name to whether it passed (ignored tests are left out).
    results: BTreeMap<String, bool>,
    /// Test name to its time in seconds, where libtest reported one.
    seconds: BTreeMap<String, f64>,
    complete: bool,
}

/// One run of every test binary, by test ID.
struct SuiteRun {
    results: BTreeMap<String, bool>,
    seconds: BTreeMap<String, f64>,
    /// Test ID to the Cargo package of its binary.
    packages: BTreeMap<String, String>,
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
    let packages = workspace_packages(&work)?;

    eprintln!("Building and running the test suite twice (baseline)...");
    let build_started = Instant::now();
    let Some(binaries) = build(&work, &target_dir, &packages)? else {
        bail!("the repository's tests do not build; see the cargo output above");
    };
    let build_seconds = build_started.elapsed().as_secs_f64();
    let mapper = TestIds::new(analysis, &work);
    let run_started = Instant::now();
    let first = run_suite(&binaries, &mapper, options.timeout)?;
    let run_seconds = run_started.elapsed().as_secs_f64();
    let second = run_suite(&binaries, &mapper, options.timeout)?;
    if !first.complete || !second.complete {
        bail!("a test binary did not complete in the baseline runs");
    }
    let ids: BTreeSet<&String> = first.results.keys().chain(second.results.keys()).collect();
    let mut baseline: BTreeMap<String, bool> = BTreeMap::new();
    let mut flaky = Vec::new();
    for id in ids {
        match (first.results.get(id), second.results.get(id)) {
            (Some(a), Some(b)) if a == b => {
                baseline.insert(id.clone(), *a);
            }
            _ => flaky.push(id.clone()),
        }
    }
    let test_seconds: BTreeMap<String, f64> = baseline
        .keys()
        .filter_map(|id| {
            let times: Vec<f64> = [&first, &second]
                .iter()
                .filter_map(|run| run.seconds.get(id).copied())
                .collect();
            (!times.is_empty())
                .then(|| (id.clone(), times.iter().sum::<f64>() / times.len() as f64))
        })
        .collect();
    let test_packages = first.packages.clone();
    eprintln!(
        "Baseline: {} tests in {} binaries, {} flaky ({:.0} s)",
        baseline.len(),
        binaries.len(),
        flaky.len(),
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
            let build_started = Instant::now();
            let Some(binaries) = build(&work, &target_dir, &packages)? else {
                return Ok(Probe {
                    symbol: target.symbol.clone(),
                    status: ProbeStatus::BuildFailed,
                    failing: Vec::new(),
                    seconds: None,
                });
            };
            let build = build_started.elapsed().as_secs_f64();
            let run_started = Instant::now();
            let run = run_suite(&binaries, &mapper, options.timeout)?;
            let failing = run
                .results
                .iter()
                .filter(|(id, passed)| !**passed && baseline.get(*id) == Some(&true))
                .map(|(id, _)| id.clone())
                .collect();
            Ok(Probe {
                symbol: target.symbol.clone(),
                status: if run.complete {
                    ProbeStatus::Ok
                } else {
                    ProbeStatus::Incomplete
                },
                failing,
                seconds: Some(RunSeconds {
                    build,
                    run: run_started.elapsed().as_secs_f64(),
                }),
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
        tests: baseline
            .keys()
            .chain(flaky.iter())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        flaky,
        test_seconds,
        test_packages,
        packages: packages
            .iter()
            .map(|p| WorkspacePackage {
                name: p.name.clone(),
                dependencies: p.dependencies.clone(),
            })
            .collect(),
        baseline_seconds: Some(RunSeconds {
            build: build_seconds,
            run: run_seconds,
        }),
        probes,
    })
}

fn run_suite(binaries: &[TestBinary], mapper: &TestIds, timeout: Duration) -> Result<SuiteRun> {
    let mut suite = SuiteRun {
        results: BTreeMap::new(),
        seconds: BTreeMap::new(),
        packages: BTreeMap::new(),
        complete: true,
    };
    for binary in binaries {
        let run = run_binary(binary, timeout)?;
        suite.complete &= run.complete;
        for (name, passed) in run.results {
            let id = mapper.id(binary, &name);
            suite.packages.insert(id.clone(), binary.package.clone());
            suite.results.insert(id, passed);
        }
        for (name, seconds) in run.seconds {
            suite.seconds.insert(mapper.id(binary, &name), seconds);
        }
    }
    Ok(suite)
}

struct Package {
    name: String,
    manifest_dir: PathBuf,
    /// Workspace packages this one depends on.
    dependencies: Vec<String>,
}

fn workspace_packages(work: &Path) -> Result<Vec<Package>> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(work)
        .output()
        .context("cannot run cargo metadata")?;
    if !output.status.success() {
        bail!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let metadata: Value = serde_json::from_slice(&output.stdout)?;
    let empty = Vec::new();
    let members = metadata["packages"].as_array().unwrap_or(&empty);
    let names: HashSet<&str> = members.iter().filter_map(|p| p["name"].as_str()).collect();
    Ok(members
        .iter()
        .map(|p| {
            let mut dependencies: Vec<String> = p["dependencies"]
                .as_array()
                .unwrap_or(&empty)
                .iter()
                .filter_map(|d| d["name"].as_str())
                .filter(|d| names.contains(d))
                .map(str::to_string)
                .collect();
            dependencies.sort();
            dependencies.dedup();
            Package {
                name: p["name"].as_str().unwrap_or_default().to_string(),
                manifest_dir: Path::new(p["manifest_path"].as_str().unwrap_or_default())
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_default(),
                dependencies,
            }
        })
        .collect())
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

    fn crate_name(&self, binary: &TestBinary) -> String {
        binary
            .src_path
            .strip_prefix(&self.work)
            .ok()
            .and_then(|rel| self.crates.get(&rel.to_string_lossy().replace('\\', "/")))
            .cloned()
            .unwrap_or_else(|| binary.target.replace('-', "_"))
    }

    fn id(&self, binary: &TestBinary, name: &str) -> String {
        let crate_name = self.crate_name(binary);
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
fn build(work: &Path, target_dir: &Path, packages: &[Package]) -> Result<Option<Vec<TestBinary>>> {
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
        let manifest_dir = manifest.parent().map(Path::to_path_buf).unwrap_or_default();
        let package = packages
            .iter()
            .find(|p| p.manifest_dir == manifest_dir)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        binaries.push(TestBinary {
            package,
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

/// Runs a test binary with libtest's JSON output, which reports each test's
/// time. A custom harness rejects those flags: it is run again plainly and
/// its text output read, and if it lists no tests it counts as one test
/// that passes or fails with its exit status.
fn run_binary(binary: &TestBinary, timeout: Duration) -> Result<BinaryRun> {
    let json_args = [
        "-Z",
        "unstable-options",
        "--format",
        "json",
        "--report-time",
    ];
    let (text, status) = execute(binary, &json_args, timeout)?;
    let mut run = BinaryRun {
        results: BTreeMap::new(),
        seconds: BTreeMap::new(),
        complete: completed(status),
    };
    let mut libtest = false;
    for line in text.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        libtest |= event["type"] == "suite";
        if event["type"] != "test" {
            continue;
        }
        let Some(name) = event["name"].as_str() else {
            continue;
        };
        let passed = match event["event"].as_str() {
            Some("ok") => true,
            Some("failed") => false,
            _ => continue,
        };
        run.results.insert(name.to_string(), passed);
        if let Some(seconds) = event["exec_time"].as_f64() {
            run.seconds.insert(name.to_string(), seconds);
        }
    }
    if libtest {
        return Ok(run);
    }

    let (text, status) = execute(binary, &[], timeout)?;
    run.complete = completed(status);
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("test ") else {
            continue;
        };
        let Some((name, outcome)) = rest.split_once(" ... ") else {
            continue;
        };
        if outcome.starts_with("ok") {
            run.results.insert(name.to_string(), true);
        } else if outcome.starts_with("FAILED") {
            run.results.insert(name.to_string(), false);
        }
    }
    if run.results.is_empty() && status.is_some() {
        run.results
            .insert(HARNESS.to_string(), status.is_some_and(|s| s.success()));
        run.complete = true;
    }
    Ok(run)
}

/// Name given to a custom-harness binary that lists no tests.
const HARNESS: &str = "[harness]";

/// libtest exits with 101 when tests fail; anything else (a signal, a
/// timeout) means results may be missing.
fn completed(status: Option<std::process::ExitStatus>) -> bool {
    status.is_some_and(|s| s.success() || s.code() == Some(101))
}

/// Runs the binary with `args`; `None` as status after a timeout.
fn execute(
    binary: &TestBinary,
    args: &[&str],
    timeout: Duration,
) -> Result<(String, Option<std::process::ExitStatus>)> {
    let mut child = Command::new(&binary.executable)
        .args(args)
        .current_dir(&binary.manifest_dir)
        .env("CARGO_MANIFEST_DIR", &binary.manifest_dir)
        .env("RUST_BACKTRACE", "0")
        // Enables libtest's JSON output and test times at run time; the
        // code under test was compiled normally.
        .env("RUSTC_BOOTSTRAP", "1")
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
    Ok((reader.join().unwrap_or_default(), status))
}

pub(crate) fn copy_tree(from: &Path, to: &Path) -> Result<()> {
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
pub(crate) fn shuffle<T>(items: &mut [T], seed: u64) {
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
