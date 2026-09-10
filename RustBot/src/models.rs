use chrono::{DateTime, Utc};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedEntry {
    pub key: String,
    pub title: String,
    pub description: String,
    pub link: String,
    pub published_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subscription {
    pub id: i64,
    pub url: String,
    pub name: String,
    pub channel: bool,
    pub users: Vec<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubscriptionInfo {
    pub id: i64,
    pub name: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keyword {
    pub id: i64,
    pub value: String,
}

#[derive(Clone, Debug)]
pub struct PendingDelivery {
    pub subscription_id: i64,
    pub subscription_name: String,
    pub channel: bool,
    pub entry: FeedEntry,
    pub user_id: i64,
    pub attempt_count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryPartKind {
    TextHtml,
    TextPlain,
    Photo,
}

impl DeliveryPartKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TextHtml => "text_html",
            Self::TextPlain => "text_plain",
            Self::Photo => "photo",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "text_html" => Some(Self::TextHtml),
            "text_plain" => Some(Self::TextPlain),
            "photo" => Some(Self::Photo),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeliveryPart {
    pub index: i64,
    pub kind: DeliveryPartKind,
    pub content: String,
    pub media_url: String,
}
