//! End-to-end extractive summarizer test against a real-feeling document.

use rover::summarizer::backend::{CompactMode, CompactOpts, Style, SummarizerBackend};
use rover::summarizer::extractive::ExtractiveBackend;
use rover::tokenizer::Tokenizer;

fn opts(mode: CompactMode, target: Option<usize>) -> CompactOpts {
    CompactOpts {
        mode,
        style: Style::Bullet,
        target_tokens: target,
        focus: None,
        preserve: vec![],
        backend_name: "default".to_string(),
    }
}

#[tokio::test]
async fn extractive_three_sentence_caps_to_target_tokens() {
    let content = "\
The Midnight Network is a privacy-preserving blockchain platform. \
It uses zero-knowledge proofs for transaction privacy. \
The network's native token is NIGHT, used for staking and governance.";
    let be = ExtractiveBackend::new("default", Tokenizer::O200k);
    let full = be
        .compact(content, &opts(CompactMode::Extractive, None))
        .await
        .unwrap();
    let bounded = be
        .compact(content, &opts(CompactMode::Extractive, Some(15)))
        .await
        .unwrap();
    assert!(
        full.len() > bounded.len(),
        "bounded={bounded:?} full={full:?}"
    );
}

#[tokio::test]
async fn headlines_emits_one_section_per_heading() {
    let content = "\
# Overview\n\
Midnight is a layer-1 privacy-preserving blockchain. It uses ZK proofs.\n\
\n\
# Tokens\n\
NIGHT is the native token. STAR is the unit of account.\n\
\n\
# Networks\n\
Devnet, testnet, and mainnet are all supported.\n";
    let be = ExtractiveBackend::new("default", Tokenizer::O200k);
    let out = be
        .compact(content, &opts(CompactMode::Headlines, None))
        .await
        .unwrap();
    // Three top-level headings → three line-start '# ' markers in output.
    // Prepending a newline lets us match the first heading the same way as
    // the rest, and is robust to heading text containing '#' (e.g. "C#").
    let probe = format!("\n{out}");
    let heading_count = probe.matches("\n# ").count();
    assert!(heading_count >= 3, "expected ≥3 '\\n# ' headings in {out}");
    assert!(out.contains("Overview"));
    assert!(out.contains("Tokens"));
    assert!(out.contains("Networks"));
}
