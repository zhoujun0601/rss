# AGENTS.md

本仓库是 TGBot_RSS 的 Rust 实现。代码与测试是行为的最终事实来源。

## 沟通与安全

- 与用户沟通使用简体中文；代码、命令和标识符保持原样。
- 修改前检查相关实现、调用方和测试。
- 不读取、输出或提交 `.env`、真实 Bot Token、代理凭据、`Pushinfo` 密钥、数据库和日志。
- 不编辑或删除 `TGBot_RSS/` 中的运行数据，除非用户明确要求。

## 目录

- `RustBot/src/`：Rust 业务源码。
- `RustBot/migrations/`：全新 Rust schema；不兼容旧 Go 数据库。
- `Docker/`：多阶段镜像与容器入口。
- `TGBot_RSS/`：Compose 持久化运行目录。
- `.github/workflows/`：双架构二进制与镜像发布。

## 验证

在 `RustBot/` 中运行：

```bash
cargo +1.98.1 fmt --all --check
cargo +1.98.1 clippy --all-targets --all-features -- -D warnings
cargo +1.98.1 test --all-targets
cargo +1.98.1 build --release --locked
```

部署改动还要在根目录运行：

```bash
docker compose config
docker compose build
```

没有有效测试 Token 时不要启动服务连接 Telegram。

## 必须保持

- Feed URL 和每次重定向只允许无 userinfo 的公网 HTTP/HTTPS 地址；直连必须绑定已验证 IP。
- Telegram HTML 必须清理标签、协议和属性，外部文本必须转义。
- 已见条目与待投递队列持久化；失败用户不能阻止其他用户成功投递。
- RSS 轮询不得重入，`Cycletime` 使用秒且必须大于零。
- 命令、回调和状态输入均由后端鉴权。
- SQLite 写入使用参数化语句及事务，保持外键和级联清理。

## 文档与提交检查

- 用户功能和总体架构更新根 `README.md`；模块专用运行或开发说明更新对应模块 README；Rust 部署与升级说明更新根 `README.md` 中的 Docker Compose 和源码运行相关章节。
- 协议或兼容行为变化应在代码、测试和相关文档中保持一致。文档与实现冲突时，先以当前代码和测试确认事实，再修正文档。
- 提交前检查 `git status --short` 与 `git diff --check`，确保只包含任务范围内的改动且没有空白符错误。
- 不自动修改版本号、release tag、发布配置或生成发布产物，除非任务明确要求。

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
