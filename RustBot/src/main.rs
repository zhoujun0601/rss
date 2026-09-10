mod app;
mod config;
mod content;
mod db;
mod feed;
mod keywords;
mod models;
mod security;
mod telegram;

use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use app::{App, build_clients};
use config::Config;
use db::Database;
use teloxide::{Bot, prelude::Requester};
use tracing::info;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

const VERSION: &str = match option_env!("TGBOT_VERSION") {
    Some(value) => value,
    None => env!("CARGO_PKG_VERSION"),
};
const GIT_COMMIT: &str = match option_env!("TGBOT_GIT_COMMIT") {
    Some(value) => value,
    None => "unknown",
};
const BUILD_TIME: &str = match option_env!("TGBOT_BUILD_TIME") {
    Some(value) => value,
    None => "unknown",
};

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--version") {
        println!("TGBot_RSS {VERSION} ({GIT_COMMIT}, {BUILD_TIME})");
        return Ok(());
    }
    let config_path = config_path();
    let config = Config::load(&config_path)?;
    if args.iter().any(|arg| arg == "--check-config") {
        println!("配置有效: {}", config_path.display());
        return Ok(());
    }

    let _log_guard = init_logging(config.Debug)?;
    info!(
        version = VERSION,
        git_commit = GIT_COMMIT,
        "TGBot_RSS Rust 版启动"
    );
    let database = Database::connect("tgbot.db").await?;
    let (http, feed) = build_clients(&config)?;
    let bot = Bot::with_client(config.BotToken.clone(), http.clone());
    let me = bot.get_me().await.context("连接 Telegram Bot API 失败")?;
    info!(username = ?me.user.username, "Telegram Bot 已连接");

    let app = Arc::new(App::new(bot, database, config, feed, http));
    let monitor = tokio::spawn(Arc::clone(&app).run_monitor());
    telegram::run(app).await;
    monitor.abort();
    Ok(())
}

fn config_path() -> PathBuf {
    std::env::var_os("TGBOT_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("config.json"))
}

fn init_logging(debug: bool) -> Result<tracing_appender::non_blocking::WorkerGuard> {
    let filter = EnvFilter::try_new(if debug { "debug" } else { "info" })?;
    let file = tracing_appender::rolling::never(".", "bot.log");
    let (file_writer, guard) = tracing_appender::non_blocking(file);
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_ansi(true))
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(file_writer),
        )
        .init();
    Ok(guard)
}
