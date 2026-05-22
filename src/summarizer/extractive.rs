//! Extractive summarizer — TextRank.
//!
//! Pipeline (PRD §7.2):
//! 1. Sentence-split via `unicode-segmentation` (UAX #29).
//! 2. Tokenize per sentence (lowercased Unicode words).
//! 3. TF-IDF per sentence (within-document IDF).
//! 4. Cosine-similarity edges (drop below 0.1).
//! 5. PageRank — damping 0.85, max_iter 50, tol 1e-4.
//! 6. Mode-specific selection (Extractive | Headlines).
//!    Abstractive mode delegates to the cloud backend; this module
//!    never sees it.
//!
//! All paths are pure (no I/O, no async). The trait wrapper at the
//! bottom satisfies `SummarizerBackend`.

use std::collections::HashMap;

use async_trait::async_trait;
use unicode_segmentation::UnicodeSegmentation;

use crate::summarizer::backend::{CompactMode, CompactOpts, Style, SummarizerBackend};
use crate::summarizer::error::BackendError;
use crate::tokenizer::{self, Tokenizer};

/// PageRank tuning. Pinned to design §2 §5.
const PAGERANK_DAMPING: f32 = 0.85;
const PAGERANK_MAX_ITER: usize = 50;
const PAGERANK_TOL: f32 = 1e-4;
const SIMILARITY_FLOOR: f32 = 0.1;

/// One sentence keeping its source offset for stable re-ordering.
#[derive(Debug, Clone)]
pub(super) struct Sentence {
    pub span_start: usize,
    pub text: String,
}

pub(super) fn split_sentences(content: &str) -> Vec<Sentence> {
    let mut out = Vec::new();
    for (offset, s) in content.split_sentence_bound_indices() {
        let trimmed = s.trim();
        if trimmed.chars().count() < 3 {
            continue;
        }
        // Skip lines that look like markdown ATX headings. UAX #29 sentence
        // segmentation classifies "# Title" as a stand-alone sentence; if we
        // kept it, the heading text would compete for selection in Extractive
        // mode and pollute Headlines mode's section buckets.
        if parse_atx_heading(trimmed).is_some() {
            continue;
        }
        out.push(Sentence {
            span_start: offset,
            text: trimmed.to_string(),
        });
    }
    out
}

#[cfg(test)]
mod sentence_tests {
    use super::*;

