//! Abstractive prompt template.
//!
//! Builds a single system prompt that's stable across providers. Per-
//! provider tweaks (e.g. omitting the "Reply with only the summary"
//! instruction for models that obey it by default) would live here if
//! ever needed; M7 ships one template.

use std::fmt::Write as _;

use crate::summarizer::backend::{CompactOpts, PreserveSection, Style};

/// Pair returned to the cloud backend.
#[derive(Debug, Clone)]
pub struct PromptParts {
    pub system: String,
    pub user: String,
}

fn style_description(style: Style) -> &'static str {
    match style {
        Style::Bullet => "Markdown bullet list, one fact per bullet, no nested bullets.",
        Style::Prose => "One or more short paragraphs.",
        Style::Executive => {
            "Two-section format: a one-sentence headline, then a 'Details' paragraph."
        }
    }
}

fn preserve_description(p: &[PreserveSection]) -> String {
    let mut names: Vec<&'static str> = p
        .iter()
        .map(|pp| match pp {
            PreserveSection::Code => "code blocks",
            PreserveSection::Tables => "tables",
            PreserveSection::Quotes => "blockquotes",
            PreserveSection::Lists => "ordered/unordered lists",
        })
        .collect();
    names.sort();
    names.dedup();
    names.join(", ")
}

/// Render the system + user prompt for an abstractive summarization call.
///
/// Sections (`focus`, `preserve`, `target_tokens`) are conditionally
/// included so absent fields don't emit empty lines. Empty `content` is
/// guarded by `debug_assert!` — debug builds panic to catch caller bugs;
/// release builds still render a sensible prompt with an empty user
/// message.
pub fn render_abstractive(opts: &CompactOpts, content: &str) -> PromptParts {
    debug_assert!(
        !content.is_empty(),
        "render_abstractive called with empty content; caller must validate",
    );
    let mut sys = String::with_capacity(512);
    sys.push_str(
        "You are a precise summarizer. Reply with only the summary — no preamble, no postamble, no meta-commentary. Output valid Markdown.\n\n",
    );
    sys.push_str("Summarize the content provided in the user message.\n\n");

    if let Some(n) = opts.target_tokens {
        writeln!(sys, "Target length: ~{n} tokens.").expect("write to String never fails");
    }
    writeln!(sys, "Output style: {}", style_description(opts.style))
        .expect("write to String never fails");
    if let Some(focus) = opts
        .focus
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        writeln!(sys, "Focus on: {focus}").expect("write to String never fails");
    }
    if !opts.preserve.is_empty() {
        writeln!(
            sys,
            "Preserve the following elements verbatim wherever they appear: {}.",
            preserve_description(&opts.preserve),
        )
        .expect("write to String never fails");
    }
    sys.push_str("\nRules:\n");
    sys.push_str("- Do not add information not present in the source.\n");
    sys.push_str(
        "- Do not include section titles or headers that the source does not have, unless the chosen style explicitly produces them.\n",
    );
    if opts.target_tokens.is_some() {
        sys.push_str("- If the source is already shorter than the target, return it unchanged.\n");
    }

    PromptParts {
        system: sys,
        user: content.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summarizer::backend::CompactMode;

    fn opts(
        style: Style,
        preserve: Vec<PreserveSection>,
        target: Option<usize>,
        focus: Option<&str>,
    ) -> CompactOpts {
        CompactOpts {
            mode: CompactMode::Abstractive,
            style,
            target_tokens: target,
            focus: focus.map(str::to_string),
            preserve,
            backend_name: "fast".to_string(),
        }
    }

    #[test]
    fn minimal_prompt_contains_required_directives() {
        let p = render_abstractive(&opts(Style::Prose, vec![], None, None), "hello world");
        assert!(p.system.contains("Reply with only the summary"));
        assert!(p.system.contains("Output valid Markdown"));
        assert!(p.system.contains("Do not add information not present"));
        assert!(!p.system.contains("Target length"));
        assert!(!p.system.contains("Focus on"));
        assert!(!p.system.contains("Preserve"));
        assert_eq!(p.user, "hello world");
    }

    #[test]
    fn target_tokens_included_when_set() {
        let p = render_abstractive(&opts(Style::Prose, vec![], Some(500), None), "x");
        assert!(p.system.contains("~500 tokens"));
    }

    #[test]
    fn return_unchanged_rule_omitted_when_no_target() {
        // Without a target, the "return unchanged if shorter than the target"
        // rule has no well-defined meaning and should be omitted.
        let no_target = render_abstractive(&opts(Style::Prose, vec![], None, None), "x");
        let with_target = render_abstractive(&opts(Style::Prose, vec![], Some(500), None), "x");
        assert!(!no_target.system.contains("return it unchanged"));
        assert!(with_target.system.contains("return it unchanged"));
    }

    #[test]
    fn focus_skipped_when_empty_or_whitespace() {
        let p = render_abstractive(&opts(Style::Prose, vec![], None, Some("   ")), "x");
        assert!(!p.system.contains("Focus on"));
    }

    #[test]
    fn preserve_section_lists_human_names_sorted() {
        let p = render_abstractive(
            &opts(
                Style::Prose,
                vec![
                    PreserveSection::Code,
                    PreserveSection::Tables,
                    PreserveSection::Code,
                ],
                None,
                None,
            ),
            "x",
        );
        // sorted+deduped → "code blocks, tables"
        assert!(
            p.system.contains("code blocks, tables"),
            "system was {}",
            p.system,
        );
    }

    #[test]
    fn system_sections_appear_in_expected_order() {
        // Render with everything set so all sections are present.
        let p = render_abstractive(
            &opts(
                Style::Prose,
                vec![PreserveSection::Code],
                Some(500),
                Some("safety"),
            ),
            "x",
        );
        let target = p
            .system
            .find("Target length")
            .expect("Target length section present");
        let style = p
            .system
            .find("Output style")
            .expect("Output style section present");
        let focus = p.system.find("Focus on").expect("Focus on section present");
        let preserve = p
            .system
            .find("Preserve the following")
            .expect("Preserve section present");
        let rules = p.system.find("Rules:").expect("Rules section present");

        assert!(target < style, "Target length must precede Output style");
        assert!(style < focus, "Output style must precede Focus on");
        assert!(focus < preserve, "Focus on must precede Preserve");
        assert!(preserve < rules, "Preserve must precede Rules");
    }

    #[test]
    fn each_style_has_a_distinct_description() {
        let a = render_abstractive(&opts(Style::Bullet, vec![], None, None), "x");
        let b = render_abstractive(&opts(Style::Prose, vec![], None, None), "x");
        let c = render_abstractive(&opts(Style::Executive, vec![], None, None), "x");
        assert_ne!(a.system, b.system);
        assert_ne!(b.system, c.system);
        // Substrings unique to each style description.
        assert!(
            a.system.contains("bullet list"),
            "bullet system: {}",
            a.system
        );
        assert!(
            b.system.contains("short paragraphs"),
            "prose system: {}",
            b.system
        );
        assert!(
            c.system.contains("headline"),
            "executive system: {}",
            c.system
        );
    }
}
