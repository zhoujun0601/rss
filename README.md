# RSSBOT

RSSBOT 是使用 Rust 编写的 Telegram RSS/Atom/JSON Feed 订阅机器人。它通过定时轮询抓取新内容，按用户关键词筛选并推送到 Telegram，使用 SQLite 持久化订阅、去重记录和失败投递队列。

## 功能

- Telegram 内联菜单管理订阅和关键词。
- 支持 `*` 通配、`-关键词` 屏蔽、`#t/#c/#a` 匹配范围和 `关键词+RSS名称`。
- 支持 RSS、Atom、JSON Feed，以及频道内容中的图片推送和文本降级。
- 单个用户发送失败不会影响其他用户，并在进程重启后继续重试。
- Telegram/Pushinfo HTTP/HTTPS 代理、管理员访问限制、可选 `Pushinfo` 和每日推送统计。
- Feed 与重定向执行公网地址验证，直连时固定实际连接 IP，防止 SSRF/DNS 重绑定。
- 公开模式对订阅、关键词、Feed 大小和待投递队列设置容量限制；添加订阅请求同时受冷却与并发控制。
- 失败投递采用有界批处理和指数退避，长消息分段进度保存在 SQLite 中。

## Docker Compose

复制环境变量模板并填写 BotFather 提供的 Token：

```bash
cp .env.example .env
docker compose up -d --build
docker compose logs -f tgbot-rss
```

停止服务：

```bash
docker compose down
```

为保证现有部署可以原地升级，Compose 服务标识 `tgbot-rss`、容器名 `TGBot_RSS` 和持久化目录 `./TGBot_RSS` 保持不变。该目录挂载到容器 `/data/`，其中保存 `config.json`、`tgbot.db` 和 `bot.log`。入口脚本完成目录初始化后以非 root 用户运行 Bot。环境变量只在内存中覆盖 JSON 配置，不会把 Token 写回配置文件。

> Rust 版使用全新的数据库结构，不兼容 Go 版 `tgbot.db`。请勿将旧数据库直接放入运行目录；程序检测到旧 schema 时会退出且不会修改数据。

## 配置

| 配置项 | 说明 | 默认值 |
| --- | --- | --- |
| `BotToken` | Telegram Bot Token，必填 | 无 |
| `ADMINIDS` | 单个管理员用户 ID；`0` 允许所有用户 | `0` |
| `Cycletime` | RSS 检查周期，单位秒，必须大于零 | `300` |
| `Debug` | 是否输出调试日志 | `false` |
| `ProxyURL` | Telegram API 和 Pushinfo 使用的可选 HTTP/HTTPS 代理；Feed 始终直连已验证 IP | 空 |
| `Pushinfo` | 管理员成功推送后的可选 HTTP 地址前缀 | 空 |
| `TZ` | 时间显示及每日统计使用的 IANA 时区 | `Asia/Shanghai` |

程序先读取工作目录的 `config.json`，再使用存在的同名环境变量覆盖。可通过 `RSSBOT_CONFIG` 指定其他 JSON 路径；旧变量 `TGBOT_CONFIG` 仍可兼容使用。

## 容量与重试

- 每个用户最多保存 100 个订阅和 200 个关键词；订阅总数最多 5000 个。
- 每个 Feed 最多处理 1000 个条目，标题、描述和链接会在持久化前限制长度。
- 待投递队列全局最多 100000 条、每个用户最多 1000 条；达到上限时跳过新任务并记录告警。
- 每轮最多公平选取 100 个待投递任务，每个用户最多 10 个；失败后从 30 秒开始指数退避，最长 1 小时。
- 每个订阅最多保留 50000 条已见记录，避免 SQLite 数据无限增长。

## 使用

- `/start`：打开主菜单。
- `/help`：显示关键词语法和项目信息。

添加订阅时输入：

```text
https://example.com/feed 科技新闻 0
https://example.com/channel/feed TG资讯播报 1
```

添加成功时，Feed 中已有条目会作为基线记录，不会补发历史内容。相同发布时间或缺失发布时间的条目使用稳定标识去重。

## 从源码运行

项目固定 Rust `1.98.1` 与 Rust 2024 Edition：

```bash
cd RustBot
cp config.json /tmp/rssbot-config.json
RSSBOT_CONFIG=/tmp/rssbot-config.json cargo run --release
```

检查配置或查看版本时不会连接 Telegram：

```bash
cargo run -- --check-config
cargo run -- --version
```

开发验证：

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
cargo build --release --locked
```

## 目录结构

```text
RustBot/                 Rust 源码、迁移、测试和配置模板
Docker/                  Dockerfile 与容器入口
TGBot_RSS/               Compose 持久化运行目录（保留旧路径以兼容现有数据）
.github/workflows/       二进制和镜像发布流程
docker-compose.yml       Compose 服务定义
```

## License

[MIT](LICENSE)
