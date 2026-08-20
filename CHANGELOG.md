# Changelog

## v0.1.0-alpha.4 — 2026-08-20

### 新增

- 建立 Python 适配层与 Rust 原生内核的混合架构，原生边界故障时保持普通对话可用。
- 增加事件采集、Episode、Claim、日记整理和稳定记忆卡物化流程。
- 增加按需 `recollect` 工具：只有模型主动申请且能力门控通过时才执行回忆。
- 增加 `glance`、`focused`、`deep` 三档回忆预算，并返回带证据 ID 的简报。
- 增加 `/mnemo summarize`、`/mnemo today`、`/mnemo stats`、`/mnemo maintain`。
- 增加 `/mnemo pause`、`/mnemo resume`、`/mnemo forget` 及作用域权限校验。
- 增加 SQLite Schema 4、迁移校验、在线备份、WAL checkpoint、`secure_delete` 和 VACUUM。
- 增加 Rust/Python JSON v1 协议与 JSON Schema 校验。

### 安全与隐私

- 原始消息采集默认关闭；群聊采集需要显式白名单。
- 回忆结果只作为当前工具返回值，不自动注入长期上下文。
- `/mnemo forget` 会清除当前作用域的正文、日记、稳定记忆和回忆审计。
- 发布包不包含项目计划书、数据库、日志、缓存、`target` 或 `.tools`。

### 验证

- 32 项 Python 单元测试通过。
- Rust 测试、格式检查、Clippy、wheel 烟测通过。
- Linux x86_64、Python 3.10+ 安装包烟测通过。

### 已知限制

- 当前安装包仅面向 Linux x86_64；Windows 原生 wheel 尚未提供。
- 语义相近但措辞不同的 claim 在 alpha.4 不自动合并，以避免错误覆盖个人事实。
