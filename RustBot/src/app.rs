use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use chrono::Utc;
use reqwest::Client;
use teloxide::Bot;
use tokio::sync::{Mutex, OwnedSemaphorePermit, RwLock, Semaphore};
use tracing::{error, info, warn};

use crate::{
    config::Config,
    db::Database,
    feed::FeedClient,
    keywords,
    models::{PendingDelivery, Subscription},
    security, telegram,
};

#[derive(Clone, Copy, Debug)]
pub enum InputState {
    AddKeyword,
    AddSubscription,
}

#[derive(Debug)]
struct PushStats {
    date: chrono::NaiveDate,
    total: u64,
    by_rss: HashMap<String, u64>,
}

pub struct App {
    pub bot: Bot,
    pub db: Database,
    pub config: Config,
    pub feed: FeedClient,
    pub http: Client,
    pub input_states: RwLock<HashMap<i64, InputState>>,
    stats: Mutex<PushStats>,
    poll_lock: Mutex<()>,
    concurrency: Arc<Semaphore>,
    delivery_concurrency: Arc<Semaphore>,
    subscription_add_concurrency: Arc<Semaphore>,
    subscription_add_attempts: Mutex<HashMap<i64, Instant>>,
}

const SUBSCRIPTION_ADD_COOLDOWN: Duration = Duration::from_secs(5);
const DELIVERY_CONCURRENCY: usize = 8;

impl App {
    pub fn new(bot: Bot, db: Database, config: Config, feed: FeedClient, http: Client) -> Self {
        let today = Utc::now().with_timezone(&config.timezone()).date_naive();
        Self {
            bot,
            db,
            config,
            feed,
            http,
            input_states: RwLock::new(HashMap::new()),
            stats: Mutex::new(PushStats {
                date: today,
                total: 0,
                by_rss: HashMap::new(),
            }),
            poll_lock: Mutex::new(()),
            concurrency: Arc::new(Semaphore::new(8)),
            delivery_concurrency: Arc::new(Semaphore::new(DELIVERY_CONCURRENCY)),
            subscription_add_concurrency: Arc::new(Semaphore::new(4)),
            subscription_add_attempts: Mutex::new(HashMap::new()),
        }
    }

    pub async fn set_input_state(&self, user_id: i64, state: InputState) {
        self.input_states.write().await.insert(user_id, state);
    }

    pub async fn take_input_state(&self, user_id: i64) -> Option<InputState> {
        self.input_states.write().await.remove(&user_id)
    }

    pub async fn clear_input_state(&self, user_id: i64) {
        self.input_states.write().await.remove(&user_id);
    }

    pub async fn claim_subscription_add(&self, user_id: i64) -> Result<OwnedSemaphorePermit> {
        let now = Instant::now();
        let mut attempts = self.subscription_add_attempts.lock().await;
        attempts.retain(|_, attempted_at| {
            now.saturating_duration_since(*attempted_at) < Duration::from_secs(3600)
        });
        if attempts.get(&user_id).is_some_and(|attempted_at| {
            now.saturating_duration_since(*attempted_at) < SUBSCRIPTION_ADD_COOLDOWN
        }) {
            anyhow::bail!("添加订阅过于频繁，请稍后重试");
        }
        attempts.insert(user_id, now);
        drop(attempts);
        Arc::clone(&self.subscription_add_concurrency)
            .try_acquire_owned()
            .context("当前添加订阅请求过多，请稍后重试")
    }

    pub async fn stats_text(&self) -> String {
        let mut stats = self.stats.lock().await;
        let today = Utc::now()
            .with_timezone(&self.config.timezone())
            .date_naive();
        if stats.date != today {
            stats.date = today;
            stats.total = 0;
            stats.by_rss.clear();
        }
        let mut output = format!("📊 今日({})推送总计：{} 次", stats.date, stats.total);
        let mut rows: Vec<_> = stats.by_rss.iter().collect();
        rows.sort_by_key(|(name, _)| *name);
        for (name, count) in rows {
            output.push_str(&format!("\n📊 {name}: {count} 次"));
        }
        output
    }

