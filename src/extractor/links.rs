//! Markdown link/image post-pass: rewrite relative URLs to absolute.

use std::sync::LazyLock;

use regex::Regex;
use url::Url;

// `[text](href)` and `![alt](src)`. The `!` prefix marks inline images.
// `text` may contain anything except `]`. `href` stops at the first
// whitespace or `)`.
static INLINE_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?P<bang>!?)\[(?P<text>[^\]]*)\]\((?P<href>[^)\s]+)(?P<rest>[^)]*)\)").unwrap()
});

// `[id]: href "optional title"` at the start of a line.
static REF_DEF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)^\[(?P<id>[^\]]+)\]:\s*(?P<href>\S+)(?P<rest>.*)$"#).unwrap()
});

pub fn absolutize(markdown: &str, base: &Url) -> String {
    let pass1 = INLINE_LINK.replace_all(markdown, |caps: &regex::Captures| {
        let bang = &caps["bang"];
        let text = &caps["text"];
        let href = &caps["href"];
        let rest = &caps["rest"];
        let abs = resolve(base, href);
        format!("{bang}[{text}]({abs}{rest})")
    });
    REF_DEF
        .replace_all(&pass1, |caps: &regex::Captures| {
            let id = &caps["id"];
            let href = &caps["href"];
            let rest = &caps["rest"];
            let abs = resolve(base, href);
            format!("[{id}]: {abs}{rest}")
        })
        .into_owned()
}

fn resolve(base: &Url, href: &str) -> String {
    // Already absolute (has scheme) or special: mailto / data / javascript? Leave alone.
    if href.contains("://")
        || href.starts_with("mailto:")
        || href.starts_with("data:")
        || href.starts_with("javascript:")
    {
        return href.to_string();
    }
    match base.join(href) {
        Ok(u) => u.to_string(),
        Err(e) => {
            tracing::debug!(target: "rover::extractor", href, err = %e, "could not join link href");
            href.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b() -> Url {
        Url::parse("https://example.com/articles/m4").unwrap()
    }

    #[test]
    fn inline_relative_link_absolutized() {
        let md = "See [docs](/docs/intro).";
        let out = absolutize(md, &b());
        assert_eq!(out, "See [docs](https://example.com/docs/intro).");
    }

    #[test]
    fn absolute_link_unchanged() {
        let md = "Visit [site](https://www.example.org/).";
        let out = absolutize(md, &b());
        assert_eq!(out, md);
    }

    #[test]
    fn inline_image_src_absolutized() {
        let md = "![alt](/img/x.png)";
        let out = absolutize(md, &b());
        assert_eq!(out, "![alt](https://example.com/img/x.png)");
    }

    #[test]
    fn reference_definition_absolutized() {
        let md = "[ref]: /docs/ref \"title\"\nSome [ref] usage.";
        let out = absolutize(md, &b());
        assert!(
            out.contains("[ref]: https://example.com/docs/ref \"title\""),
            "got: {out}"
        );
    }

    #[test]
    fn anchor_hash_absolutized() {
        let md = "[next](#section)";
        let out = absolutize(md, &b());
        assert!(
            out.contains("https://example.com/articles/m4#section"),
            "got: {out}"
        );
    }

    #[test]
    fn mailto_and_data_preserved() {
        let md = "Email [me](mailto:x@y.z) and ![pixel](data:image/png;base64,iVBORw0KGgo).";
        let out = absolutize(md, &b());
        assert_eq!(out, md);
    }
}
