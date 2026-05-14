//! extraction_quality scorer.
//!
//! score = density + 0.05 (if title) + 0.10 (if metadata), clamped to [0, 1].
//! density = extracted_text_len / raw_html_text_len, clamped to [0, 1].

pub fn score(
    extracted_md: &str,
    raw_html_text_len: usize,
    has_metadata: bool,
    has_title: bool,
) -> f32 {
    let extracted_len = visible_text_len(extracted_md);
    let density = (extracted_len as f32 / raw_html_text_len.max(1) as f32).min(1.0);
    let mut bonus = 0.0;
    if has_title {
        bonus += 0.05;
    }
    if has_metadata {
        bonus += 0.10;
    }
    (density + bonus).clamp(0.0, 1.0)
}

/// Strip whitespace-only lines and a leading YAML frontmatter block, then return char count.
fn visible_text_len(md: &str) -> usize {
    let after_fm = strip_frontmatter(md);
    after_fm
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|l| l.chars().count())
        .sum()
}

fn strip_frontmatter(s: &str) -> &str {
    if !s.starts_with("---\n") {
        return s;
    }
    let rest = &s[4..];
    if let Some(end) = rest.find("\n---\n") {
        return &rest[end + 5..];
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_scores_zero() {
        assert_eq!(score("", 100, false, false), 0.0);
    }

    #[test]
    fn perfect_density_with_bonuses_clamped_to_one() {
        // 100-char extracted text against 50-char raw -> density 2.0 clamped to 1.0
        let md = "a".repeat(100);
        let s = score(&md, 50, true, true);
        assert!((s - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn density_only_path_produces_expected_ratio() {
        let md = "a".repeat(40);
        let s = score(&md, 100, false, false);
        assert!((s - 0.40).abs() < 0.01);
    }

    #[test]
    fn bonuses_alone_are_capped_below_one() {
        let s = score("", 100, true, true);
        // density 0; bonuses 0.15
        assert!((s - 0.15).abs() < 0.01);
    }

    #[test]
    fn score_always_in_unit_interval() {
        for raw in [1usize, 10, 100, 1000, 1_000_000] {
            for md_len in [0usize, 1, 10, 100, 10_000] {
                let md = "a".repeat(md_len);
                let s = score(&md, raw, true, true);
                assert!((0.0..=1.0).contains(&s), "raw={raw} md_len={md_len} s={s}");
            }
        }
    }

    #[test]
    fn frontmatter_excluded_from_density() {
        let md = "---\nurl: \"x\"\n---\n\nhello world\n";
        let s = score(md, 100, false, false);
        // "hello world" is 11 chars -> 0.11
        assert!((s - 0.11).abs() < 0.02);
    }
}