    #[test]
    fn split_produces_three_sentences_from_simple_paragraph() {
        let text = "Hello world. How are you? Fine!";
        let s = split_sentences(text);
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].text, "Hello world.");
        assert_eq!(s[1].text, "How are you?");
        assert_eq!(s[2].text, "Fine!");
    }

    #[test]
    fn split_skips_short_fragments() {
        let text = "Hi. This is a longer sentence.";
        let s = split_sentences(text);
        // "Hi." is 3 chars, sits exactly at the >=3 threshold.
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn split_handles_unicode_punctuation() {
        let text = "Привет мир. Это тест.";
        let s = split_sentences(text);
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn split_preserves_byte_offsets() {
        let text = "First sentence. Second sentence here.";
        let s = split_sentences(text);
        assert!(s[0].span_start < s[1].span_start);
    }

    #[test]
    fn split_returns_empty_for_empty_input() {
        assert!(split_sentences("").is_empty());
    }
}

/// Lowercased Unicode word tokens. Filters punctuation and whitespace.
pub(super) fn tokenize(s: &str) -> Vec<String> {
    s.unicode_words().map(str::to_lowercase).collect()
}

/// Returns (per-sentence L2-normalized TF-IDF vectors, vocabulary index map).
/// Each vector is a sparse `HashMap<term_index, f32>` for cheap cosine.
pub(super) fn tfidf_vectors(sentences: &[Sentence]) -> Vec<HashMap<usize, f32>> {
    if sentences.is_empty() {
        return Vec::new();
    }

    // Step A: document frequencies + vocabulary.
    let mut df: HashMap<String, usize> = HashMap::new();
    let mut vocab: HashMap<String, usize> = HashMap::new();
    let mut vocab_terms: Vec<String> = Vec::new();
    let tokens_per_sent: Vec<Vec<String>> = sentences.iter().map(|s| tokenize(&s.text)).collect();
    for terms in &tokens_per_sent {
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for t in terms {
            if seen.insert(t.as_str()) {
                *df.entry(t.clone()).or_insert(0) += 1;
            }
            if !vocab.contains_key(t) {
                let id = vocab.len();
                vocab.insert(t.clone(), id);
                vocab_terms.push(t.clone());
            }
        }
    }
    let n = sentences.len() as f32;

    // Step B: per-sentence TF, multiplied by IDF, then L2-normalized.
    let mut vectors = Vec::with_capacity(sentences.len());
    for terms in &tokens_per_sent {
        let mut tf: HashMap<usize, f32> = HashMap::new();
        for t in terms {
            let id = vocab[t.as_str()];
            *tf.entry(id).or_insert(0.0) += 1.0;
        }
        let mut tfidf: HashMap<usize, f32> = HashMap::new();
        for (id, count) in tf {
            let term = vocab_terms[id].as_str();
            let dfv = df[term] as f32;
            let idf = (n / dfv).ln(); // 0 if term in every sentence
            if idf > 0.0 {
                tfidf.insert(id, count * idf);
            }
        }
        // L2 normalize.
        let norm: f32 = tfidf.values().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in tfidf.values_mut() {
                *v /= norm;
            }
        }
        vectors.push(tfidf);
    }
    vectors
}

/// Cosine over two sparse vectors.
pub(super) fn cosine(a: &HashMap<usize, f32>, b: &HashMap<usize, f32>) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let (small, large) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    let mut sum = 0.0;
    for (k, v) in small {
        if let Some(w) = large.get(k) {
            sum += v * w;
        }
    }
    sum
}

#[cfg(test)]
mod tfidf_tests {
    use super::*;

    fn sents(strs: &[&str]) -> Vec<Sentence> {
        strs.iter()
            .enumerate()
            .map(|(i, s)| Sentence {
                span_start: i * 100,
                text: s.to_string(),
            })
            .collect()
    }

    #[test]
    fn tokenize_lowercases_words() {
        let t = tokenize("Hello, World! Don't stop.");
        assert!(t.contains(&"hello".to_string()));
        assert!(t.contains(&"world".to_string()));
        assert!(t.contains(&"don't".to_string()));
        assert!(t.contains(&"stop".to_string()));
    }

    #[test]
    fn tfidf_zeroes_out_terms_appearing_everywhere() {
        let s = sents(&["the cat sat", "the cat slept", "the cat ran"]);
        let vecs = tfidf_vectors(&s);
        // "the" and "cat" appear in every sentence → idf = ln(1) = 0; should be absent.
        for v in &vecs {
            // The unique tokens (sat/slept/ran) must remain.
            assert_eq!(v.len(), 1, "sentence kept only the discriminating term");
        }
    }

    #[test]
    fn cosine_is_one_for_identical_sentences() {
        // Use a 3-doc corpus so IDF for the shared tokens stays non-zero
        // (without this, "hello world" appears in every doc → idf = ln(N/N) = 0
        // and the TF-IDF vectors collapse to empty).
        let s = sents(&["hello world", "hello world", "goodbye moon"]);
        let v = tfidf_vectors(&s);
        let c = cosine(&v[0], &v[1]);
        assert!(
            (c - 1.0).abs() < 1e-5,
            "cosine on identical non-degenerate sentences was {c}",
        );
    }

