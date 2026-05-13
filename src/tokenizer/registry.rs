//! Tokenizer enum + repo-id table.

use std::fmt;
use std::str::FromStr;

use super::TokenizerError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tokenizer {
    Cl100k,
    O200k,
    Claude,
    Llama3,
    Qwen3,
}

impl Tokenizer {
    /// Canonical short name used in TOML config and the MCP `tokenizer` arg.
    pub fn as_str(self) -> &'static str {
        match self {
            Tokenizer::Cl100k => "cl100k",
            Tokenizer::O200k => "o200k",
            Tokenizer::Claude => "claude",
            Tokenizer::Llama3 => "llama3",
            Tokenizer::Qwen3 => "qwen3",
        }
    }

    /// HuggingFace `(repo_id, filename)` pair used by the downloader.
    pub fn hf_source(self) -> (&'static str, &'static str) {
        match self {
            Tokenizer::Cl100k => ("Xenova/gpt-4", "tokenizer.json"),
            Tokenizer::O200k => ("Xenova/gpt-4o", "tokenizer.json"),
            Tokenizer::Claude => ("Xenova/claude-tokenizer", "tokenizer.json"),
            Tokenizer::Llama3 => ("meta-llama/Meta-Llama-3-8B", "tokenizer.json"),
            Tokenizer::Qwen3 => ("Qwen/Qwen2.5-0.5B", "tokenizer.json"),
        }
    }

    /// All known variants, in declaration order. Used by integration tests.
    pub const ALL: [Tokenizer; 5] = [
        Tokenizer::Cl100k,
        Tokenizer::O200k,
        Tokenizer::Claude,
        Tokenizer::Llama3,
        Tokenizer::Qwen3,
    ];
}

impl fmt::Display for Tokenizer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Tokenizer {
    type Err = TokenizerError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "cl100k" => Ok(Tokenizer::Cl100k),
            "o200k" => Ok(Tokenizer::O200k),
            "claude" => Ok(Tokenizer::Claude),
            "llama3" => Ok(Tokenizer::Llama3),
            "qwen3" => Ok(Tokenizer::Qwen3),
            other => Err(TokenizerError::UnknownFamily(other.to_string())),
        }
    }
}

impl serde::Serialize for Tokenizer {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for Tokenizer {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_all_known_families() {
        for t in Tokenizer::ALL {
            let parsed: Tokenizer = t.as_str().parse().unwrap();
            assert_eq!(parsed, t);
        }
    }

    #[test]
    fn unknown_family_errors() {
        let result: Result<Tokenizer, _> = "gpt-5".parse();
        assert!(matches!(result, Err(TokenizerError::UnknownFamily(s)) if s == "gpt-5"));
    }

    #[test]
    fn hf_source_is_non_empty() {
        for t in Tokenizer::ALL {
            let (repo, file) = t.hf_source();
            assert!(!repo.is_empty(), "repo for {t} is empty");
            assert!(!file.is_empty(), "file for {t} is empty");
            assert_eq!(file, "tokenizer.json");
        }
    }

    #[test]
    fn serde_round_trip_via_string() {
        let json = serde_json::to_string(&Tokenizer::O200k).unwrap();
        assert_eq!(json, "\"o200k\"");
        let parsed: Tokenizer = serde_json::from_str("\"claude\"").unwrap();
        assert_eq!(parsed, Tokenizer::Claude);
    }
}
