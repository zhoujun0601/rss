use std::{
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use scraper::Html;
use serde::Deserialize;
use teloxide::{
    Bot,
    dispatching::UpdateFilterExt,
    dptree,
    prelude::*,
    types::{
        CallbackQuery, ChatId, InlineKeyboardButton, InlineKeyboardMarkup, InputFile, Message,
        MessageId, ParseMode, Update,
    },
};
use tokio::sync::Mutex;
use tracing::{debug, error, warn};

use crate::{
    app::{App, InputState},
    content::{clean_html, escape_html, extract_image_url, split_utf8_bytes},
    keywords,
    models::{DeliveryPart, DeliveryPartKind, PendingDelivery},
    security::validate_public_http_url,
};

const MAX_MESSAGE_BYTES: usize = 4000;
const MAX_CAPTION_BYTES: usize = 900;
const DELETE_PAGE_SIZE: usize = 24;
const MAX_BUTTON_LABEL_CHARS: usize = 40;
const DOWNLOAD_COUNT_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const DOWNLOAD_COUNT_RETRY_DELAY: Duration = Duration::from_secs(60);
const RELEASES_URL: &str = "https://api.github.com/repos/zhoujun0601/rss/releases";

#[derive(Clone, Copy)]
struct CachedDownloadCount {
    value: u64,
    fetched_at: Instant,
}

#[derive(Default)]
struct DownloadCountCache {
    value: Option<CachedDownloadCount>,
    retry_after: Option<Instant>,
    refreshing: bool,
}

static DOWNLOAD_COUNT_CACHE: LazyLock<Mutex<DownloadCountCache>> =
    LazyLock::new(|| Mutex::new(DownloadCountCache::default()));

pub async fn run(app: Arc<App>) {
    let handler = dptree::entry()
        .branch(Update::filter_message().endpoint(handle_message))
        .branch(Update::filter_callback_query().endpoint(handle_callback));
    Dispatcher::builder(app.bot.clone(), handler)
        .dependencies(dptree::deps![app])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
}

async fn handle_message(bot: Bot, message: Message, app: Arc<App>) -> ResponseResult<()> {
    let Some(user) = message.from.as_ref() else {
        return Ok(());
    };
    let user_id = user.id.0 as i64;
    if !app.config.is_authorized(user_id) {
        bot.send_message(message.chat.id, "你没有权限使用此机器人")
            .await?;
        return Ok(());
    }
    let text = message.text().unwrap_or_default().trim();
    if text.starts_with('/') {
        app.clear_input_state(user_id).await;
        let command = text.split_whitespace().next().unwrap_or_default();
        let command = command
            .trim_start_matches('/')
            .split('@')
            .next()
            .unwrap_or_default();
        match command {
            "start" => show_main_menu(&bot, &app, user_id, user.full_name(), None).await?,
            "help" => show_help(&bot, &app, user_id, None).await?,
            _ => {
                bot.send_message(
                    message.chat.id,
                    format!("未知命令: {command}\n请使用 /start 查看菜单或 /help 获取帮助"),
                )
                .await?;
            }
        }
        return Ok(());
    }

    match app.take_input_state(user_id).await {
        Some(InputState::AddKeyword) => add_keywords(&bot, &app, user_id, text, None).await?,
        Some(InputState::AddSubscription) => {
            if let Err(error) = add_subscription(&bot, &app, user_id, text, None).await {
                app.set_input_state(user_id, InputState::AddSubscription)
                    .await;
                send_error(&bot, user_id, None, &format!("❌ {error}")).await?;
            }
        }
        None => {
            bot.send_message(message.chat.id, "请使用 /start 查看菜单或 /help 获取帮助")
                .await?;
        }
    }
    Ok(())
}

async fn handle_callback(bot: Bot, query: CallbackQuery, app: Arc<App>) -> ResponseResult<()> {
    let user_id = query.from.id.0 as i64;
    if !app.config.is_authorized(user_id) {
        bot.answer_callback_query(query.id).text("无权限").await?;
        return Ok(());
    }
    bot.answer_callback_query(query.id.clone()).await?;
    let Some(message) = query.message else {
        return Ok(());
    };
    let message_id = message.id();
    let data = query.data.unwrap_or_default();
    if data != "add_keyword" && data != "add_subscription" {
        app.clear_input_state(user_id).await;
    }
    let result = match data.as_str() {
        "back_to_menu" => {
            show_main_menu(
                &bot,
                &app,
                user_id,
                query.from.full_name(),
                Some(message_id),
            )
            .await
        }
        "add_keyword" => {
            app.set_input_state(user_id, InputState::AddKeyword).await;
            edit_or_send(
                &bot,
                user_id,
                Some(message_id),
                keyword_prompt(),
                back_keyboard(),
                false,
            )
            .await
        }
        "view_keywords" => view_keywords(&bot, &app, user_id, Some(message_id)).await,
        "delete_keyword" => delete_keyword_menu(&bot, &app, user_id, Some(message_id), 0).await,
        value if value.starts_with("delete_keyword:") => {
            let page = value
                .trim_start_matches("delete_keyword:")
                .parse::<usize>()
                .unwrap_or(0);
            delete_keyword_menu(&bot, &app, user_id, Some(message_id), page).await
        }
        "add_subscription" => {
            app.set_input_state(user_id, InputState::AddSubscription)
                .await;
            edit_or_send(
                &bot,
                user_id,
                Some(message_id),
                subscription_prompt(),
                back_keyboard(),
                false,
            )
            .await
        }
        "view_subscriptions" => view_subscriptions(&bot, &app, user_id, Some(message_id)).await,
        "delete_subscription" => {
            delete_subscription_menu(&bot, &app, user_id, Some(message_id), 0).await
        }
        value if value.starts_with("delete_subscription:") => {
            let page = value
                .trim_start_matches("delete_subscription:")
                .parse::<usize>()
                .unwrap_or(0);
            delete_subscription_menu(&bot, &app, user_id, Some(message_id), page).await
        }
        "help" => show_help(&bot, &app, user_id, Some(message_id)).await,
        value if value.starts_with("del_kw:") => match value
            .trim_start_matches("del_kw:")
            .parse::<i64>()
        {
            Ok(id) => {
                app.db
                    .remove_keyword(user_id, id)
                    .await
                    .map_err(to_request_error)?;
                delete_keyword_menu(&bot, &app, user_id, Some(message_id), 0).await
            }
            Err(_) => send_error(&bot, user_id, Some(message_id), "删除关键词失败：参数错误").await,
        },
        value if value.starts_with("del_sub:") => match value
            .trim_start_matches("del_sub:")
            .parse::<i64>()
        {
            Ok(id) => {
                app.db
                    .remove_subscription(user_id, id)
                    .await
                    .map_err(to_request_error)?;
                delete_subscription_menu(&bot, &app, user_id, Some(message_id), 0).await
            }
            Err(_) => send_error(&bot, user_id, Some(message_id), "删除订阅失败：参数错误").await,
        },
        _ => send_error(&bot, user_id, Some(message_id), "未知的操作，请重试").await,
    };
    if let Err(error) = result {
        error!(user_id, %error, callback = %data, "处理回调失败");
        return Err(error);
    }
    Ok(())
}

async fn show_main_menu(
    bot: &Bot,
    app: &App,
    user_id: i64,
    name: String,
    message_id: Option<MessageId>,
) -> ResponseResult<()> {
    let (subscriptions, keyword_count) = app.db.stats(user_id).await.map_err(to_request_error)?;
    let stats = app.stats_text().await;
    let text = format!(
        "👋 欢迎使用 RSSBOT 订阅机器人！\n\n👥 {}(<code>{}</code>)：\n📰 订阅数：{}    🔍关键词数：{}\n\n{}\n1️⃣ 订阅管理：增加/删除/查看 RSS 源\n2️⃣ 关键词管理：增加/删除/查看关键词\n\n请选择以下操作：",
        escape_html(&name),
        user_id,
        subscriptions,
        keyword_count,
        escape_html(&stats)
    );
    edit_or_send(bot, user_id, message_id, &text, main_keyboard(), true).await
}

async fn show_help(
    bot: &Bot,
    app: &App,
    user_id: i64,
    message_id: Option<MessageId>,
) -> ResponseResult<()> {
    let downloads = cached_download_count(&app.http).await;
    let text = format!(
        "🤖 RSSBOT 订阅机器人\n📡 Rust 编写的 RSS/Atom/JSON Feed 订阅推送工具\n💾 使用 SQLite 保存订阅、关键词、抓取进度和失败队列\n📰 当前项目下载：{downloads} 次\n\n📝 <b>使用帮助</b>\n\n🔤 <b>关键词基础</b>\n• 支持中英文，可用逗号分隔多个关键词\n• <code>*</code> 匹配任意字符，<code>-关键词</code> 屏蔽内容\n\n🎯 <b>匹配范围</b>\n• 默认只匹配标题\n• <code>#t关键词</code> 只匹配标题\n• <code>#c关键词</code> 只匹配描述\n• <code>#a关键词</code> 匹配标题和描述\n\n📡 <b>RSS 过滤</b>\n• <code>关键词+RSS名称</code> 只匹配指定订阅源\n• 单独使用 <code>*</code> 可接收该订阅源的全部内容\n\n📦 项目地址: https://github.com/zhoujun0601/rss\n🔧 问题反馈: https://github.com/zhoujun0601/rss/issues"
    );
    edit_or_send(bot, user_id, message_id, &text, back_keyboard(), true).await
}

async fn add_keywords(
    bot: &Bot,
    app: &App,
    user_id: i64,
    text: &str,
    message_id: Option<MessageId>,
) -> ResponseResult<()> {
    let values = keywords::normalize_input(text);
    if values.is_empty() {
        app.set_input_state(user_id, InputState::AddKeyword).await;
        return send_error(bot, user_id, message_id, "❌ 请输入有效的关键词").await;
    }
    let added = app
        .db
        .add_keywords(user_id, &values)
        .await
        .map_err(to_request_error)?;
    let all = app
        .db
        .keyword_values(user_id)
        .await
        .map_err(to_request_error)?;
    let text = if added == 0 {
        "❌ 没有新增关键词，可能全部已存在".to_owned()
    } else {
        format!(
            "✅ 成功添加 {added} 个关键词\n当前共有 {} 个关键词\n\n📋 关键词列表：\n{}",
            all.len(),
            numbered(&all)
        )
    };
    edit_or_send(bot, user_id, message_id, &text, back_keyboard(), false).await
}

async fn add_subscription(
    bot: &Bot,
    app: &App,
    user_id: i64,
    text: &str,
    message_id: Option<MessageId>,
) -> Result<()> {
    let parts: Vec<&str> = text.split_whitespace().collect();
    if parts.len() != 3 {
        bail!("格式错误，请输入：URL 名称 TG频道用1常规用0");
    }
    let channel = match parts[2] {
        "0" => false,
        "1" => true,
        _ => bail!("频道标记必须为 0 或 1"),
    };
    let _permit = app.claim_subscription_add(user_id).await?;
    app.db
        .check_subscription_capacity(parts[0], parts[1], user_id)
        .await?;
    validate_public_http_url(parts[0]).await?;
    let entries = app.feed.fetch(parts[0]).await.context("RSS 源验证失败")?;
    let result = app
        .db
        .add_subscription(parts[0], parts[1], channel, user_id, &entries)
        .await?;
    debug!(?result, user_id, rss = parts[1], "添加订阅成功");
    let response = format!("✅ 成功添加订阅：\n📰 {}\n🔗 {}", parts[1], parts[0]);
    edit_or_send(bot, user_id, message_id, &response, back_keyboard(), false)
        .await
        .map_err(anyhow::Error::from)
}

async fn view_keywords(
    bot: &Bot,
    app: &App,
    user_id: i64,
    message_id: Option<MessageId>,
) -> ResponseResult<()> {
    let values = app
        .db
        .keyword_values(user_id)
        .await
        .map_err(to_request_error)?;
    if values.is_empty() {
        return send_error(bot, user_id, message_id, "你还没有添加任何关键词").await;
    }
    let escaped: Vec<String> = values
        .iter()
        .map(|value| format!("<code>{}</code>", escape_html(value)))
        .collect();
    let text = format!(
        "📋 你的关键词列表（共 {} 个）：\n\n{}",
        values.len(),
        numbered(&escaped)
    );
    edit_or_send(bot, user_id, message_id, &text, back_keyboard(), true).await
}

async fn delete_keyword_menu(
    bot: &Bot,
    app: &App,
    user_id: i64,
    message_id: Option<MessageId>,
    page: usize,
) -> ResponseResult<()> {
    let items = app.db.keywords(user_id).await.map_err(to_request_error)?;
    if items.is_empty() {
        return send_error(bot, user_id, message_id, "你还没有添加任何关键词").await;
    }
    let buttons: Vec<_> = items
        .into_iter()
        .map(|item| (item.value, format!("del_kw:{}", item.id)))
        .collect();
    let (page, total_pages) = bounded_page(page, buttons.len());
    edit_or_send(
        bot,
        user_id,
        message_id,
        &format!(
            "请选择要删除的关键词（第 {}/{} 页）：",
            page + 1,
            total_pages
        ),
        delete_keyboard(&buttons, page, "delete_keyword"),
        false,
    )
    .await
}

async fn view_subscriptions(
    bot: &Bot,
    app: &App,
    user_id: i64,
    message_id: Option<MessageId>,
) -> ResponseResult<()> {
    let items = app
        .db
        .subscriptions_for_user(user_id)
        .await
        .map_err(to_request_error)?;
    if items.is_empty() {
        return send_error(bot, user_id, message_id, "你还没有添加任何订阅").await;
    }
    let rows: Vec<String> = items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            format!(
                "订阅{}.<code>{}</code>\n{}",
                index + 1,
                escape_html(&item.name),
                escape_html(&item.url)
            )
        })
        .collect();
    let text = format!(
        "📰 你的订阅列表（共 {} 个）：\n\n{}",
        items.len(),
        rows.join("\n")
    );
    edit_or_send(bot, user_id, message_id, &text, back_keyboard(), true).await
}

