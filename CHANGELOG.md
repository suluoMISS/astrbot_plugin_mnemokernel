# Changelog

## Unreleased

- 新增 AstrBot 插件页面「记忆浏览」：运行状态、数据库位置、会话选择、搜索与分页。
- 新增 Rust 只读浏览接口，提供采集记录、当前日记、稳定记忆及回忆记录。
- 页面 API 要求 AstrBot 登录身份，设置 no-store，并在内核过旧时明确提示升级。
- 发布脚本纳入页面静态资源和页面中文标题；需重建原生 wheel 后才能使用新增浏览功能。

## v0.1.0 — 2026-09-14

### 发布

- 发布 Linux x86_64 与 Windows x86_64 的 Python 3.10+ ABI3 wheel 和 AstrBot 上传 ZIP。
- 上传 ZIP 内置与目标平台匹配的 .so 或 .pyd，并附带 SHA256SUMS。
- 正式版本号已同步到 AstrBot 元数据、Python 包和 Rust 原生包。

### 稳定性

- 保留原生协议与 Schema 健康检查；不匹配时继续 fail-closed。
- Windows wheel、Linux wheel 交叉构建、两平台发布 ZIP 内容验收以及包内原生日记/回忆 smoke 均纳入发布验收；Python 37 项与 Rust 24 项测试通过。
- 继续采用保守记忆策略：相似但未被明确证据确认的个人事实不自动覆盖旧记忆。

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
- 增加原生协议/Schema 健康契约校验；初始化或关闭异常时保持 fail-closed。

### 安全与隐私

- 原始消息采集默认关闭；群聊采集需要显式白名单。
- 回忆结果只作为当前工具返回值，不自动注入长期上下文。
- `/mnemo forget` 会清除当前作用域的正文、日记、稳定记忆和回忆审计。
- 发布包不包含项目计划书、数据库、日志、缓存、`target` 或 `.tools`。

### 验证

- 37 项 Python 单元测试通过。
- Rust 测试、格式检查、Clippy、wheel 烟测通过。
- Linux x86_64 与 Windows x86_64、Python 3.10+ 安装包烟测通过。

### 已知限制

- 语义相近但措辞不同的 claim 在 alpha.4 不自动合并，以避免错误覆盖个人事实。
