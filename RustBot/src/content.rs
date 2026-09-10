use std::{
    collections::{HashMap, HashSet},
    sync::LazyLock,
};

use ammonia::{Builder, UrlRelative};
use regex::Regex;

static IMG_SRC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)<img[^>]+src=["']([^"']+)["']"#).expect("valid image regex")
});
static IMAGE_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)https?://[^\s"']+\.(?:jpg|jpeg|png|gif|webp)(?:\?[^\s"']*)?"#)
        .expect("valid image URL regex")
});
static TELEGRAM_CDN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)https?://cdn[0-9]*\.cdn-telegram\.org/[^\s"']+"#)
        .expect("valid Telegram CDN regex")
});
static EMPTY_ANCHOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<a>(.*?)</a>").expect("valid empty anchor regex"));

pub fn extract_image_url(input: &str) -> Option<String> {
    IMG_SRC
        .captures(input)
        .and_then(|captures| captures.get(1))
        .or_else(|| IMAGE_URL.find(input))
        .or_else(|| TELEGRAM_CDN.find(input))
        .map(|value| value.as_str().to_owned())
}

pub fn clean_html(input: &str) -> String {
    let tags: HashSet<&str> = ["a", "b", "i", "u", "s", "code", "pre", "br"]
        .into_iter()
        .collect();
    let clean_content: HashSet<&str> = ["script", "style", "img"].into_iter().collect();
    let schemes: HashSet<&str> = ["http", "https", "tg"].into_iter().collect();
    let mut builder = Builder::default();
    builder
        .tags(tags)
        .tag_attributes(HashMap::new())
        .generic_attributes(HashSet::new())
        .add_tag_attributes("a", ["href"])
        .url_schemes(schemes)
        .url_relative(UrlRelative::Deny)
        .clean_content_tags(clean_content)
        .link_rel(None);
    let cleaned = builder.clean(input).to_string().replace("<br>", "\n");
    let cleaned = EMPTY_ANCHOR.replace_all(&cleaned, "$1");
    collapse_newlines(&cleaned)
}

pub fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn split_utf8_bytes(input: &str, max_bytes: usize) -> Vec<String> {
    if max_bytes == 0 || input.len() <= max_bytes {
        return vec![input.to_owned()];
    }
    let mut rest = input;
    let mut chunks = Vec::new();
    while rest.len() > max_bytes {
        let mut cut = max_bytes;
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        let candidate = &rest[..cut];
        if let Some(newline) = candidate.rfind('\n').filter(|index| *index > cut / 2) {
            chunks.push(rest[..newline].to_owned());
            rest = &rest[newline + 1..];
        } else {
            chunks.push(candidate.to_owned());
            rest = &rest[cut..];
        }
    }
    if !rest.is_empty() {
        chunks.push(rest.to_owned());
    }
    chunks
}

fn collapse_newlines(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut newlines = 0;
    for character in input.chars() {
        if character == '\n' {
            newlines += 1;
            if newlines <= 2 {
                result.push(character);
            }
        } else {
            newlines = 0;
            result.push(character);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_active_content_and_links() {
        let result = clean_html(
            r#"<b>2 &lt; 3</b><script>alert(1)</script><a href="javascript:x">bad</a><a href="https://example.com">good</a>"#,
        );
        assert!(result.contains("<b>2 &lt; 3</b>"));
        assert!(!result.contains("alert(1)"));
        assert!(!result.contains("javascript:"));
        assert!(!result.contains("bad</a>"));
        assert!(result.contains("https://example.com"));
    }

    #[test]
    fn splits_without_breaking_utf8() {
        let input = "你好".repeat(20);
        let chunks = split_utf8_bytes(&input, 17);
        assert_eq!(chunks.concat(), input);
        assert!(chunks.iter().all(|chunk| chunk.len() <= 17));
    }

    #[test]
    fn extracts_first_image() {
        assert_eq!(
            extract_image_url(r#"<img src="https://example.com/a.jpg">"#).as_deref(),
            Some("https://example.com/a.jpg")
        );
    }
}