async fn delete_subscription_menu(
    bot: &Bot,
    app: &App,
    user_id: i64,
    message_id: Option<MessageId>,
    page: usize,
) -> ResponseResult<()> {
    let items = app
        .db
        .subscriptions_for_user(user_id)
        .await
        .map_err(to_request_error)?;
    if items.is_empty() {
        return send_error(bot, user_id, message_id, "你还没有添加任何订阅").await;
    }
    let buttons: Vec<_> = items
        .into_iter()
        .map(|item| (item.name, format!("del_sub:{}", item.id)))
        .collect();
    let (page, total_pages) = bounded_page(page, buttons.len());
    edit_or_send(
        bot,
        user_id,
        message_id,
        &format!("请选择要删除的订阅（第 {}/{} 页）：", page + 1, total_pages),
        delete_keyboard(&buttons, page, "delete_subscription"),
        false,
    )
    .await
}

pub async fn deliver_feed_entry(
    app: &App,
    delivery: &PendingDelivery,
    matched: &[String],
) -> Result<()> {
    let date = delivery
        .entry
        .published_at
        .with_timezone(&app.config.timezone())
        .format("%F %T")
        .to_string();
    let keyword_text = matched
        .iter()
        .map(|value| format!("<code>{}</code>", escape_html(value)))
        .collect::<Vec<_>>()
        .join(" ");
    let fallback_text = if delivery.channel {
        let description = clean_html(&delivery.entry.description);
        let header = format!(
            "👋 {}: {}\n🕒 {}\n",
            escape_html(&delivery.subscription_name),
            keyword_text,
            date
        );
        let full = format!("{header}{description}");
        full
    } else {
        format!(
            "📌 {}\n🔖 关键词: {}\n🕒 {}\n🔗 {}",
            escape_html(&delivery.entry.title),
            keyword_text,
            date,
            escape_html(&delivery.entry.link)
        )
    };

    let mut stored_parts = app.db.delivery_parts(delivery).await?;
    if stored_parts.is_empty() {
        let parts = initial_delivery_parts(delivery, &fallback_text, &date).await;
        app.db.initialize_delivery_parts(delivery, &parts).await?;
        stored_parts = app.db.delivery_parts(delivery).await?;
    }

    send_delivery_parts(app, delivery, &fallback_text, stored_parts).await
}

