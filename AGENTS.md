# AGENTS.md

本文件是代理在本仓库工作的常驻入口。代码和测试是当前行为的最终事实来源；修改前先阅读相关实现、调用方和测试，不要仅凭本文件推断行为。

## 沟通语言

所有对话回复必须使用简体中文。代码、命令、路径、标识符、配置项、日志原文、英文专有名词与缩写保持原样。

## 项目概览

TGBot_RSS 是一个 Go 编写的 Telegram RSS 订阅机器人，使用 SQLite 保存订阅、用户关键词和订阅抓取游标，并通过定时轮询向订阅用户发送匹配内容。

- `TGRSSBot/main.go`：程序入口、配置加载、Telegram 交互、用户状态、数据库初始化、订阅管理、HTTP 客户端和 RSS 监控调度。
- `TGRSSBot/rss.go`：Feed 拉取与解析、关键词匹配、消息投递、失败重试、HTML 清理和抓取游标更新。
- `TGRSSBot/*_test.go`：Go 单元与回归测试。
- `TGRSSBot/config.json`：可提交的配置模板，不得写入真实 Token 或其他秘密。
- `Docker/Dockerfile`：使用 Go 1.27.1 和 CGO 构建二进制，并生成运行镜像。
- `Docker/main.sh`：容器入口；每次启动都会用环境变量同步挂载目录中的 `config.json`。
- `docker-compose.yml`：构建并运行 `tgbot-rss` 服务，将宿主机 `./TGBot_RSS` 挂载到容器 `/root/`。
- `.github/workflows/`：多架构二进制发布和镜像发布流程。
- `TGBot_RSS/`：Compose 运行数据目录，可能包含二进制、`config.json`、`tgbot.db` 和日志；它不是核心源码目录。

## 开始工作前

1. 先执行 `git status --short`，识别并保留用户已有改动和未跟踪的运行数据。
2. 修改业务行为时同时阅读 `TGRSSBot/main.go`、`TGRSSBot/rss.go` 中相关调用链以及现有测试。
3. 修改部署行为时同时检查 `Docker/Dockerfile`、`Docker/main.sh`、`docker-compose.yml` 和相关 GitHub Actions。
4. 不要编辑、覆盖或删除 `TGBot_RSS/tgbot.db`、运行日志、已生成二进制或用户配置，除非任务明确要求处理这些运行数据。

## 环境与执行政策

项目基线为 Go 1.27.1。`github.com/mattn/go-sqlite3` 依赖 CGO，因此构建和测试环境必须包含 C 编译器及 libc 开发文件。宿主机不保证安装 Go 工具链，优先通过 Docker 执行 Go 命令，不要修改宿主机工具链或 shell 配置。

在仓库根目录运行完整 Go 验证：

```bash
docker run --rm \
  -v "$PWD/TGRSSBot:/src" \
  -w /src \
  golang:1.27.1-alpine \
  sh -lc 'apk add --no-cache gcc musl-dev && test -z "$(gofmt -l .)" && go test ./... && go vet ./... && go build ./...'
```

验证容器构建与 Compose 配置：

```bash
docker compose config
docker compose build
```

只有在任务明确需要启动服务且已提供可用测试 Token 时才运行 `docker compose up -d`。`.env.example` 仅是模板；占位 Token 会使 Telegram API 初始化失败。不要在输出、日志、补丁或提交中泄露 `.env`、真实 `BotToken`、代理凭据或推送 URL 中的秘密。

## 验证要求

- Go 行为改动至少运行相关测试、`gofmt` 检查、`go test ./...`、`go vet ./...` 和 `go build ./...`。
- 缺陷修复应优先添加可复现原问题的回归测试，并覆盖成功路径、关键边界和失败路径。
- Dockerfile、入口脚本、Compose、环境变量或挂载路径改动必须运行 `docker compose config` 和镜像构建。
- GitHub Actions 或跨架构构建改动要核对 CGO 编译器、目标架构、产物名称和发布路径的一致性。
- 纯文档改动无需构建应用，但应检查文档命令、路径和版本是否与仓库一致。
- 无法执行某项验证时，在最终答复中明确说明原因，不要声称已经通过。

