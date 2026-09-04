# TGBot_RSS

TGBot_RSS 是一个使用 Go 编写的 Telegram RSS/Atom 订阅机器人。用户可以在 Telegram 中添加 RSS 源和关键词，机器人按设定周期抓取新内容，并把匹配的条目推送给订阅者。数据使用 SQLite 持久化。

## 功能

- 通过 Telegram 菜单添加、查看和取消 RSS 订阅。
- 通过关键词筛选推送内容，支持中英文、逗号分隔、通配符和屏蔽词。
- 使用 `#t`、`#c`、`#a` 控制匹配标题、描述或全部内容。
- 使用 `关键词+RSS名称` 将关键词限制到指定订阅源。
- 支持普通 RSS 推送和带图片提取的频道推送。
- 失败的单条投递会在后续轮询中重试，不影响其他用户和订阅源。
- 通过 HTTP/HTTPS 代理访问 Telegram 和 RSS 源，并对 RSS 地址执行公网目标校验。

## 快速开始（Docker Compose）

需要安装 Docker 和 Docker Compose。先准备配置文件：

```bash
cp .env.example .env
```

编辑 `.env`，至少填写 Telegram BotFather 提供的 `BotToken`，然后启动：

```bash
docker compose up -d --build
docker compose logs -f tgbot-rss
```

容器将宿主机的 `./TGBot_RSS` 挂载到 `/root/`。SQLite 数据库 `tgbot.db`、日志 `bot.log`、运行配置和二进制文件都会保存在该目录中。容器每次启动时会根据环境变量更新挂载目录中的 `config.json`，请勿在运行目录中提交真实 Token 或其他秘密。

停止服务：

```bash
docker compose down
```

## 配置

配置项可写在 `.env`（Compose）或 `TGRSSBot/config.json`（直接运行）中：

| 配置项 | 说明 | 示例 |
| --- | --- | --- |
| `BotToken` | Telegram Bot API Token，必填 | `123456:replace-me` |
| `ADMINIDS` | 管理员 Telegram 用户 ID；设为 `0` 表示所有用户可用，非 `0` 时仅该用户可操作 | `0` |
| `Cycletime` | RSS 检查周期，单位为秒；必须是正整数，非正数配置会在启动时拒绝 | `300` |
| `Debug` | 是否输出调试日志，只能是 `true` 或 `false` | `false` |
| `ProxyURL` | 可选的 HTTP/HTTPS 代理地址 | `http://127.0.0.1:7890` |
| `Pushinfo` | 可选的 HTTP 推送地址；管理员收到推送后会把消息附加到该地址 | `https://example.com/push?text=` |
| `TZ` | 容器时区 | `Asia/Shanghai` |

`ADMINIDS` 当前是单个整数 ID，不是逗号分隔的 ID 列表。代理凭据、Bot Token 和推送地址中的密钥不应写入 Git、日志或公开配置模板。

## 使用方法

启动机器人后，在 Telegram 中发送：

- `/start`：打开主菜单，管理订阅和关键词。
- `/help`：查看帮助信息。

添加订阅时输入以下格式，字段之间使用空格分隔：

```text
URL 名称 频道标记
```

例如：

```text
https://example.com/feed 科技新闻 0
https://example.com/channel/feed TG资讯播报 1
```

频道标记为 `0` 或 `1`。添加时会验证 URL、RSS/Atom 响应和订阅名称；URL 必须是无用户凭据的公网 `http` 或 `https` 地址。订阅名称作为唯一标识，建议不要包含空格。

关键词示例：

```text
技术,开源                 # 匹配标题中的任意关键词
#t技术                    # 只匹配标题
#c安全                    # 只匹配描述
#a人工智能                # 匹配标题和描述
你*帅*                   # * 匹配任意字符
-广告                    # 命中后屏蔽该条目
技术+科技新闻             # 只匹配名为“科技新闻”的 RSS
*                        # 对该订阅源全部推送
```

默认关键词只匹配标题；关键词匹配不区分大小写。屏蔽词优先级高于普通关键词，未指定范围时会同时检查标题和描述。

## 从源码运行

项目使用 Go 1.27.1，并依赖 `go-sqlite3`，因此需要 C 编译器和 libc 开发文件。宿主机未安装 Go 时，可以使用 Docker：

```bash
docker run --rm \
  -v "$PWD/TGRSSBot:/src" \
  -w /src \
  golang:1.27.1-alpine \
  sh -lc 'apk add --no-cache gcc musl-dev && go build -o /tmp/TGBot_RSS .'
```

直接运行时，从 `TGRSSBot` 目录执行，并确保该目录包含已填写的 `config.json`：

```bash
cd TGRSSBot
go test ./...
go run .
```

程序会在当前工作目录创建或打开 `tgbot.db` 和 `bot.log`。

## 项目结构

```text
TGRSSBot/             Go 源码、测试和配置模板
  main.go             启动、Telegram 交互、数据库和调度
  rss.go              RSS 拉取、解析、匹配和投递
Docker/Dockerfile     多阶段 Docker 构建
Docker/main.sh        容器启动和环境变量同步
docker-compose.yml    Compose 服务定义
TGBot_RSS/            Compose 的持久化运行目录
```

## 安全与数据说明

- RSS URL 及其重定向会拒绝 loopback、private、link-local、multicast 等非公网目标。
- Feed 内容会清理 Telegram 不支持或不安全的 HTML，并转义用户输入和外部文本。
- 数据库包含订阅、用户关键词和 Feed 抓取游标。删除运行目录中的 `tgbot.db` 会丢失这些数据，请先备份。
- 不要把 `TGBot_RSS/config.json`、`.env`、`tgbot.db`、日志或构建产物提交到版本库。

## 许可证

本项目以 [MIT License](LICENSE) 发布。