async fn initial_delivery_parts(
    delivery: &PendingDelivery,
    text: &str,
    date: &str,
) -> Vec<DeliveryPart> {
    if delivery.channel
        && let Some(image) = extract_image_url(&delivery.entry.description)
        && validate_public_http_url(&image).await.is_ok()
    {
        let caption = if text.len() <= MAX_CAPTION_BYTES {
            text.to_owned()
        } else {
            format!(
                "👋 {}\n🕒 {}",
                escape_html(&delivery.subscription_name),
                date
            )
        };
        let mut parts = vec![DeliveryPart {
            index: 0,
            kind: DeliveryPartKind::Photo,
            content: caption,
            media_url: image,
        }];
        if text.len() > MAX_CAPTION_BYTES {
            let mut text_parts = delivery_text_parts(text);
            for part in &mut text_parts {
                part.index += 1;
            }
            parts.extend(text_parts);
        }
        return parts;
    }

    delivery_text_parts(text)
}

fn delivery_text_parts(html: &str) -> Vec<DeliveryPart> {
    if html.len() <= MAX_MESSAGE_BYTES {
        return vec![DeliveryPart {
            index: 0,
            kind: DeliveryPartKind::TextHtml,
            content: html.to_owned(),
            media_url: String::new(),
        }];
    }
    let plain = {
        let fragment = Html::parse_fragment(html);
        fragment.root_element().text().collect::<String>()
    };
    split_utf8_bytes(&plain, MAX_MESSAGE_BYTES)
        .into_iter()
        .enumerate()
        .map(|(index, content)| DeliveryPart {
            index: index as i64,
            kind: DeliveryPartKind::TextPlain,
            content,
            media_url: String::new(),
        })
        .collect()
}

