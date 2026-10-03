//! Reads source lines for "jump to code" in the UI, confined to the
//! repository's working tree.

use std::path::{Component, Path};

/// Lines returned per request at most.
pub const MAX_LINES: u32 = 400;
/// Files larger than this are not served.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snippet {
    pub start_line: u32,
    pub end_line: u32,
    pub total_lines: u32,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceError {
    /// The path is absolute, contains `..`, or leaves the repository.
    OutsideRepository,
    NotFound,
    TooLarge,
    InvalidRange(String),
    Unreadable(String),
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceError::OutsideRepository => {
                f.write_str("file must be a relative path inside the repository")
            }
            SourceError::NotFound => f.write_str("file not found in the repository working tree"),
            SourceError::TooLarge => f.write_str("file is too large to display"),
            SourceError::InvalidRange(why) => write!(f, "invalid line range: {why}"),
            SourceError::Unreadable(why) => write!(f, "cannot read file: {why}"),
        }
    }
}

/// Lines `start..=end` (1-based) of `file` inside `root`. The range is
/// clamped to the file length and to [`MAX_LINES`].
pub fn read_snippet(root: &Path, file: &str, start: u32, end: u32) -> Result<Snippet, SourceError> {
    if start == 0 || end < start {
        return Err(SourceError::InvalidRange(format!(
            "start {start} must be at least 1 and not after end {end}"
        )));
    }
    let relative = Path::new(file);
    let plain = relative
        .components()
        .all(|c| matches!(c, Component::Normal(_)));
    if file.is_empty() || !plain {
        return Err(SourceError::OutsideRepository);
    }
    let root = root
        .canonicalize()
        .map_err(|e| SourceError::Unreadable(e.to_string()))?;
    let path = root
        .join(relative)
        .canonicalize()
        .map_err(|_| SourceError::NotFound)?;
    // Symlinks may still point elsewhere; check the resolved location.
    if !path.starts_with(&root) {
        return Err(SourceError::OutsideRepository);
    }
    let metadata = std::fs::metadata(&path).map_err(|e| SourceError::Unreadable(e.to_string()))?;
    if !metadata.is_file() {
        return Err(SourceError::NotFound);
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(SourceError::TooLarge);
    }
    let content =
        std::fs::read_to_string(&path).map_err(|e| SourceError::Unreadable(e.to_string()))?;
    let all: Vec<&str> = content.lines().collect();
    let total = all.len() as u32;
    if start > total.max(1) {
        return Err(SourceError::InvalidRange(format!(
            "start {start} is past the end of the file ({total} lines)"
        )));
    }
    let end = end.min(total).min(start + MAX_LINES - 1);
    let lines = all[(start - 1) as usize..end as usize]
        .iter()
        .map(|l| l.to_string())
        .collect();
    Ok(Snippet {
        start_line: start,
        end_line: end,
        total_lines: total,
        lines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "a\nb\nc\nd\n").unwrap();
        dir
    }

    #[test]
    fn reads_and_clamps_ranges() {
        let dir = repo();
        let s = read_snippet(dir.path(), "src/lib.rs", 2, 3).unwrap();
        assert_eq!(s.lines, vec!["b", "c"]);
        assert_eq!(s.total_lines, 4);
        let clamped = read_snippet(dir.path(), "src/lib.rs", 3, 99).unwrap();
        assert_eq!((clamped.end_line, clamped.lines.len()), (4, 2));
    }

    #[test]
    fn refuses_paths_outside_the_repository() {
        let dir = repo();
        for bad in [
            "../secret",
            "/etc/passwd",
            "src/../../x",
            "",
            "./src/lib.rs",
        ] {
            assert_eq!(
                read_snippet(dir.path(), bad, 1, 1),
                Err(SourceError::OutsideRepository),
                "{bad}"
            );
        }
    }

    #[test]
    fn refuses_symlinks_that_escape() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.rs"), "token").unwrap();
        let dir = repo();
        std::os::unix::fs::symlink(outside.path().join("secret.rs"), dir.path().join("link.rs"))
            .unwrap();
        assert_eq!(
            read_snippet(dir.path(), "link.rs", 1, 1),
            Err(SourceError::OutsideRepository)
        );
    }

    #[test]
    fn rejects_bad_ranges_and_missing_files() {
        let dir = repo();
        assert!(matches!(
            read_snippet(dir.path(), "src/lib.rs", 0, 1),
            Err(SourceError::InvalidRange(_))
        ));
        assert!(matches!(
            read_snippet(dir.path(), "src/lib.rs", 9, 10),
            Err(SourceError::InvalidRange(_))
        ));
        assert_eq!(
            read_snippet(dir.path(), "src/none.rs", 1, 1),
            Err(SourceError::NotFound)
        );
    }
}