    #[test]
    fn cosine_is_zero_for_disjoint_sentences() {
        // Use a corpus where each sentence has a unique discriminator, so
        // IDF is non-zero and vectors actually carry weight.
        let s = sents(&["alpha beta", "gamma delta", "epsilon zeta"]);
        let v = tfidf_vectors(&s);
        // Sentences 0 and 1 share no tokens.
        assert!(cosine(&v[0], &v[1]).abs() < 1e-5);
    }

    #[test]
    fn tfidf_returns_empty_for_empty_input() {
        assert!(tfidf_vectors(&[]).is_empty());
    }

    #[test]
    fn tfidf_handles_all_universal_corpus() {
        // Every sentence shares every token → IDF for every term is 0 →
        // every vector should be empty. PageRank should still produce a
        // valid (uniform) distribution.
        let s = sents(&["the cat", "the cat", "the cat"]);
        let v = tfidf_vectors(&s);
        for vec in &v {
            assert!(vec.is_empty(), "expected empty vector, got {vec:?}");
        }
        let pr = pagerank(&v);
        assert_eq!(pr.len(), 3);
        // All rows are dangling → uniform after one iteration.
        for score in &pr {
            assert!(
                (score - 1.0 / 3.0).abs() < 0.1,
                "expected ~0.333 uniform, got {score}",
            );
        }
    }
}

/// Run PageRank on the dense similarity matrix. Edges below
/// `SIMILARITY_FLOOR` are zeroed before power iteration. Returns one
/// score per sentence, length-matched to `vectors`.
pub(super) fn pagerank(vectors: &[HashMap<usize, f32>]) -> Vec<f32> {
    let n = vectors.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![1.0];
    }

    // Build weighted similarity matrix in row-major form. Diagonal = 0.
    let mut weights = vec![0.0_f32; n * n];
    let mut row_sums = vec![0.0_f32; n];
    for i in 0..n {
        for j in (i + 1)..n {
            let s = cosine(&vectors[i], &vectors[j]);
            if s >= SIMILARITY_FLOOR {
                weights[i * n + j] = s;
                weights[j * n + i] = s;
                row_sums[i] += s;
                row_sums[j] += s;
            }
        }
    }

    let inv_n = 1.0_f32 / n as f32;
    let mut score = vec![inv_n; n];
    let teleport = (1.0 - PAGERANK_DAMPING) * inv_n;

    for _ in 0..PAGERANK_MAX_ITER {
        let mut next = vec![teleport; n];
        for j in 0..n {
            if row_sums[j] == 0.0 {
                // Dangling: distribute uniformly.
                let share = PAGERANK_DAMPING * score[j] * inv_n;
                for slot in next.iter_mut() {
                    *slot += share;
                }
            } else {
                let factor = PAGERANK_DAMPING * score[j] / row_sums[j];
                for i in 0..n {
                    let w = weights[i * n + j];
                    if w > 0.0 {
                        next[i] += factor * w;
                    }
                }
            }
        }
        let delta: f32 = score
            .iter()
            .zip(next.iter())
            .map(|(a, b)| (a - b).abs())
            .sum();
        score = next;
        if delta < PAGERANK_TOL {
            break;
        }
    }
    score
}

#[cfg(test)]
mod pagerank_tests {
    use super::*;

    fn sents(strs: &[&str]) -> Vec<Sentence> {
        strs.iter()
            .enumerate()
            .map(|(i, s)| Sentence {
                span_start: i * 100,
                text: s.to_string(),
            })
            .collect()
    }

    #[test]
    fn empty_returns_empty() {
        assert!(pagerank(&[]).is_empty());
    }