async fn send_delivery_parts(
    app: &App,
    delivery: &PendingDelivery,
    fallback_text: &str,
    mut parts: Vec<DeliveryPart>,
) -> Result<()> {
    loop {
        let Some(part) = parts.first().cloned() else {
            return Ok(());
        };
        let result = match part.kind {
            DeliveryPartKind::TextHtml => app
                .bot
                .send_message(ChatId(delivery.user_id), &part.content)
                .parse_mode(ParseMode::Html)
                .await
                .map(|_| ()),
            DeliveryPartKind::TextPlain => app
                .bot
                .send_message(ChatId(delivery.user_id), &part.content)
                .await
                .map(|_| ()),
            DeliveryPartKind::Photo => {
                let url = validate_public_http_url(&part.media_url).await;
                match url {
                    Ok(url) => app
                        .bot
                        .send_photo(ChatId(delivery.user_id), InputFile::url(url))
                        .caption(&part.content)
                        .parse_mode(ParseMode::Html)
                        .await
                        .map(|_| ()),
                    Err(error) => {
                        warn!(user_id = delivery.user_id, %error, "图片地址不再有效，降级为文本");
                        let fallback = delivery_text_parts(fallback_text);
                        app.db.replace_delivery_parts(delivery, &fallback).await?;
                        parts = fallback;
                        continue;
                    }
                }
            }
        };
        if let Err(error) = result {
            if part.kind == DeliveryPartKind::Photo {
                warn!(user_id = delivery.user_id, %error, "图片发送失败，降级为文本");
                let fallback = delivery_text_parts(fallback_text);
                app.db.replace_delivery_parts(delivery, &fallback).await?;
                parts = fallback;
                continue;
            }
            return Err(error.into());
        }
        app.db.complete_delivery_part(delivery, part.index).await?;
        parts.remove(0);
    }
}