## 必须保持的约束

- RSS Feed URL 只允许无 userinfo 的 `http` 或 `https` URL。首次验证、正常轮询和每次重定向都必须拒绝 loopback、private、link-local、unspecified、multicast 等非公网目标。
- SSRF 防护必须约束实际建立连接时使用的 IP，不能只做一次独立 DNS 预解析；保留原始主机名用于 HTTP `Host` 和 TLS 校验。
- Telegram `HTML` 模式下，所有来自 Feed、用户输入和数据库的文本及属性都必须正确转义。仅保留 Telegram 支持的标签和允许的链接协议；移除非法 `<a>` 时必须同时处理配对结束标签，不能生成残缺 HTML。
- Telegram 单条消息长度限制必须按 UTF-8 边界切分，不能切断多字节字符；错误或非正数限制应安全降级。
- Feed 抓取游标以订阅为单位，但投递失败必须按收件人和条目隔离。单个不可达用户不能阻止其他用户的游标推进，也不能导致成功消息在后续轮询中重复发送。
- 修改失败重试时要保留取消订阅和关键词变化处理，并明确考虑进程重启后内存重试状态丢失的现有语义；若改为持久化，必须提供兼容且幂等的 SQLite 迁移。
- `subscriptions`、`user_keywords` 和 `feed_data` 的建表语句位于 `initDatabase`，是当前 SQLite schema 的事实来源。数据库写入继续使用参数化 SQL，并维持事务、锁和并发访问安全。
- RSS 轮询不得重入；配置的 `Cycletime` 按当前实现的秒语义使用，非正数配置必须在启动阶段被拒绝。
- 访问控制必须继续由后端 `isAuthorized` 强制执行，不能只依赖 Telegram 菜单或按钮是否可见。
- 关闭响应体、数据库行迭代器、事务和网络连接；错误路径不得泄漏资源、静默吞错或留下部分更新状态。

## 配置与运行数据

- `.env.example` 与 `TGRSSBot/config.json` 只维护无秘密的默认模板；真实 `.env` 和运行目录配置不得提交。
- `Docker/main.sh` 会把 `BotToken`、`ADMINIDS`、`Cycletime`、`Debug`、`ProxyURL` 和 `Pushinfo` 写入挂载的 `/root/config.json`。修改任一配置项时，要同步入口脚本、Compose、模板和 Go `Config` 结构。
- Compose 使用 bind mount `./TGBot_RSS:/root/` 持久化 SQLite、日志、配置和运行二进制。不得在镜像或启动脚本调整中意外覆盖已有数据库。
- `Debug` 必须是 `true` 或 `false`；`ADMINIDS` 和 `Cycletime` 必须保持整数解析与启动失败行为清晰。

## 文档与提交检查

- 用户功能和总体架构更新根 `README`；模块专用运行或开发说明更新对应模块 `README`；Rust 部署与升级说明更新 `docker-rust/README.md`。
- 协议或兼容行为变化应在代码、测试和相关文档中保持一致。文档与实现冲突时，先以当前代码和测试确认事实，再修正文档。
- 提交前检查 `git status --short` 与 `git diff --check`，确保只包含任务范围内的改动且没有空白符错误。
- 不自动修改版本号、release tag、发布配置或生成发布产物，除非任务明确要求。
- 保持改动聚焦于任务涉及的模块，不做无关重构、依赖升级或生成文件刷新。
- 不提交构建产物、SQLite 数据库、日志、临时文件、`.env` 或包含凭据的配置。

### Git commit message

提交代码时，Git commit message 不要只写标题，必须使用以下格式：

- 第一行写简洁的 commit 标题。
- 标题后空一行。
- 下面使用 2～4 个简短 bullet 描述主要修改内容。
- 不要写得过于详细，只说明核心改动。
- 不要添加无意义的总结或测试结果，除非测试本身是重要修改。

示例：

```text
feat: add Rust server health check

- Add `/health` endpoint
- Add health check response model
- Update related integration tests
```
