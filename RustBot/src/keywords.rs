use regex::RegexBuilder;

use crate::models::FeedEntry;

pub fn normalize_input(input: &str) -> Vec<String> {
    let normalized = input.replace('，', ",");
    let mut values: Vec<String> = normalized
        .split_whitespace()
        .flat_map(|part| part.split(','))
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    values.sort();
    values.dedup();
    values
}

pub fn matches(entry: &FeedEntry, keywords: &[String], rss_name: &str) -> Vec<String> {
    let title = entry.title.to_lowercase();
    let description = entry.description.to_lowercase();
    let all = format!("{title} {description}");
    let mut matched = Vec::new();

    for raw in keywords {
        let mut keyword = raw.trim();
        if keyword.is_empty() {
            continue;
        }
        let blocked = keyword.starts_with('-');
        if blocked {
            keyword = keyword.trim_start_matches('-');
        }

        let (scope, processed) = if let Some(value) = keyword.strip_prefix("#t") {
            (Scope::Title, value)
        } else if let Some(value) = keyword.strip_prefix("#c") {
            (Scope::Description, value)
        } else if let Some(value) = keyword.strip_prefix("#a") {
            (Scope::All, value)
        } else {
            (Scope::Default, keyword)
        };
        let processed = processed.trim();
        let mut plus = processed.split('+');
        let actual = plus.next().unwrap_or_default().trim();
        let rss_filter = plus.next().map(str::trim);
        if plus.next().is_some() {
            // Preserve the original behavior for malformed multi-plus expressions.
            if is_match(
                processed,
                target(scope, blocked, &title, &description, &all),
            ) {
                if blocked {
                    return Vec::new();
                }
                matched.push(processed.to_owned());
            }
            continue;
        }
        if rss_filter.is_some_and(|filter| !filter.eq_ignore_ascii_case(rss_name)) {
            continue;
        }
        if actual.is_empty() {
            continue;
        }
        if is_match(actual, target(scope, blocked, &title, &description, &all)) {
            if blocked {
                return Vec::new();
            }
            matched.push(actual.to_owned());
        }
    }
    matched
}

#[derive(Clone, Copy)]
enum Scope {
    Default,
    Title,
    Description,
    All,
}

fn target<'a>(
    scope: Scope,
    blocked: bool,
    title: &'a str,
    description: &'a str,
    all: &'a str,
) -> &'a str {
    match scope {
        Scope::Title => title,
        Scope::Description => description,
        Scope::All => all,
        Scope::Default if blocked => all,
        Scope::Default => title,
    }
}

fn is_match(keyword: &str, content: &str) -> bool {
    let lower = keyword.to_lowercase();
    if lower.contains('*') {
        let pattern = lower
            .split('*')
            .map(regex::escape)
            .collect::<Vec<_>>()
            .join(".*");
        if let Ok(regex) = RegexBuilder::new(&pattern).case_insensitive(true).build() {
            return regex.is_match(content);
        }
    }
    content.contains(&lower)
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;

    fn entry() -> FeedEntry {
        FeedEntry {
            key: "1".into(),
            title: "Go release".into(),
            description: "New security fixes".into(),
            link: String::new(),
            published_at: Utc::now(),
        }
    }

    fn values(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn normalizes_commas_whitespace_and_duplicates() {
        assert_eq!(
            normalize_input("beta,alpha，beta gamma"),
            values(&["alpha", "beta", "gamma"])
        );
    }

    #[test]
    fn supports_scopes_filters_wildcards_and_blocks() {
        let item = entry();
        assert_eq!(matches(&item, &values(&["#tgo"]), "news"), values(&["go"]));
        assert_eq!(
            matches(&item, &values(&["#csecurity"]), "news"),
            values(&["security"])
        );
        assert_eq!(
            matches(&item, &values(&["g*ease+News"]), "news"),
            values(&["g*ease"])
        );
        assert!(matches(&item, &values(&["go", "-security"]), "news").is_empty());
        assert_eq!(
            matches(&item, &values(&["go", "-#tsecurity"]), "news"),
            values(&["go"])
        );
    }

    #[test]
    fn wildcard_treats_other_regex_characters_as_literals() {
        let mut item = entry();
        item.title = "release(v123) file-final.txt [abc value".into();
        assert_eq!(
            matches(
                &item,
                &values(&["release(v*)", "file*.txt", "[abc*"]),
                "news"
            ),
            values(&["release(v*)", "file*.txt", "[abc*"])
        );
        assert!(matches(&item, &values(&["release.v*"]), "news").is_empty());
    }
}