async fn edit_or_send(
    bot: &Bot,
    user_id: i64,
    message_id: Option<MessageId>,
    text: &str,
    keyboard: InlineKeyboardMarkup,
    html: bool,
) -> ResponseResult<()> {
    let (pages, parse_as_html) = message_pages(text, html);
    for (index, page) in pages.into_iter().enumerate() {
        let markup = keyboard.clone();
        if index == 0
            && let Some(message_id) = message_id
        {
            let mut request = bot
                .edit_message_text(ChatId(user_id), message_id, page)
                .reply_markup(markup);
            if parse_as_html {
                request = request.parse_mode(ParseMode::Html);
            }
            request.await?;
        } else {
            let mut request = bot.send_message(ChatId(user_id), page).reply_markup(markup);
            if parse_as_html {
                request = request.parse_mode(ParseMode::Html);
            }
            request.await?;
        }
    }
    Ok(())
}

fn message_pages(text: &str, html: bool) -> (Vec<String>, bool) {
    if text.len() <= MAX_MESSAGE_BYTES {
        return (vec![text.to_owned()], html);
    }
    let plain = if html {
        let fragment = Html::parse_fragment(text);
        fragment.root_element().text().collect::<String>()
    } else {
        text.to_owned()
    };
    (split_utf8_bytes(&plain, MAX_MESSAGE_BYTES), false)
}

