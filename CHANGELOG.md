# Changelog

## v0.1.1 — 2026-09-17

### 管理页面

- 新增 AstrBot 插件页面「记忆浏览」，可查看内核状态、数据库位置、采集开关和会话列表。
- 支持按会话浏览、搜索和分页查看采集记录、每日日记、稳定记忆及回忆记录。
- 页面只允许已登录的 AstrBot 管理面板用户访问，不提供任意 SQL、数据库下载或模型上下文注入。

### 内核与安全

- 新增 Rust 固定查询投影和绑定参数，浏览接口不接受 SQL、表名或文件路径输入。
- 作用域隔离、单页 50 条上限、搜索长度限制和 `no-store` 缓存策略已纳入页面接口。
- 清除后的正文继续保持不可读；页面不会从事件信封恢复已删除内容。
- 发布 ZIP 优先加载自身携带的原生运行时，避免 AstrBot 共享目录中的旧 wheel 覆盖新内核。

### 发布与验证

- 版本统一升级到 `v0.1.1`，重新生成 Linux/Windows x86_64 ABI3 wheel 和平台 ZIP。
- Python 44 项、Rust 25 项测试通过；Rustfmt、Clippy、页面交互测试和两平台安装包 smoke test 通过。
- Windows wheel 在本机完成原生加载、数据库浏览和插件安装包验证。

### 升级说明

- 必须安装完整的 `v0.1.1` 平台 ZIP；只替换 HTML 或 Python 文件不会获得新的原生浏览接口。
- v0.1.0 的 Schema 4 数据无需删除或手工迁移；v0.1.1 会沿用并校验现有数据库。
- 安装或热重载时若发现旧的 `_mnemokernel`，优先切换到包内 runtime；若 `native_runtime` 缺失，则从包内匹配平台 wheel 自动修复。
- 从 GitHub 源码安装时由 `requirements.txt` 自动选择 Linux/Windows ABI3 wheel。
- GitHub 源码内置两个平台的 v0.1.1 wheel；即使插件页跳过依赖安装，也能在启动时离线修复 `native_runtime`。
- 平台 ZIP 不再携带 `requirements.txt`，避免 AstrBot 上传安装时重复联网安装已经内置的原生运行时。
- 旧版 AstrBot 不支持 Plugin Pages 时，原有聊天命令仍可继续使用。

### 当前限制

- 当前发布包仍面向 Linux/Windows x86_64。
- 真实 AstrBot WebUI 中的最终安装验收、消息钩子语义和模型判断准确率仍建议在个人实例中确认。
- 语义相近但措辞不同的个人事实仍不会自动覆盖旧记忆。

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