    #[test]
    fn single_sentence_gets_full_mass() {
        let v = tfidf_vectors(&sents(&["alpha beta gamma"]));
        let pr = pagerank(&v);
        assert_eq!(pr.len(), 1);
        assert!((pr[0] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn central_sentence_outscores_peripheral() {
        // Three sentences; the middle one shares tokens with each peripheral
        // and PageRank should reflect that centrality.
        let v = tfidf_vectors(&sents(&[
            "alpha unique_left",
            "alpha gamma bridge",
            "gamma unique_right",
        ]));
        let pr = pagerank(&v);
        assert!(
            pr[1] >= pr[0] && pr[1] >= pr[2],
            "middle sentence should be highest: {pr:?}",
        );
    }

    #[test]
    fn scores_sum_close_to_one() {
        let v = tfidf_vectors(&sents(&[
            "alpha beta gamma",
            "alpha gamma",
            "delta epsilon zeta",
            "alpha epsilon",
        ]));
        let pr = pagerank(&v);
        let total: f32 = pr.iter().sum();
        assert!((total - 1.0).abs() < 0.05, "sum was {total}");
    }

    #[test]
    fn fully_disconnected_graph_converges_to_uniform() {
        // Every sentence has a unique discriminator and no shared structure
        // → all pairwise cosines fall below SIMILARITY_FLOOR → every row is
        // dangling → distribution should converge to uniform 1/n.
        let v = tfidf_vectors(&sents(&[
            "alpha unique_one",
            "beta unique_two",
            "gamma unique_three",
            "delta unique_four",
        ]));
        let pr = pagerank(&v);
        for score in &pr {
            assert!(
                (score - 0.25).abs() < 0.05,
                "expected ~0.25 uniform, got {score}",
            );
        }
    }
}

/// Select sentences for `mode = Extractive`, ranked by PageRank, then
/// re-ordered by source position. Honors `target_tokens` by greedily
/// admitting top-ranked sentences while the cumulative tokenizer count
/// stays at or under the budget. If even the single highest sentence
/// already exceeds the budget, it is still emitted (a warning is logged).
fn select_extractive(
    sentences: &[Sentence],
    scores: &[f32],
    target_tokens: Option<usize>,
    family: Tokenizer,
) -> Vec<usize> {
    if sentences.is_empty() {
        return Vec::new();
    }
    // Rank index list, highest first.
    let mut order: Vec<usize> = (0..sentences.len()).collect();
    order.sort_by(|a, b| {
        scores[*b]
            .partial_cmp(&scores[*a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let chosen = match target_tokens {
        None => order,
        Some(max) => {
            let mut chosen = Vec::new();
            let mut cumulative: usize = 0;
            let mut warned_fallback = false;
            for idx in order {
                // Token-count by configured tokenizer family. If the call
                // fails (model not loaded), fall back to a char/4 heuristic
                // and warn once.
                let count = match tokenizer::count(&sentences[idx].text, family) {
                    Ok(c) => c,
                    Err(e) => {
                        if !warned_fallback {
                            tracing::warn!(
                                target: "rover::summarizer",
                                family = ?family,
                                error = %e,
                                "tokenizer unavailable; falling back to chars/4 heuristic for target_tokens accounting"
                            );
                            warned_fallback = true;
                        }
                        sentences[idx].text.chars().count().div_ceil(4)
                    }
                };
                if chosen.is_empty() {
                    chosen.push(idx);
                    cumulative = count;
                    if count > max {
                        tracing::warn!(
                            target: "rover::summarizer",
                            sentence_tokens = count,
                            target_tokens = max,
                            "top-ranked sentence exceeds target_tokens; emitting anyway",
                        );
                        break;
                    }
                } else if cumulative + count <= max {
                    chosen.push(idx);
                    cumulative += count;
                } else {
                    // Continue scanning — a shorter top-N candidate may fit later.
                }
            }
            chosen
        }
    };

    let mut by_position = chosen;
    by_position.sort_by_key(|&i| sentences[i].span_start);
    by_position
}

/// Format selected sentences into the chosen output style.
fn format_selected(sentences: &[Sentence], indices: &[usize], style: Style) -> String {
    match style {
        Style::Bullet => indices
            .iter()
            .map(|&i| format!("- {}", sentences[i].text))
            .collect::<Vec<_>>()
            .join("\n"),
        Style::Prose => indices
            .iter()
            .map(|&i| sentences[i].text.clone())
            .collect::<Vec<_>>()
            .join(" "),
        Style::Executive => {
            // Use the first selected sentence as the headline, the rest as
            // a 'Details' block.
            if indices.is_empty() {
                String::new()
            } else {
                let head = &sentences[indices[0]].text;
                if indices.len() == 1 {
                    head.clone()
                } else {
                    let rest = indices[1..]
                        .iter()
                        .map(|&i| sentences[i].text.clone())
                        .collect::<Vec<_>>()
                        .join(" ");
                    format!("{head}\n\nDetails: {rest}")
                }
            }
        }
    }
}

#[cfg(test)]
mod extractive_mode_tests {
    use super::*;

    fn sents(strs: &[&str]) -> Vec<Sentence> {
        strs.iter()
            .enumerate()
            .map(|(i, s)| Sentence {
                span_start: i * 100,
                text: s.to_string(),
            })
            .collect()
    }

    #[test]
    fn select_extractive_picks_in_source_order() {
        let s = sents(&["Low.", "High Importance Sentence Here.", "Mid."]);
        let v = tfidf_vectors(&s);
        let pr = pagerank(&v);
        // Force a known ordering: top-1 in the middle should still come out
        // sorted by source-position when selected.
        let _ = pr;
        let chosen = select_extractive(&s, &[0.1, 0.9, 0.5], None, Tokenizer::O200k);
        assert_eq!(chosen, vec![0, 1, 2]);
    }

    #[test]
    fn select_extractive_caps_to_target_tokens() {
        let s = sents(&[
            "first sentence.",  // ~4 tokens
            "second sentence.", // ~4 tokens
            "third sentence.",  // ~4 tokens
        ]);
        let chosen = select_extractive(&s, &[0.5, 0.5, 0.5], Some(5), Tokenizer::O200k);
        // Greedy admits ranked-first sentence (length ~4), then refuses the
        // next (would push cumulative >5).
        assert_eq!(chosen.len(), 1);
    }

    #[test]
    fn select_extractive_skips_oversize_and_admits_lower_ranked_that_fits() {
        // In tests the tokenizer registry is empty, so this exercises the
        // chars/4 fallback heuristic. Character lengths chosen so:
        //   s0 = 44 chars → ceil(44/4) = 11 tokens
        //   s1 =  5 chars → ceil( 5/4) =  2 tokens
        //   s2 = 44 chars → ceil(44/4) = 11 tokens
        // Rank order: [0 (0.9), 2 (0.8), 1 (0.1)]. Budget = 13.
        // Walk:
        //   idx 0 → cumulative = 11 ≤ 13, admit.
        //   idx 2 → 11 + 11 = 22 > 13, skip (continue scanning).
        //   idx 1 → 11 +  2 = 13 ≤ 13, admit (proves we continue, not break).
        // After sort by span: [0, 1].
        let s = sents(&[
            "first sentence here that is reasonably long.", // 44 chars
            "okay.",                                        //  5 chars
            "third sentence here that is reasonably long.", // 44 chars
        ]);
        let chosen = select_extractive(&s, &[0.9, 0.1, 0.8], Some(13), Tokenizer::O200k);
        assert!(chosen.contains(&0), "expected index 0 in {chosen:?}");
        assert!(chosen.contains(&1), "expected index 1 in {chosen:?}");
        assert!(
            !chosen.contains(&2),
            "expected index 2 excluded in {chosen:?}"
        );
    }

    #[test]
    fn format_bullet_prefixes_dashes() {
        let s = sents(&["a.", "b.", "c."]);
        assert_eq!(format_selected(&s, &[0, 2], Style::Bullet), "- a.\n- c.",);
    }

    #[test]
    fn format_executive_with_one_sentence_omits_details() {
        let s = sents(&["only one."]);
        assert_eq!(format_selected(&s, &[0], Style::Executive), "only one.");
    }
}

/// (depth, heading_text) parsed from an ATX heading line, or None.
fn parse_atx_heading(line: &str) -> Option<(usize, &str)> {
    let bytes = line.as_bytes();
    let mut depth = 0;
    while depth < bytes.len() && bytes[depth] == b'#' {
        depth += 1;
    }
    if depth == 0 || depth > 6 {
        return None;
    }
    if depth == bytes.len() {
        return None;
    }
    if bytes[depth] != b' ' {
        return None;
    }
    let text = line[depth + 1..].trim();
    if text.is_empty() {
        None
    } else {
        Some((depth, text))
    }
}

#[derive(Debug)]
struct HeadingSection {
    depth: usize,
    heading: String,
    /// Indices into the `sentences` array.
    sentence_indices: Vec<usize>,
}

/// Walk the source, building (heading → sentences-in-section) groups.
/// Sentences are matched into a section by their `span_start` relative
/// to the byte offsets of the heading lines.
fn group_by_headings(content: &str, sentences: &[Sentence]) -> Vec<HeadingSection> {
    let mut headings: Vec<HeadingSection> = Vec::new();
    let mut heading_offsets: Vec<usize> = Vec::new();
    let mut byte_offset = 0;
    for line in content.split_inclusive('\n') {
        let line_trimmed = line.trim_end_matches('\n');
        if let Some((depth, text)) = parse_atx_heading(line_trimmed) {
            headings.push(HeadingSection {
                depth,
                heading: text.to_string(),
                sentence_indices: Vec::new(),
            });
            heading_offsets.push(byte_offset);
        }
        byte_offset += line.len();
    }
    if headings.is_empty() {
        return Vec::new();
    }
    for (si, sent) in sentences.iter().enumerate() {
        let mut bucket: Option<usize> = None;
        for (hi, off) in heading_offsets.iter().enumerate() {
            if sent.span_start >= *off {
                bucket = Some(hi);
            } else {
                break;
            }
        }
        if let Some(b) = bucket {
            headings[b].sentence_indices.push(si);
        }
    }
    headings
}

/// Headlines mode (design §2): for each heading at the deepest covered
/// depth, emit `## heading\n\n{top-1 sentence}\n\n`. Documents without
/// any headings fall back to the top-3 highest-scoring sentences as a
/// bullet list; `target_tokens`, if set, additionally caps the per-sentence
/// token budget within `select_extractive` before the top-3 truncation.
fn select_headlines(
    content: &str,
    sentences: &[Sentence],
    scores: &[f32],
    target_tokens: Option<usize>,
    family: Tokenizer,
) -> String {
    let mut groups = group_by_headings(content, sentences);
    // Drop empty sections (heading with no sentence beneath).
    groups.retain(|g| !g.sentence_indices.is_empty());

    if groups.is_empty() {
        // No headings → fall back to flat extractive top-3 (capped by tokens
        // if target_tokens supplied).
        let chosen = select_extractive(
            sentences,
            scores,
            target_tokens.or(Some(usize::MAX)),
            family,
        );
        let trimmed: Vec<usize> = chosen.into_iter().take(3).collect();
        return format_selected(sentences, &trimmed, Style::Bullet);
    }

    // Pick deepest covered depth.
    let deepest = groups.iter().map(|g| g.depth).max().unwrap();

    // For each group at that depth, pick its highest-scoring sentence.
    let mut emitted = Vec::new();
    let mut cumulative_tokens: usize = 0;
    let token_budget = target_tokens.unwrap_or(usize::MAX);
    let mut warned_fallback = false;
    for g in groups.iter().filter(|g| g.depth == deepest) {
        let best = g.sentence_indices.iter().copied().max_by(|a, b| {
            scores[*a]
                .partial_cmp(&scores[*b])
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        if let Some(idx) = best {
            let count = match tokenizer::count(&sentences[idx].text, family) {
                Ok(c) => c,
                Err(e) => {
                    if !warned_fallback {
                        tracing::warn!(
                            target: "rover::summarizer",
                            family = ?family,
                            error = %e,
                            "tokenizer unavailable; falling back to chars/4 heuristic for headlines budget"
                        );
                        warned_fallback = true;
                    }
                    sentences[idx].text.chars().count().div_ceil(4)
                }
            };
            // Always include the first heading even if oversize.
            if !emitted.is_empty() && cumulative_tokens + count > token_budget {
                break;
            }
            let prefix = "#".repeat(g.depth);
            emitted.push(format!("{prefix} {}\n\n{}", g.heading, sentences[idx].text));
            cumulative_tokens += count;
        }
    }
    emitted.join("\n\n")
}

#[cfg(test)]
mod headlines_tests {
    use super::*;

    #[test]
    fn parse_atx_extracts_depth_and_text() {
        assert_eq!(parse_atx_heading("# Hello"), Some((1, "Hello")));
        assert_eq!(parse_atx_heading("### Three"), Some((3, "Three")));
        assert_eq!(parse_atx_heading("####### Too Deep"), None);
        assert_eq!(parse_atx_heading("#NoSpace"), None);
        assert_eq!(parse_atx_heading("Not a heading"), None);
    }

    #[test]
    fn group_by_headings_buckets_sentences_correctly() {
        let content =
            "# A\nfirst sentence here.\nsecond sentence here.\n# B\nthird sentence here.\n";
        let s = split_sentences(content);
        let groups = group_by_headings(content, &s);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].heading, "A");
        assert_eq!(groups[1].heading, "B");
        // Section A should have 2 sentences (first + second).
        assert_eq!(groups[0].sentence_indices.len(), 2);
        // Section B should have 1 sentence (third).
        assert_eq!(groups[1].sentence_indices.len(), 1);
    }

    #[test]
    fn select_headlines_emits_one_per_section() {
        let content = "# Intro\nThe quick brown fox.\nThe lazy dog.\n## Body\nDetails matter here.\nMore words follow.\n";
        let s = split_sentences(content);
        let v = tfidf_vectors(&s);
        let pr = pagerank(&v);
        let out = select_headlines(content, &s, &pr, None, Tokenizer::O200k);
        // Two headings at top level should produce two sections — but the
        // deepest depth is 2 ("## Body"), so only that section is included.
        assert!(out.contains("## Body"));
        assert!(out.contains("Details") || out.contains("More words"));
    }

    #[test]
    fn select_headlines_falls_back_when_no_headings() {
        let content = "First sentence here. Second sentence here. Third one.";
        let s = split_sentences(content);
        let v = tfidf_vectors(&s);
        let pr = pagerank(&v);
        let out = select_headlines(content, &s, &pr, None, Tokenizer::O200k);
        // No headings → bullet fallback.
        assert!(out.contains("- "));
    }

    #[test]
    fn select_headlines_picks_deepest_depth_under_mixed_nesting() {
        // # A
        //   ## A1 ...
        //   ## A2 ...
        // # B
        //   ## B1 ...
        //
        // Deepest covered depth is 2; output should contain A1/A2/B1
        // headings and not A or B (which are depth 1).
        let content = "\
# A\n\
## A1\n\
Sentence under A1 here describing the thing.\n\
## A2\n\
Sentence under A2 here describing the other thing.\n\
# B\n\
## B1\n\
Sentence under B1 here describing a third thing.\n";
        let s = split_sentences(content);
        let v = tfidf_vectors(&s);
        let pr = pagerank(&v);
        let out = select_headlines(content, &s, &pr, None, Tokenizer::O200k);
        assert!(out.contains("## A1"), "expected ## A1 in {out}");
        assert!(out.contains("## A2"), "expected ## A2 in {out}");
        assert!(out.contains("## B1"), "expected ## B1 in {out}");
        // Top-level "# A" and "# B" should NOT appear as section headings
        // in their own right.
        for line in out.lines() {
            if line.starts_with("# ") && !line.starts_with("## ") {
                panic!("unexpected H1 in headlines output: {line}");
            }
        }
    }
}

/// The public extractive backend. Stateless aside from the `name` field
/// (configurable via the registry so a project can have multiple
/// extractive entries — e.g. one named "default" and one named "fast").
#[derive(Debug, Clone)]
pub struct ExtractiveBackend {
    name: String,
    /// Tokenizer family for `target_tokens` accounting. Defaults to the
    /// configured tokenizer at service construction time.
    tokenizer: Tokenizer,
}

impl ExtractiveBackend {
    pub fn new(name: impl Into<String>, tokenizer: Tokenizer) -> Self {
        Self {
            name: name.into(),
            tokenizer,
        }
    }

    /// Run the full pipeline. Public for direct testing without the trait.
    pub fn run(&self, content: &str, opts: &CompactOpts) -> String {
        let sentences = split_sentences(content);
        if sentences.is_empty() {
            return String::new();
        }
        let vectors = tfidf_vectors(&sentences);
        let scores = pagerank(&vectors);

        match opts.mode {
            CompactMode::Headlines => select_headlines(
                content,
                &sentences,
                &scores,
                opts.target_tokens,
                self.tokenizer,
            ),
            CompactMode::Extractive => {
                let indices =
                    select_extractive(&sentences, &scores, opts.target_tokens, self.tokenizer);
                format_selected(&sentences, &indices, opts.style)
            }
            CompactMode::Abstractive => {
                // Abstractive falls through to extractive when this backend
                // is invoked directly — the service-level fallback path uses
                // exactly this code path.
                let indices =
                    select_extractive(&sentences, &scores, opts.target_tokens, self.tokenizer);
                format_selected(&sentences, &indices, opts.style)
            }
        }
    }
}

#[async_trait]
impl SummarizerBackend for ExtractiveBackend {
    async fn compact(&self, content: &str, opts: &CompactOpts) -> Result<String, BackendError> {
        if content.trim().is_empty() {
            return Err(BackendError::Invalid("empty content".to_string()));
        }
        Ok(self.run(content, opts))
    }

    fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
mod trait_tests {
    use super::*;
    use crate::summarizer::backend::{CompactMode, PreserveSection, Style};

    fn opts(mode: CompactMode, style: Style) -> CompactOpts {
        CompactOpts {
            mode,
            style,
            target_tokens: None,
            focus: None,
            preserve: vec![],
            backend_name: "default".to_string(),
        }
    }

    #[tokio::test]
    async fn empty_content_returns_invalid_error() {
        let be = ExtractiveBackend::new("default", Tokenizer::O200k);
        let r = be
            .compact("   ", &opts(CompactMode::Extractive, Style::Prose))
            .await;
        assert!(matches!(r, Err(BackendError::Invalid(_))));
    }

    #[tokio::test]
    async fn extractive_returns_non_empty_for_real_content() {
        let be = ExtractiveBackend::new("default", Tokenizer::O200k);
        let content = "The cat sat on the mat. The dog ran away quickly. The bird flew south.";
        let out = be
            .compact(content, &opts(CompactMode::Extractive, Style::Prose))
            .await
            .unwrap();
        assert!(!out.is_empty());
    }

    #[tokio::test]
    async fn name_round_trips() {
        let be = ExtractiveBackend::new("alt-name", Tokenizer::O200k);
        assert_eq!(be.name(), "alt-name");
    }

    #[test]
    fn preserve_unused_for_extractive_compiles() {
        // Sanity check: PreserveSection is part of the trait surface even
        // though extractive ignores it.
        let _ = PreserveSection::Code;
    }
}