async fn send_error(
    bot: &Bot,
    user_id: i64,
    message_id: Option<MessageId>,
    text: &str,
) -> ResponseResult<()> {
    edit_or_send(bot, user_id, message_id, text, back_keyboard(), false).await
}

fn main_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback("📝 添加关键词", "add_keyword"),
            InlineKeyboardButton::callback("📋 查看关键词", "view_keywords"),
        ],
        vec![InlineKeyboardButton::callback(
            "🗑️ 删除关键词",
            "delete_keyword",
        )],
        vec![
            InlineKeyboardButton::callback("➕ 添加订阅", "add_subscription"),
            InlineKeyboardButton::callback("📰 查看订阅", "view_subscriptions"),
        ],
        vec![
            InlineKeyboardButton::callback("🗑️ 删除订阅", "delete_subscription"),
            InlineKeyboardButton::callback("ℹ️ 关于", "help"),
        ],
    ])
}

fn back_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
        "🔙 返回主菜单",
        "back_to_menu",
    )]])
}

fn delete_keyboard(
    items: &[(String, String)],
    page: usize,
    page_action: &str,
) -> InlineKeyboardMarkup {
    let mut rows = Vec::new();
    let start = page.saturating_mul(DELETE_PAGE_SIZE);
    let end = (start + DELETE_PAGE_SIZE).min(items.len());
    for chunk in items[start..end].chunks(3) {
        rows.push(
            chunk
                .iter()
                .map(|(label, data)| {
                    InlineKeyboardButton::callback(
                        format!("❌ {}", truncate_label(label)),
                        data.clone(),
                    )
                })
                .collect(),
        );
    }
    let total_pages = items.len().div_ceil(DELETE_PAGE_SIZE).max(1);
    let mut navigation = Vec::new();
    if page > 0 {
        navigation.push(InlineKeyboardButton::callback(
            "⬅️ 上一页",
            format!("{page_action}:{}", page - 1),
        ));
    }
    navigation.push(InlineKeyboardButton::callback("🔙 返回", "back_to_menu"));
    if page + 1 < total_pages {
        navigation.push(InlineKeyboardButton::callback(
            "下一页 ➡️",
            format!("{page_action}:{}", page + 1),
        ));
    }
    rows.push(navigation);
    InlineKeyboardMarkup::new(rows)
}

