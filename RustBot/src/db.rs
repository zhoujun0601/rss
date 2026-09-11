use std::{
    collections::{HashMap, HashSet},
    path::Path,
    str::FromStr,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use sqlx::{
    Row, Sqlite, SqlitePool, Transaction,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use tracing::warn;

use crate::models::{
    DeliveryPart, DeliveryPartKind, FeedEntry, Keyword, PendingDelivery, Subscription,
    SubscriptionInfo,
};

pub const MAX_SUBSCRIPTIONS_PER_USER: i64 = 100;
pub const MAX_KEYWORDS_PER_USER: i64 = 200;
pub const MAX_KEYWORD_CHARS: usize = 128;
pub const MAX_SUBSCRIPTION_NAME_CHARS: usize = 64;
pub const MAX_URL_CHARS: usize = 2048;
const MAX_GLOBAL_SUBSCRIPTIONS: i64 = 5000;
const MAX_GLOBAL_PENDING: i64 = 100_000;
const MAX_PENDING_PER_USER: i64 = 1000;
const MAX_SEEN_PER_SUBSCRIPTION: i64 = 50_000;
const PENDING_BATCH_SIZE: i64 = 100;
const PENDING_PER_USER_BATCH: i64 = 10;

#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AddSubscriptionResult {
    Created,
    Joined,
}

impl Database {
    pub async fn connect(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))?
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(30))
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await
            .context("无法连接 SQLite")?;

        let subscriptions_exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='subscriptions'",
        )
        .fetch_one(&pool)
        .await?;
        let metadata_exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='app_metadata'",
        )
        .fetch_one(&pool)
        .await?;
        if subscriptions_exists > 0 && metadata_exists == 0 {
            pool.close().await;
            bail!("检测到旧版 Go 数据库；Rust 版不支持原地迁移，请移走 tgbot.db 后重试");
        }

        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("执行数据库迁移失败")?;
        Ok(Self { pool })
    }

    pub async fn add_subscription(
        &self,
        url: &str,
        name: &str,
        channel: bool,
        user_id: i64,
        seed_entries: &[FeedEntry],
    ) -> Result<AddSubscriptionResult> {
        validate_subscription_values(url, name)?;
        let mut tx = self.pool.begin().await?;
        let subscription_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM user_subscriptions WHERE user_id=?")
                .bind(user_id)
                .fetch_one(&mut *tx)
                .await?;
        if subscription_count >= MAX_SUBSCRIPTIONS_PER_USER {
            bail!("每个用户最多订阅 {MAX_SUBSCRIPTIONS_PER_USER} 个 RSS 源");
        }
        let existing = sqlx::query(
            "SELECT id, rss_url, rss_name, channel FROM subscriptions \
             WHERE rss_url = ? OR rss_name = ?",
        )
        .bind(url)
        .bind(name)
        .fetch_optional(&mut *tx)
        .await?;

        let (subscription_id, result) = if let Some(row) = existing {
            let existing_url: String = row.try_get("rss_url")?;
            let existing_name: String = row.try_get("rss_name")?;
            if existing_url != url || existing_name != name {
                bail!("订阅名称或 URL 已被其他订阅使用");
            }
            let existing_channel = row.try_get::<i64, _>("channel")? == 1;
            if existing_channel != channel {
                bail!("该共享订阅已使用不同的频道格式，请保持频道标记一致");
            }
            (row.try_get("id")?, AddSubscriptionResult::Joined)
        } else {
            let global_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM subscriptions")
                .fetch_one(&mut *tx)
                .await?;
            if global_count >= MAX_GLOBAL_SUBSCRIPTIONS {
                bail!("机器人订阅总数已达到上限 {MAX_GLOBAL_SUBSCRIPTIONS}");
            }
            let result = sqlx::query(
                "INSERT INTO subscriptions (rss_url, rss_name, channel) VALUES (?, ?, ?)",
            )
            .bind(url)
            .bind(name)
            .bind(i64::from(channel))
            .execute(&mut *tx)
            .await?;
            let id = result.last_insert_rowid();
            let now = Utc::now();
            let latest = seed_entries
                .iter()
                .max_by_key(|entry| entry.published_at)
                .map(|entry| (entry.published_at, entry.title.as_str()))
                .unwrap_or((now, ""));
            sqlx::query(
                "INSERT INTO feed_cursors (subscription_id, last_published_at, latest_title) VALUES (?, ?, ?)",
            )
            .bind(id)
            .bind(latest.0.to_rfc3339())
            .bind(latest.1)
            .execute(&mut *tx)
            .await?;
            for entry in seed_entries {
                insert_seen(&mut tx, id, entry).await?;
            }
            (id, AddSubscriptionResult::Created)
        };

        let insert = sqlx::query(
            "INSERT OR IGNORE INTO user_subscriptions (subscription_id, user_id) VALUES (?, ?)",
        )
        .bind(subscription_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
        if insert.rows_affected() == 0 {
            bail!("你已经订阅了这个 RSS 源");
        }
        if result == AddSubscriptionResult::Joined {
            for entry in seed_entries {
                sqlx::query(
                    "INSERT OR IGNORE INTO user_entry_baselines \
                     (subscription_id, user_id, entry_key) \
                     SELECT ?, ?, ? WHERE NOT EXISTS (\
                         SELECT 1 FROM seen_entries \
                         WHERE subscription_id=? AND entry_key=?\
                     )",
                )
                .bind(subscription_id)
                .bind(user_id)
                .bind(&entry.key)
                .bind(subscription_id)
                .bind(&entry.key)
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await?;
        Ok(result)
    }

    pub async fn check_subscription_capacity(
        &self,
        url: &str,
        name: &str,
        user_id: i64,
    ) -> Result<()> {
        validate_subscription_values(url, name)?;
        let subscription_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM user_subscriptions WHERE user_id=?")
                .bind(user_id)
                .fetch_one(&self.pool)
                .await?;
        if subscription_count >= MAX_SUBSCRIPTIONS_PER_USER {
            bail!("每个用户最多订阅 {MAX_SUBSCRIPTIONS_PER_USER} 个 RSS 源");
        }
        let existing: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM subscriptions WHERE rss_url=? AND rss_name=?")
                .bind(url)
                .bind(name)
                .fetch_one(&self.pool)
                .await?;
        if existing == 0 {
            let global_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM subscriptions")
                .fetch_one(&self.pool)
                .await?;
            if global_count >= MAX_GLOBAL_SUBSCRIPTIONS {
                bail!("机器人订阅总数已达到上限 {MAX_GLOBAL_SUBSCRIPTIONS}");
            }
        }
        Ok(())
    }

    pub async fn subscriptions(&self) -> Result<Vec<Subscription>> {
        let rows = sqlx::query(
            "SELECT s.id, s.rss_url, s.rss_name, s.channel, us.user_id \
             FROM subscriptions s LEFT JOIN user_subscriptions us ON us.subscription_id=s.id \
             ORDER BY s.id, us.user_id",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut output: Vec<Subscription> = Vec::new();
        for row in rows {
            let id: i64 = row.try_get("id")?;
            if output.last().is_none_or(|item| item.id != id) {
                output.push(Subscription {
                    id,
                    url: row.try_get("rss_url")?,
                    name: row.try_get("rss_name")?,
                    channel: row.try_get::<i64, _>("channel")? == 1,
                    users: Vec::new(),
                });
            }
            if let Some(user_id) = row.try_get::<Option<i64>, _>("user_id")? {
                output
                    .last_mut()
                    .expect("subscription was inserted")
                    .users
                    .push(user_id);
            }
        }
        Ok(output)
    }

    pub async fn subscriptions_for_user(&self, user_id: i64) -> Result<Vec<SubscriptionInfo>> {
        let rows = sqlx::query(
            "SELECT s.id, s.rss_name, s.rss_url FROM subscriptions s \
             JOIN user_subscriptions us ON us.subscription_id=s.id \
             WHERE us.user_id=? ORDER BY s.rss_name",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(SubscriptionInfo {
                    id: row.try_get("id")?,
                    name: row.try_get("rss_name")?,
                    url: row.try_get("rss_url")?,
                })
            })
            .collect()
    }

    pub async fn remove_subscription(&self, user_id: i64, subscription_id: i64) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let removed =
            sqlx::query("DELETE FROM user_subscriptions WHERE subscription_id=? AND user_id=?")
                .bind(subscription_id)
                .bind(user_id)
                .execute(&mut *tx)
                .await?
                .rows_affected();
        if removed == 0 {
            bail!("订阅不存在或不属于当前用户");
        }
        let remaining: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM user_subscriptions WHERE subscription_id=?")
                .bind(subscription_id)
                .fetch_one(&mut *tx)
                .await?;
        if remaining == 0 {
            sqlx::query("DELETE FROM subscriptions WHERE id=?")
                .bind(subscription_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(remaining == 0)
    }

    pub async fn keywords(&self, user_id: i64) -> Result<Vec<Keyword>> {
        let rows = sqlx::query("SELECT id, value FROM keywords WHERE user_id=? ORDER BY value")
            .bind(user_id)
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|row| {
                Ok(Keyword {
                    id: row.try_get("id")?,
                    value: row.try_get("value")?,
                })
            })
            .collect()
    }

    pub async fn keyword_values(&self, user_id: i64) -> Result<Vec<String>> {
        Ok(self
            .keywords(user_id)
            .await?
            .into_iter()
            .map(|item| item.value)
            .collect())
    }

    pub async fn add_keywords(&self, user_id: i64, values: &[String]) -> Result<usize> {
        if let Some(value) = values
            .iter()
            .find(|value| value.chars().count() > MAX_KEYWORD_CHARS)
        {
            bail!(
                "关键词不能超过 {MAX_KEYWORD_CHARS} 个字符: {}",
                truncate(value, 24)
            );
        }
        let mut tx = self.pool.begin().await?;
        let existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM keywords WHERE user_id=?")
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await?;
        let mut added = 0;
        for value in values {
            added += sqlx::query("INSERT OR IGNORE INTO keywords (user_id, value) VALUES (?, ?)")
                .bind(user_id)
                .bind(value)
                .execute(&mut *tx)
                .await?
                .rows_affected() as usize;
            if existing + added as i64 > MAX_KEYWORDS_PER_USER {
                bail!("每个用户最多保存 {MAX_KEYWORDS_PER_USER} 个关键词");
            }
        }
        tx.commit().await?;
        Ok(added)
    }

    pub async fn remove_keyword(&self, user_id: i64, keyword_id: i64) -> Result<bool> {
        let removed = sqlx::query("DELETE FROM keywords WHERE id=? AND user_id=?")
            .bind(keyword_id)
            .bind(user_id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(removed > 0)
    }

    pub async fn enqueue_entries(
        &self,
        subscription: &Subscription,
        entries: &[FeedEntry],
        user_keywords: &[(i64, Vec<String>)],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let now = Utc::now().to_rfc3339();
        let mut latest: Option<&FeedEntry> = None;
        let mut pending_counts = HashMap::new();
        let mut capped_users = HashSet::new();
        let mut global_pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pending_deliveries")
            .fetch_one(&mut *tx)
            .await?;
        for (user_id, _) in user_keywords {
            let count: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM pending_deliveries WHERE user_id=?")
                    .bind(user_id)
                    .fetch_one(&mut *tx)
                    .await?;
            pending_counts.insert(*user_id, count);
        }
        for entry in entries {
            let inserted = insert_seen(&mut tx, subscription.id, entry).await?;
            if !inserted {
                clear_entry_baselines(&mut tx, subscription.id, &entry.key).await?;
                continue;
            }
            if latest.is_none_or(|item| entry.published_at > item.published_at) {
                latest = Some(entry);
            }
            for (user_id, keywords) in user_keywords {
                let pending_count = pending_counts.entry(*user_id).or_default();
                if *pending_count >= MAX_PENDING_PER_USER || global_pending >= MAX_GLOBAL_PENDING {
                    if capped_users.insert(*user_id) {
                        warn!(
                            user_id,
                            subscription_id = subscription.id,
                            "待投递队列已达到容量上限，跳过新条目"
                        );
                    }
                    continue;
                }
                let is_join_baseline: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM user_entry_baselines \
                     WHERE subscription_id=? AND user_id=? AND entry_key=?)",
                )
                .bind(subscription.id)
                .bind(user_id)
                .bind(&entry.key)
                .fetch_one(&mut *tx)
                .await?;
                if is_join_baseline {
                    continue;
                }
                if crate::keywords::matches(entry, keywords, &subscription.name).is_empty() {
                    continue;
                }
                let inserted = sqlx::query(
                    "INSERT OR IGNORE INTO pending_deliveries \
                     (subscription_id, entry_key, user_id, title, description, link, published_at, created_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(subscription.id)
                .bind(&entry.key)
                .bind(user_id)
                .bind(&entry.title)
                .bind(&entry.description)
                .bind(&entry.link)
                .bind(entry.published_at.to_rfc3339())
                .bind(&now)
                .execute(&mut *tx)
                .await?;
                *pending_count += inserted.rows_affected() as i64;
                global_pending += inserted.rows_affected() as i64;
            }
            clear_entry_baselines(&mut tx, subscription.id, &entry.key).await?;
        }
        if let Some(entry) = latest {
            sqlx::query(
                "UPDATE feed_cursors SET last_published_at=?, latest_title=? WHERE subscription_id=?",
            )
            .bind(entry.published_at.to_rfc3339())
            .bind(&entry.title)
            .bind(subscription.id)
            .execute(&mut *tx)
                .await?;
        }
        sqlx::query(
            "DELETE FROM seen_entries WHERE rowid IN (\
                 SELECT rowid FROM seen_entries WHERE subscription_id=? \
                 ORDER BY seen_at DESC, rowid DESC LIMIT -1 OFFSET ?\
             )",
        )
        .bind(subscription.id)
        .bind(MAX_SEEN_PER_SUBSCRIPTION)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn pending_for_subscription(
        &self,
        subscription_id: i64,
    ) -> Result<Vec<PendingDelivery>> {
        let rows = sqlx::query(
            "WITH ranked AS (\
                 SELECT p.subscription_id, s.rss_name, s.channel, p.entry_key, p.user_id, \
                        p.title, p.description, p.link, p.published_at, p.attempt_count, \
                        p.created_at, \
                        ROW_NUMBER() OVER (PARTITION BY p.user_id ORDER BY p.created_at, p.entry_key) AS user_rank \
                 FROM pending_deliveries p JOIN subscriptions s ON s.id=p.subscription_id \
                 WHERE p.subscription_id=? AND (p.next_attempt_at='' OR p.next_attempt_at<=?)\
             ) \
             SELECT * FROM ranked WHERE user_rank<=? \
             ORDER BY user_rank, created_at, user_id LIMIT ?",
        )
        .bind(subscription_id)
        .bind(Utc::now().to_rfc3339())
        .bind(PENDING_PER_USER_BATCH)
        .bind(PENDING_BATCH_SIZE)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let raw: String = row.try_get("published_at")?;
                let published_at = DateTime::parse_from_rfc3339(&raw)?.with_timezone(&Utc);
                Ok(PendingDelivery {
                    subscription_id: row.try_get("subscription_id")?,
                    subscription_name: row.try_get("rss_name")?,
                    channel: row.try_get::<i64, _>("channel")? == 1,
                    entry: FeedEntry {
                        key: row.try_get("entry_key")?,
                        title: row.try_get("title")?,
                        description: row.try_get("description")?,
                        link: row.try_get("link")?,
                        published_at,
                    },
                    user_id: row.try_get("user_id")?,
                    attempt_count: row.try_get::<i64, _>("attempt_count")?.max(0) as u32,
                })
            })
            .collect()
    }

    pub async fn complete_delivery(&self, delivery: &PendingDelivery) -> Result<()> {
        sqlx::query(
            "DELETE FROM pending_deliveries WHERE subscription_id=? AND entry_key=? AND user_id=?",
        )
        .bind(delivery.subscription_id)
        .bind(&delivery.entry.key)
        .bind(delivery.user_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn fail_delivery(&self, delivery: &PendingDelivery, error: &str) -> Result<()> {
        let exponent = delivery.attempt_count.min(7);
        let delay_seconds = 30_i64.saturating_mul(1_i64 << exponent).min(3600);
        let next_attempt_at = (Utc::now() + chrono::Duration::seconds(delay_seconds)).to_rfc3339();
        sqlx::query(
            "UPDATE pending_deliveries SET attempt_count=attempt_count+1, last_error=?, next_attempt_at=? \
             WHERE subscription_id=? AND entry_key=? AND user_id=?",
        )
        .bind(truncate(error, 2000))
        .bind(next_attempt_at)
        .bind(delivery.subscription_id)
        .bind(&delivery.entry.key)
        .bind(delivery.user_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delivery_parts(&self, delivery: &PendingDelivery) -> Result<Vec<DeliveryPart>> {
        let rows = sqlx::query(
            "SELECT part_index, kind, content, media_url FROM pending_delivery_parts \
             WHERE subscription_id=? AND entry_key=? AND user_id=? ORDER BY part_index",
        )
        .bind(delivery.subscription_id)
        .bind(&delivery.entry.key)
        .bind(delivery.user_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let raw_kind: String = row.try_get("kind")?;
                let kind = DeliveryPartKind::parse(&raw_kind)
                    .with_context(|| format!("未知的投递分段类型: {raw_kind}"))?;
                Ok(DeliveryPart {
                    index: row.try_get("part_index")?,
                    kind,
                    content: row.try_get("content")?,
                    media_url: row.try_get("media_url")?,
                })
            })
            .collect()
    }

    pub async fn initialize_delivery_parts(
        &self,
        delivery: &PendingDelivery,
        parts: &[DeliveryPart],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        insert_delivery_parts(&mut tx, delivery, parts, true).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn replace_delivery_parts(
        &self,
        delivery: &PendingDelivery,
        parts: &[DeliveryPart],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "DELETE FROM pending_delivery_parts \
             WHERE subscription_id=? AND entry_key=? AND user_id=?",
        )
        .bind(delivery.subscription_id)
        .bind(&delivery.entry.key)
        .bind(delivery.user_id)
        .execute(&mut *tx)
        .await?;
        insert_delivery_parts(&mut tx, delivery, parts, false).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn complete_delivery_part(
        &self,
        delivery: &PendingDelivery,
        part_index: i64,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "DELETE FROM pending_delivery_parts \
             WHERE subscription_id=? AND entry_key=? AND user_id=? AND part_index=?",
        )
        .bind(delivery.subscription_id)
        .bind(&delivery.entry.key)
        .bind(delivery.user_id)
        .bind(part_index)
        .execute(&mut *tx)
        .await?;
        let remaining: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pending_delivery_parts \
             WHERE subscription_id=? AND entry_key=? AND user_id=?",
        )
        .bind(delivery.subscription_id)
        .bind(&delivery.entry.key)
        .bind(delivery.user_id)
        .fetch_one(&mut *tx)
        .await?;
        if remaining == 0 {
            sqlx::query(
                "DELETE FROM pending_deliveries \
                 WHERE subscription_id=? AND entry_key=? AND user_id=?",
            )
            .bind(delivery.subscription_id)
            .bind(&delivery.entry.key)
            .bind(delivery.user_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn remove_delivery_if_stale(
        &self,
        delivery: &PendingDelivery,
        matched: bool,
    ) -> Result<bool> {
        let subscribed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM user_subscriptions WHERE subscription_id=? AND user_id=?",
        )
        .bind(delivery.subscription_id)
        .bind(delivery.user_id)
        .fetch_one(&self.pool)
        .await?;
        if subscribed == 0 || !matched {
            self.complete_delivery(delivery).await?;
            return Ok(true);
        }
        Ok(false)
    }

    pub async fn stats(&self, user_id: i64) -> Result<(i64, i64)> {
        let subscriptions: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM user_subscriptions WHERE user_id=?")
                .bind(user_id)
                .fetch_one(&self.pool)
                .await?;
        let keywords: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM keywords WHERE user_id=?")
            .bind(user_id)
            .fetch_one(&self.pool)
            .await?;
        Ok((subscriptions, keywords))
    }

    #[cfg(test)]
    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

async fn insert_delivery_parts(
    tx: &mut Transaction<'_, Sqlite>,
    delivery: &PendingDelivery,
    parts: &[DeliveryPart],
    ignore_existing: bool,
) -> Result<()> {
    let statement = if ignore_existing {
        "INSERT OR IGNORE INTO pending_delivery_parts \
         (subscription_id, entry_key, user_id, part_index, kind, content, media_url) \
         VALUES (?, ?, ?, ?, ?, ?, ?)"
    } else {
        "INSERT INTO pending_delivery_parts \
         (subscription_id, entry_key, user_id, part_index, kind, content, media_url) \
         VALUES (?, ?, ?, ?, ?, ?, ?)"
    };
    for part in parts {
        sqlx::query(statement)
            .bind(delivery.subscription_id)
            .bind(&delivery.entry.key)
            .bind(delivery.user_id)
            .bind(part.index)
            .bind(part.kind.as_str())
            .bind(&part.content)
            .bind(&part.media_url)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

fn validate_subscription_values(url: &str, name: &str) -> Result<()> {
    if url.chars().count() > MAX_URL_CHARS {
        bail!("RSS URL 不能超过 {MAX_URL_CHARS} 个字符");
    }
    if name.is_empty() || name.chars().count() > MAX_SUBSCRIPTION_NAME_CHARS {
        bail!("订阅名称必须为 1 到 {MAX_SUBSCRIPTION_NAME_CHARS} 个字符");
    }
    Ok(())
}

async fn insert_seen(
    tx: &mut Transaction<'_, Sqlite>,
    subscription_id: i64,
    entry: &FeedEntry,
) -> Result<bool> {
    let result = sqlx::query(
        "INSERT OR IGNORE INTO seen_entries \
         (subscription_id, entry_key, published_at, seen_at) VALUES (?, ?, ?, ?)",
    )
    .bind(subscription_id)
    .bind(&entry.key)
    .bind(entry.published_at.to_rfc3339())
    .bind(Utc::now().to_rfc3339())
    .execute(&mut **tx)
    .await?;
    Ok(result.rows_affected() > 0)
}

async fn clear_entry_baselines(
    tx: &mut Transaction<'_, Sqlite>,
    subscription_id: i64,
    entry_key: &str,
) -> Result<()> {
    sqlx::query("DELETE FROM user_entry_baselines WHERE subscription_id=? AND entry_key=?")
        .bind(subscription_id)
        .bind(entry_key)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use tempfile::TempDir;

    use super::*;

    fn entry(key: &str, title: &str) -> FeedEntry {
        FeedEntry {
            key: key.to_owned(),
            title: title.to_owned(),
            description: "description".to_owned(),
            link: format!("https://example.com/{key}"),
            published_at: Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
        }
    }

    async fn database() -> (TempDir, std::path::PathBuf, Database) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("test.db");
        let database = Database::connect(&path).await.unwrap();
        (directory, path, database)
    }

    #[tokio::test]
    async fn seeds_existing_entries_and_enqueues_only_new_matches() {
        let (_directory, _path, db) = database().await;
        db.add_subscription(
            "https://example.com/feed",
            "news",
            false,
            42,
            &[entry("old", "Old")],
        )
        .await
        .unwrap();
        db.add_keywords(42, &["*".to_owned()]).await.unwrap();
        let subscription = db.subscriptions().await.unwrap().remove(0);
        db.enqueue_entries(
            &subscription,
            &[entry("old", "Old"), entry("new", "New")],
            &[(42, vec!["*".to_owned()])],
        )
        .await
        .unwrap();
        let pending = db.pending_for_subscription(subscription.id).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].entry.key, "new");
    }

    #[tokio::test]
    async fn joining_user_does_not_receive_entries_seen_during_join() {
        let (_directory, _path, db) = database().await;
        db.add_subscription(
            "https://example.com/feed",
            "news",
            false,
            1,
            &[entry("old", "Old")],
        )
        .await
        .unwrap();
        db.add_keywords(1, &["*".to_owned()]).await.unwrap();

        db.add_subscription(
            "https://example.com/feed",
            "news",
            false,
            2,
            &[entry("old", "Old"), entry("before-join", "Before join")],
        )
        .await
        .unwrap();
        db.add_keywords(2, &["*".to_owned()]).await.unwrap();

        let subscription = db.subscriptions().await.unwrap().remove(0);
        db.enqueue_entries(
            &subscription,
            &[entry("before-join", "Before join")],
            &[(1, vec!["*".to_owned()]), (2, vec!["*".to_owned()])],
        )
        .await
        .unwrap();
        let pending = db.pending_for_subscription(subscription.id).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].user_id, 1);

        db.enqueue_entries(
            &subscription,
            &[entry("after-join", "After join")],
            &[(1, vec!["*".to_owned()]), (2, vec!["*".to_owned()])],
        )
        .await
        .unwrap();
        let pending = db.pending_for_subscription(subscription.id).await.unwrap();
        assert_eq!(pending.len(), 3);
        assert!(
            pending
                .iter()
                .any(|item| { item.user_id == 2 && item.entry.key == "after-join" })
        );
    }

    #[tokio::test]
    async fn rejects_conflicting_channel_mode_for_shared_subscription() {
        let (_directory, _path, db) = database().await;
        db.add_subscription("https://example.com/feed", "news", false, 1, &[])
            .await
            .unwrap();
        let error = db
            .add_subscription("https://example.com/feed", "news", true, 2, &[])
            .await
            .unwrap_err();
        assert!(error.to_string().contains("频道标记一致"));
        assert!(db.subscriptions_for_user(2).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn pending_delivery_survives_reconnect() {
        let (directory, path, db) = database().await;
        db.add_subscription("https://example.com/feed", "news", false, 7, &[])
            .await
            .unwrap();
        db.add_keywords(7, &["*".to_owned()]).await.unwrap();
        let subscription = db.subscriptions().await.unwrap().remove(0);
        db.enqueue_entries(
            &subscription,
            &[entry("persistent", "Persistent")],
            &[(7, vec!["*".to_owned()])],
        )
        .await
        .unwrap();
        db.pool.close().await;

        let reopened = Database::connect(&path).await.unwrap();
        assert_eq!(
            reopened
                .pending_for_subscription(subscription.id)
                .await
                .unwrap()
                .len(),
            1
        );
        drop(directory);
    }

    #[tokio::test]
    async fn deleting_last_subscriber_cascades_runtime_data() {
        let (_directory, _path, db) = database().await;
        db.add_subscription("https://example.com/feed", "news", false, 7, &[])
            .await
            .unwrap();
        db.add_keywords(7, &["*".to_owned()]).await.unwrap();
        let subscription = db.subscriptions().await.unwrap().remove(0);
        db.enqueue_entries(
            &subscription,
            &[entry("new", "New")],
            &[(7, vec!["*".to_owned()])],
        )
        .await
        .unwrap();
        assert!(db.remove_subscription(7, subscription.id).await.unwrap());
        let subscriptions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM subscriptions")
            .fetch_one(db.pool())
            .await
            .unwrap();
        let seen: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM seen_entries")
            .fetch_one(db.pool())
            .await
            .unwrap();
        let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pending_deliveries")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!((subscriptions, seen, pending), (0, 0, 0));
    }

    #[tokio::test]
    async fn refuses_legacy_go_schema() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("legacy.db");
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(options).await.unwrap();
        sqlx::query("CREATE TABLE subscriptions (subscription_id INTEGER PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
        let error = match Database::connect(path).await {
            Ok(_) => panic!("legacy database was accepted"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("旧版 Go 数据库"));
    }

    #[tokio::test]
    async fn failed_delivery_is_deferred_instead_of_immediately_retried() {
        let (_directory, _path, db) = database().await;
        db.add_subscription("https://example.com/feed", "news", false, 7, &[])
            .await
            .unwrap();
        db.add_keywords(7, &["*".to_owned()]).await.unwrap();
        let subscription = db.subscriptions().await.unwrap().remove(0);
        db.enqueue_entries(
            &subscription,
            &[entry("retry", "Retry")],
            &[(7, vec!["*".to_owned()])],
        )
        .await
        .unwrap();
        let delivery = db
            .pending_for_subscription(subscription.id)
            .await
            .unwrap()
            .remove(0);
        db.fail_delivery(&delivery, "temporary failure")
            .await
            .unwrap();

        assert!(
            db.pending_for_subscription(subscription.id)
                .await
                .unwrap()
                .is_empty()
        );
        let attempt_count: i64 = sqlx::query_scalar("SELECT attempt_count FROM pending_deliveries")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(attempt_count, 1);
    }

    #[tokio::test]
    async fn completed_delivery_parts_survive_reconnect() {
        let (directory, path, db) = database().await;
        db.add_subscription("https://example.com/feed", "news", false, 7, &[])
            .await
            .unwrap();
        db.add_keywords(7, &["*".to_owned()]).await.unwrap();
        let subscription = db.subscriptions().await.unwrap().remove(0);
        db.enqueue_entries(
            &subscription,
            &[entry("parts", "Parts")],
            &[(7, vec!["*".to_owned()])],
        )
        .await
        .unwrap();
        let delivery = db
            .pending_for_subscription(subscription.id)
            .await
            .unwrap()
            .remove(0);
        let parts = vec![
            DeliveryPart {
                index: 0,
                kind: DeliveryPartKind::TextPlain,
                content: "first".into(),
                media_url: String::new(),
            },
            DeliveryPart {
                index: 1,
                kind: DeliveryPartKind::TextPlain,
                content: "second".into(),
                media_url: String::new(),
            },
        ];
        db.initialize_delivery_parts(&delivery, &parts)
            .await
            .unwrap();
        db.complete_delivery_part(&delivery, 0).await.unwrap();
        db.pool.close().await;

        let reopened = Database::connect(&path).await.unwrap();
        let delivery = reopened
            .pending_for_subscription(subscription.id)
            .await
            .unwrap()
            .remove(0);
        let remaining = reopened.delivery_parts(&delivery).await.unwrap();
        assert_eq!(remaining, vec![parts[1].clone()]);
        reopened.complete_delivery_part(&delivery, 1).await.unwrap();
        let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pending_deliveries")
            .fetch_one(reopened.pool())
            .await
            .unwrap();
        assert_eq!(pending, 0);
        drop(directory);
    }

    #[tokio::test]
    async fn rejects_oversized_user_values() {
        let (_directory, _path, db) = database().await;
        let keyword = "x".repeat(MAX_KEYWORD_CHARS + 1);
        assert!(db.add_keywords(7, &[keyword]).await.is_err());
        let name = "x".repeat(MAX_SUBSCRIPTION_NAME_CHARS + 1);
        assert!(
            db.add_subscription("https://example.com/feed", &name, false, 7, &[])
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn pending_batch_is_bounded_and_fair_between_users() {
        let (_directory, _path, db) = database().await;
        db.add_subscription("https://example.com/feed", "news", false, 1, &[])
            .await
            .unwrap();
        db.add_subscription("https://example.com/feed", "news", false, 2, &[])
            .await
            .unwrap();
        db.add_keywords(1, &["*".to_owned()]).await.unwrap();
        db.add_keywords(2, &["*".to_owned()]).await.unwrap();
        let subscription = db.subscriptions().await.unwrap().remove(0);
        let entries: Vec<_> = (0..11)
            .map(|index| entry(&format!("entry-{index}"), "Entry"))
            .collect();
        db.enqueue_entries(
            &subscription,
            &entries,
            &[(1, vec!["*".to_owned()]), (2, vec!["*".to_owned()])],
        )
        .await
        .unwrap();

        let pending = db.pending_for_subscription(subscription.id).await.unwrap();
        assert_eq!(pending.len(), 20);
        assert_eq!(pending.iter().filter(|item| item.user_id == 1).count(), 10);
        assert_eq!(pending.iter().filter(|item| item.user_id == 2).count(), 10);
    }
}