    async fn record_push(&self, rss_name: &str) {
        let mut stats = self.stats.lock().await;
        let today = Utc::now()
            .with_timezone(&self.config.timezone())
            .date_naive();
        if stats.date != today {
            stats.date = today;
            stats.total = 0;
            stats.by_rss.clear();
        }
        stats.total += 1;
        *stats.by_rss.entry(rss_name.to_owned()).or_default() += 1;
    }

    pub async fn run_monitor(self: Arc<Self>) {
        self.poll_once().await;
        info!(interval_seconds = self.config.Cycletime, "RSS 监控已启动");
        let mut interval = tokio::time::interval(Duration::from_secs(self.config.Cycletime));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval.tick().await;
        loop {
            interval.tick().await;
            self.poll_once().await;
        }
    }

    pub async fn poll_once(self: &Arc<Self>) {
        let Ok(_guard) = self.poll_lock.try_lock() else {
            warn!("上一次 RSS 检查尚未结束，跳过本轮");
            return;
        };
        info!("开始检查 RSS 订阅");
        let subscriptions = match self.db.subscriptions().await {
            Ok(items) => items,
            Err(error) => {
                error!(%error, "读取订阅失败");
                return;
            }
        };
        let mut tasks = tokio::task::JoinSet::new();
        for subscription in subscriptions {
            let app = Arc::clone(self);
            let permit = Arc::clone(&self.concurrency).acquire_owned().await;
            tasks.spawn(async move {
                let _permit = permit.context("轮询并发控制器已关闭")?;
                app.process_subscription(subscription).await
            });
        }
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => error!(%error, "处理订阅失败"),
                Err(error) => error!(%error, "订阅任务异常退出"),
            }
        }
        info!("RSS 检查完成");
    }

    async fn process_subscription(self: &Arc<Self>, subscription: Subscription) -> Result<()> {
        let entries = match self.feed.fetch(&subscription.url).await {
            Ok(entries) => entries,
            Err(error) => {
                warn!(rss = %subscription.name, %error, "抓取 Feed 失败");
                return self.deliver_pending(subscription.id).await;
            }
        };
        let mut user_keywords = Vec::with_capacity(subscription.users.len());
        for user_id in &subscription.users {
            user_keywords.push((*user_id, self.db.keyword_values(*user_id).await?));
        }
        self.db
            .enqueue_entries(&subscription, &entries, &user_keywords)
            .await?;
        self.deliver_pending(subscription.id).await?;
        Ok(())
    }

    async fn deliver_pending(self: &Arc<Self>, subscription_id: i64) -> Result<()> {
        let mut deliveries_by_user = BTreeMap::<i64, Vec<PendingDelivery>>::new();
        for delivery in self.db.pending_for_subscription(subscription_id).await? {
            deliveries_by_user
                .entry(delivery.user_id)
                .or_default()
                .push(delivery);
        }

        let mut tasks = tokio::task::JoinSet::new();
        for (user_id, deliveries) in deliveries_by_user {
            let app = Arc::clone(self);
            let concurrency = Arc::clone(&self.delivery_concurrency);
            tasks.spawn(async move {
                let result = async {
                    let _permit = concurrency
                        .acquire_owned()
                        .await
                        .context("投递并发控制器已关闭")?;
                    for delivery in deliveries {
                        app.deliver_one(delivery).await?;
                    }
                    Ok::<_, anyhow::Error>(())
                }
                .await;
                (user_id, result)
            });
        }
        while let Some(result) = tasks.join_next().await {
            log_delivery_task_result(result);
        }
        Ok(())
    }

    async fn deliver_one(&self, delivery: PendingDelivery) -> Result<()> {
        let current_keywords = self.db.keyword_values(delivery.user_id).await?;
        let matched = keywords::matches(
            &delivery.entry,
            &current_keywords,
            &delivery.subscription_name,
        );
        if self
            .db
            .remove_delivery_if_stale(&delivery, !matched.is_empty())
            .await?
        {
            return Ok(());
        }
        match telegram::deliver_feed_entry(self, &delivery, &matched).await {
            Ok(()) => {
                self.db.complete_delivery(&delivery).await?;
                self.record_push(&delivery.subscription_name).await;
                self.send_pushinfo(&delivery).await;
            }
            Err(error) => {
                warn!(user_id = delivery.user_id, rss = %delivery.subscription_name, %error, "发送失败，将在下轮重试");
                self.db.fail_delivery(&delivery, &error.to_string()).await?;
            }
        }
        Ok(())
    }

    async fn send_pushinfo(&self, delivery: &PendingDelivery) {
        if self.config.Pushinfo.is_empty()
            || self.config.ADMINIDS == 0
            || delivery.user_id != self.config.ADMINIDS
        {
            return;
        }
        let message = format!(
            "📌 {}\n🕒 {}\n🔗 {}",
            delivery.entry.title,
            delivery
                .entry
                .published_at
                .with_timezone(&self.config.timezone())
                .format("%F %T"),
            delivery.entry.link
        );
        let encoded: String = url::form_urlencoded::byte_serialize(message.as_bytes()).collect();
        let target = format!("{}{encoded}", self.config.Pushinfo);
        match self.http.get(target).send().await {
            Ok(response) if response.status().is_success() => {}
            Ok(response) => warn!(status = %response.status(), "Pushinfo 返回非成功状态"),
            Err(error) => {
                let error = error.without_url();
                warn!(%error, "Pushinfo 请求失败");
            }
        }
    }
}