fn bounded_page(requested: usize, item_count: usize) -> (usize, usize) {
    let total_pages = item_count.div_ceil(DELETE_PAGE_SIZE).max(1);
    (requested.min(total_pages - 1), total_pages)
}

fn truncate_label(value: &str) -> String {
    if value.chars().count() <= MAX_BUTTON_LABEL_CHARS {
        return value.to_owned();
    }
    let mut output: String = value.chars().take(MAX_BUTTON_LABEL_CHARS - 1).collect();
    output.push('…');
    output
}

fn numbered(values: &[String]) -> String {
    values
        .iter()
        .enumerate()
        .map(|(index, value)| format!("{}.{}", index + 1, value))
        .collect::<Vec<_>>()
        .join("  ")
}

fn keyword_prompt() -> &'static str {
    "请输入要添加的关键词，多个关键词可用逗号分隔：\n\n💡 * 可匹配任意字符，-关键词 表示屏蔽\n💡 #t/#c/#a 分别匹配标题、描述或全部\n💡 关键词+RSS名称 可限制订阅源\n💡 单独使用 * 可全量推送"
}

fn subscription_prompt() -> &'static str {
    "✏️ 手动添加新订阅：\n⚠️ 频道需要先转为 RSS\n\n请输入：URL 名称 TG频道用1常规用0\n\n常规：https://example.com/feed 科技新闻 0\n频道：https://example.com/channel/feed TG资讯播报 1"
}

fn to_request_error(error: anyhow::Error) -> teloxide::RequestError {
    teloxide::RequestError::Io(Arc::new(std::io::Error::other(error.to_string())))
}

#[derive(Deserialize)]
struct Release {
    assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
    download_count: u64,
}

async fn download_count_from(client: &reqwest::Client, url: &str) -> Result<u64> {
    let releases: Vec<Release> = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "RSSBOT-Rust/1.0")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(releases
        .into_iter()
        .flat_map(|release| release.assets)
        .map(|asset| asset.download_count)
        .sum())
}

async fn cached_download_count(client: &reqwest::Client) -> u64 {
    cached_download_count_from(client, RELEASES_URL, &DOWNLOAD_COUNT_CACHE).await
}

