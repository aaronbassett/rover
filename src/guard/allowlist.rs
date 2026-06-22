//! URL-glob matching for the per-method `[prompt_injection.allowlist]` lists.

use regex::Regex;

/// Returns `true` if `url` matches any glob in `globs`. `*` matches any run of
/// characters; all other characters match literally. An empty list never
/// matches; a bare `"*"` matches everything.
pub fn matches(globs: &[String], url: &str) -> bool {
    globs.iter().any(|g| glob_to_regex(g).is_match(url))
}

/// Translate a `*`-glob into an anchored regex. `*` → `.*`; every other
/// character is regex-escaped so it matches literally.
fn glob_to_regex(glob: &str) -> Regex {
    let mut pat = String::with_capacity(glob.len() + 4);
    pat.push('^');
    for ch in glob.chars() {
        if ch == '*' {
            pat.push_str(".*");
        } else {
            // regex::escape on a single-char string is simplest and correct.
            pat.push_str(&regex::escape(&ch.to_string()));
        }
    }
    pat.push('$');
    // The pattern only ever contains `^`, `$`, `.*`, and escaped literals, so
    // compilation cannot fail.
    Regex::new(&pat).expect("glob_to_regex produces a valid regex")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_list_never_matches() {
        assert!(!matches(&[], "https://example.com/"));
    }

    #[test]
    fn bare_star_matches_everything() {
        let globs = vec!["*".to_string()];
        assert!(matches(&globs, "https://example.com/a/b"));
        assert!(matches(&globs, "file:///etc/hosts"));
    }

    #[test]
    fn subdomain_and_path_glob() {
        let globs = vec!["https://*.example.com/*".to_string()];
        assert!(matches(&globs, "https://docs.example.com/page"));
        assert!(matches(&globs, "https://a.b.example.com/x/y"));
        assert!(!matches(&globs, "https://example.com/page")); // no subdomain
        assert!(!matches(&globs, "https://evil.com/example.com")); // host mismatch
    }

    #[test]
    fn literal_dot_is_not_a_wildcard() {
        let globs = vec!["https://example.com/*".to_string()];
        assert!(matches(&globs, "https://example.com/x"));
        assert!(!matches(&globs, "https://exampleXcom/x"));
    }

    #[test]
    fn any_glob_in_list_matches() {
        let globs = vec!["https://a.com/*".to_string(), "https://b.com/*".to_string()];
        assert!(matches(&globs, "https://b.com/page"));
    }
}