fn log_delivery_task_result(
    result: std::result::Result<(i64, Result<()>), tokio::task::JoinError>,
) {
    match result {
        Ok((_, Ok(()))) => {}
        Ok((user_id, Err(error))) => error!(user_id, %error, "用户投递任务失败"),
        Err(error) => error!(%error, "用户投递任务异常退出"),
    }
}

pub fn build_clients(config: &Config) -> Result<(Client, FeedClient)> {
    let http = security::standard_client(
        (!config.ProxyURL.trim().is_empty()).then_some(config.ProxyURL.as_str()),
        Duration::from_secs(60),
    )?;
    Ok((http, FeedClient::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

    use crate::models::FeedEntry;

    #[tokio::test]
    async fn subscription_adds_are_rate_limited_per_user() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::connect(directory.path().join("rate-limit.db"))
            .await
            .unwrap();
        let config = Config {
            BotToken: "123:test".into(),
            ..Config::default()
        };
        let (http, feed) = build_clients(&config).unwrap();
        let bot = Bot::with_client(config.BotToken.clone(), http.clone());
        let app = App::new(bot, database, config, feed, http);

        drop(app.claim_subscription_add(7).await.unwrap());
        assert!(app.claim_subscription_add(7).await.is_err());
        assert!(app.claim_subscription_add(8).await.is_ok());
    }

    #[tokio::test]
    async fn slow_delivery_for_one_user_does_not_serialize_other_users() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(403)
                    .set_delay(Duration::from_millis(400))
                    .set_body_json(json!({
                        "ok": false,
                        "error_code": 403,
                        "description": "blocked"
                    })),
            )
            .expect(2)
            .mount(&server)
            .await;

        let directory = tempfile::tempdir().unwrap();
        let database = Database::connect(directory.path().join("delivery-concurrency.db"))
            .await
            .unwrap();
        database
            .add_subscription("https://example.com/feed", "news", false, 1, &[])
            .await
            .unwrap();
        database
            .add_subscription("https://example.com/feed", "news", false, 2, &[])
            .await
            .unwrap();
        database.add_keywords(1, &["*".into()]).await.unwrap();
        database.add_keywords(2, &["*".into()]).await.unwrap();
        let subscription = database.subscriptions().await.unwrap().remove(0);
        database
            .enqueue_entries(
                &subscription,
                &[FeedEntry {
                    key: "concurrent".into(),
                    title: "Concurrent".into(),
                    description: String::new(),
                    link: "https://example.com/item".into(),
                    published_at: Utc::now(),
                }],
                &[(1, vec!["*".into()]), (2, vec!["*".into()])],
            )
            .await
            .unwrap();

        let config = Config {
            BotToken: "123:test".into(),
            ..Config::default()
        };
        let (http, feed) = build_clients(&config).unwrap();
        let bot = Bot::with_client(config.BotToken.clone(), http.clone())
            .set_api_url(format!("{}/", server.uri()).parse().unwrap());
        let app = Arc::new(App::new(bot, database, config, feed, http));

        let started = Instant::now();
        app.deliver_pending(subscription.id).await.unwrap();
        assert!(
            started.elapsed() < Duration::from_millis(700),
            "different users were delivered serially"
        );
    }
}