async fn cached_download_count_from(
    client: &reqwest::Client,
    url: &str,
    cache: &Mutex<DownloadCountCache>,
) -> u64 {
    let fallback = {
        let mut state = cache.lock().await;
        if let Some(cached) = state.value
            && cached.fetched_at.elapsed() < DOWNLOAD_COUNT_TTL
        {
            return cached.value;
        }
        if state.refreshing
            || state
                .retry_after
                .is_some_and(|retry_after| retry_after > Instant::now())
        {
            return state.value.map_or(0, |cached| cached.value);
        }
        state.refreshing = true;
        state.value.map(|cached| cached.value).unwrap_or(0)
    };

    let result = download_count_from(client, url).await;
    let mut state = cache.lock().await;
    state.refreshing = false;
    match result {
        Ok(value) => {
            state.value = Some(CachedDownloadCount {
                value,
                fetched_at: Instant::now(),
            });
            state.retry_after = None;
            value
        }
        Err(error) => {
            state.retry_after = Some(Instant::now() + DOWNLOAD_COUNT_RETRY_DELAY);
            warn!(%error, "获取项目下载次数失败");
            fallback
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

    #[test]
    fn long_dynamic_text_is_split_within_telegram_limit() {
        let text = "内容<&>".repeat(1000);
        let (pages, html) = message_pages(&text, true);
        assert!(!html);
        assert!(pages.len() > 1);
        assert!(pages.iter().all(|page| page.len() <= MAX_MESSAGE_BYTES));
        assert_eq!(pages.concat(), text);
    }

    #[test]
    fn delete_keyboard_pages_and_bounds_requests() {
        let items: Vec<_> = (0..50)
            .map(|index| (format!("item-{index}"), format!("delete:{index}")))
            .collect();
        assert_eq!(bounded_page(99, items.len()), (2, 3));
        let keyboard = delete_keyboard(&items, 1, "delete_keyword");
        assert_eq!(keyboard.inline_keyboard.len(), 9);
        assert!(keyboard.inline_keyboard.last().unwrap().len() >= 2);
    }

    #[test]
    fn truncates_long_button_labels() {
        let label = truncate_label(&"关键词".repeat(30));
        assert_eq!(label.chars().count(), MAX_BUTTON_LABEL_CHARS);
        assert!(label.ends_with('…'));
    }

    #[test]
    fn delivery_text_parts_are_stable_and_bounded() {
        let text = "content".repeat(2000);
        let parts = delivery_text_parts(&text);
        assert!(parts.len() > 1);
        for (index, part) in parts.iter().enumerate() {
            assert_eq!(part.index, index as i64);
            assert_eq!(part.kind, DeliveryPartKind::TextPlain);
            assert!(part.content.len() <= MAX_MESSAGE_BYTES);
        }
        assert_eq!(
            parts
                .iter()
                .map(|part| part.content.as_str())
                .collect::<String>(),
            text
        );
    }

    #[tokio::test]
    async fn parses_download_count_response() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"assets": [{"download_count": 3}, {"download_count": 5}]},
                {"assets": [{"download_count": 7}]}
            ])))
            .mount(&server)
            .await;

        let count = download_count_from(&reqwest::Client::new(), &server.uri())
            .await
            .unwrap();
        assert_eq!(count, 15);
    }

    #[tokio::test]
    async fn caches_download_count_requests() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!([{"assets": [{"download_count": 9}]}])),
            )
            .expect(1)
            .mount(&server)
            .await;
        let cache = Mutex::new(DownloadCountCache::default());
        let client = reqwest::Client::new();

        assert_eq!(
            cached_download_count_from(&client, &server.uri(), &cache).await,
            9
        );
        assert_eq!(
            cached_download_count_from(&client, &server.uri(), &cache).await,
            9
        );
    }

    #[tokio::test]
    async fn concurrent_download_count_refresh_does_not_block_waiters() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(400))
                    .set_body_json(json!([{"assets": [{"download_count": 9}]}])),
            )
            .expect(1)
            .mount(&server)
            .await;
        let cache = Arc::new(Mutex::new(DownloadCountCache::default()));
        let client = reqwest::Client::new();
        let refresh = {
            let cache = Arc::clone(&cache);
            let client = client.clone();
            let url = server.uri();
            tokio::spawn(async move { cached_download_count_from(&client, &url, &cache).await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;

        let started = Instant::now();
        assert_eq!(
            cached_download_count_from(&client, &server.uri(), &cache).await,
            0
        );
        assert!(started.elapsed() < Duration::from_millis(100));
        assert_eq!(refresh.await.unwrap(), 9);
    }

    #[tokio::test]
    async fn failed_download_count_refresh_is_backed_off() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;
        let cache = Mutex::new(DownloadCountCache::default());
        let client = reqwest::Client::new();

        assert_eq!(
            cached_download_count_from(&client, &server.uri(), &cache).await,
            0
        );
        assert_eq!(
            cached_download_count_from(&client, &server.uri(), &cache).await,
            0
        );
    }
}
