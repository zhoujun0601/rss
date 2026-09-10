use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use feed_rs::model::Entry;
use sha2::{Digest, Sha256};
use url::Url;

use crate::{models::FeedEntry, security::SecureHttpClient};

const MAX_FEED_ENTRIES: usize = 1000;
const MAX_TITLE_BYTES: usize = 1024;
const MAX_DESCRIPTION_BYTES: usize = 32 * 1024;
const MAX_LINK_BYTES: usize = 4096;

#[derive(Clone)]
pub struct FeedClient {
    http: SecureHttpClient,
}

impl FeedClient {
    pub fn new(proxy: Option<Url>) -> Self {
        Self {
            http: SecureHttpClient::new(proxy, Duration::from_secs(60)),
        }
    }

    pub async fn fetch(&self, url: &str) -> Result<Vec<FeedEntry>> {
        let body = self.http.get(url).await?;
        parse(&body)
    }
}

pub fn parse(body: &[u8]) -> Result<Vec<FeedEntry>> {
    let parser = feed_rs::parser::Builder::new()
        .id_generator(|links, title, uri| {
            let mut hasher = Sha256::new();
            for link in links {
                hasher.update(link.href.as_bytes());
            }
            if let Some(title) = title {
                hasher.update(title.content.as_bytes());
            }
            if let Some(uri) = uri {
                hasher.update(uri.as_bytes());
            }
            format!("tgbot-generated:{}", hex::encode(hasher.finalize()))
        })
        .build();
    let feed = parser.parse(body).context("无法解析 RSS/Atom/JSON Feed")?;
    if feed.entries.len() > MAX_FEED_ENTRIES {
        bail!("Feed 条目数超过上限 {MAX_FEED_ENTRIES}");
    }
    let fetched_at = Utc::now();
    Ok(feed
        .entries
        .iter()
        .map(|entry| convert_entry(entry, fetched_at))
        .collect())
}

fn convert_entry(entry: &Entry, fallback_time: DateTime<Utc>) -> FeedEntry {
    let title = entry
        .title
        .as_ref()
        .map(|text| text.content.clone())
        .unwrap_or_default();
    let description = entry
        .summary
        .as_ref()
        .map(|text| text.content.clone())
        .or_else(|| {
            entry
                .content
                .as_ref()
                .and_then(|content| content.body.clone())
        })
        .unwrap_or_default();
    let link = entry
        .links
        .iter()
        .find(|link| {
            link.rel
                .as_deref()
                .is_none_or(|relation| relation == "alternate")
        })
        .or_else(|| entry.links.first())
        .map(|link| link.href.clone())
        .unwrap_or_default();
    let source_time = entry.published.or(entry.updated);
    let published_at = source_time.unwrap_or(fallback_time);
    let identity = if !entry.id.starts_with("tgbot-generated:") && !entry.id.trim().is_empty() {
        format!("id:{}", entry.id.trim())
    } else if !link.is_empty() {
        format!("link:{link}")
    } else {
        let source_timestamp = source_time
            .map(|time| time.to_rfc3339())
            .unwrap_or_default();
        format!("content:{title}\0{description}\0{source_timestamp}")
    };
    let key = hex::encode(Sha256::digest(identity.as_bytes()));
    FeedEntry {
        key,
        title: truncate_utf8_bytes(&title, MAX_TITLE_BYTES),
        description: truncate_utf8_bytes(&description, MAX_DESCRIPTION_BYTES),
        link: truncate_utf8_bytes(&link, MAX_LINK_BYTES),
        published_at,
    }
}

fn truncate_utf8_bytes(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rss_and_produces_stable_keys() {
        let xml = br#"<?xml version="1.0"?><rss version="2.0"><channel><title>x</title><link>https://example.com</link><description>x</description><item><guid>a</guid><title>Hello</title><link>https://example.com/a</link><pubDate>Wed, 01 Jan 2025 00:00:00 GMT</pubDate></item></channel></rss>"#;
        let first = parse(xml).unwrap();
        let second = parse(xml).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].key, second[0].key);
        assert_eq!(first[0].title, "Hello");
    }

    #[test]
    fn parses_atom() {
        let xml = br#"<feed xmlns="http://www.w3.org/2005/Atom"><title>x</title><id>x</id><updated>2025-01-01T00:00:00Z</updated><entry><title>A</title><id>entry-a</id><updated>2025-01-02T00:00:00Z</updated></entry></feed>"#;
        assert_eq!(parse(xml).unwrap().len(), 1);
    }

    #[test]
    fn undated_unidentified_entries_keep_stable_keys() {
        let xml = br#"<?xml version="1.0"?><rss version="2.0"><channel><title>x</title><link>https://example.com</link><description>x</description><item><title>No date</title><description>body</description></item></channel></rss>"#;
        let first = parse(xml).unwrap();
        let second = parse(xml).unwrap();
        assert_eq!(first[0].key, second[0].key);
    }

    #[test]
    fn truncates_oversized_entry_fields_on_utf8_boundaries() {
        let description = "界".repeat(MAX_DESCRIPTION_BYTES);
        let xml = format!(
            r#"<?xml version="1.0"?><rss version="2.0"><channel><title>x</title><link>https://example.com</link><description>x</description><item><guid>a</guid><title>Hello</title><description>{description}</description></item></channel></rss>"#
        );
        let entries = parse(xml.as_bytes()).unwrap();
        assert!(entries[0].description.len() <= MAX_DESCRIPTION_BYTES);
        assert!(
            entries[0]
                .description
                .is_char_boundary(entries[0].description.len())
        );
    }

    #[test]
    fn rejects_feeds_with_too_many_entries() {
        let items = (0..=MAX_FEED_ENTRIES)
            .map(|index| format!("<item><guid>{index}</guid><title>x</title></item>"))
            .collect::<String>();
        let xml = format!(
            r#"<?xml version="1.0"?><rss version="2.0"><channel><title>x</title><link>https://example.com</link><description>x</description>{items}</channel></rss>"#
        );
        assert!(parse(xml.as_bytes()).is_err());
    }
}
