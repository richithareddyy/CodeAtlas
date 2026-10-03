use std::path::Path;

use serde::{Deserialize, Serialize};

/// Languages CodeAtlas recognises for repository statistics. Only languages
/// for which [`Language::is_analyzable`] returns true are parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Rust,
    C,
    Cpp,
    CSharp,
    Go,
    Java,
    JavaScript,
    Kotlin,
    Python,
    Ruby,
    Shell,
    Svelte,
    Swift,
    TypeScript,
}

impl Language {
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?;
        Some(match ext {
            "rs" => Language::Rust,
            "c" | "h" => Language::C,
            "cc" | "cpp" | "cxx" | "hpp" | "hh" => Language::Cpp,
            "cs" => Language::CSharp,
            "go" => Language::Go,
            "java" => Language::Java,
            "js" | "mjs" | "cjs" | "jsx" => Language::JavaScript,
            "kt" | "kts" => Language::Kotlin,
            "py" => Language::Python,
            "rb" => Language::Ruby,
            "sh" | "bash" => Language::Shell,
            "svelte" => Language::Svelte,
            "swift" => Language::Swift,
            "ts" | "tsx" | "mts" | "cts" => Language::TypeScript,
            _ => return None,
        })
    }

    pub fn is_analyzable(self) -> bool {
        matches!(self, Language::Rust)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_by_extension() {
        assert_eq!(
            Language::from_path(Path::new("src/lib.rs")),
            Some(Language::Rust)
        );
        assert_eq!(
            Language::from_path(Path::new("a/b.tsx")),
            Some(Language::TypeScript)
        );
        assert_eq!(Language::from_path(Path::new("README.md")), None);
        assert_eq!(Language::from_path(Path::new("Makefile")), None);
    }
}
