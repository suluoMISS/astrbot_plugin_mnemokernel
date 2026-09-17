# MnemoKernel（忆核）

MnemoKernel 是一个面向 AstrBot 的长期记忆插件。它不把检索结果自动塞进每轮上下文，而是把记忆分成“线索、申请、重建、证据简报”四个阶段：模型只能申请回忆，可信内核负责决定范围、预算、证据和可见性。

当前版本：`v0.1.1`（Linux/Windows x86_64 可安装稳定版）

### 记忆浏览插件页面

支持 Plugin Pages 的 AstrBot 版本可从「插件 → 忆核详情 → 插件页面 → 记忆浏览」打开数据浏览页。
新增页面目录后需要重新加载插件。页面显示数据库在 AstrBot 主机上的路径、内核与采集状态，
支持按会话浏览和搜索稳定记忆、日记、采集记录和回忆记录，每页 20 条。
只有登录 AstrBot 管理面板的管理员可使用，聊天用户的回忆权限不会因此扩大。

本功能需要与源码一起重建的新原生内核；已有 v0.1.0 wheel 不包含浏览接口，升级时请安装完整的 v0.1.1 平台 ZIP。
仅复制 HTML/Python 文件时，页面会提示升级完整安装包，不会绕过内核直接读取 SQLite。
旧 AstrBot 不支持页面接口时，原有聊天命令继续工作。

没有数据显示时，先检查页面中的内核状态，再确认插件配置中已开启采集；采集默认关闭。
日记需要启用整理或执行 `/mnemo summarize`，稳定记忆由日记中的有效候选生成。
已清除的采集正文显示为不可读，页面不恢复历史正文，也不提供任意 SQL 或数据库下载。

## 当前已经实现的边界

- AstrBot 插件入口与配置面板；
- 用户消息与机器人最终回复的规范化采集协议；
- 回忆申请协议、Rust 限域检索与能力门控 `recollect` 工具；能力未就绪时不注册；
- 原生内核缺失时 fail-closed：记忆停用，但不影响普通聊天；
- Rust 核心领域类型、确定性 ID、状态转换和提案校验；
- SQLite Schema 4：内嵌迁移清单、校验和、在线备份与失败恢复；
- append-only 事件信封与可撤销正文载荷分离；
- 原生侧敏感字段遮蔽与元数据白名单；
- Rust 原生作用域清除：审计、WAL checkpoint、`secure_delete`、VACUUM，且事件重放不复活正文；
- Episode、Claim 和日记导出的受约束表结构；
- 手动 `/mnemo summarize [YYYY-MM-DD]`，以及到点后由首条新消息触发的上一日自动整理；
- 日记整理最多读取 96 条事件、64,000 字正文，并显式标记上下文是否被截断；
- `/mnemo today [YYYY-MM-DD]`：读取当天正式日记版本；
- `/mnemo pause`、`/mnemo resume`：按当前作用域暂停或恢复采集；群聊默认仅允许配置的控制账号执行；
- `/mnemo forget`：60 秒二次确认后清除当前作用域的正文、日记、稳定记忆和回忆审计，并保持退出采集；同一作用域不能直接 resume；
- `/mnemo stats`、`/mnemo maintain`：查看作用域统计、按保留期清理过期正文；
- 日记 claim（含 `preference`）会通过稳定指纹物化为带证据的记忆卡；完全相同的 claim 只强化原卡，不产生副本；
- Python/Rust 之间的 JSON v1 协议与 JSON Schema；
- 0.1.1 已通过 44 项 Python 测试、25 项 Rust 测试、Rustfmt、Clippy、页面交互测试、Windows wheel 原生烟测、Linux wheel 交叉构建与两平台安装包 smoke test。

当前阶段不会把日记或 RAG 文本自动注入模型；模型只能提交候选提案，不能直接修改正式记忆。

## 目录

```text
main.py                         AstrBot 薄壳
mnemokernel_adapter/            Python 协议适配和安全降级
rust/crates/mnemokernel-core/   领域规则与不变量
rust/crates/mnemokernel-store/  SQLite 持久层
rust/crates/mnemokernel-py/     PyO3 边界
migrations/                     数据库迁移
schemas/                        跨语言协议
tests/                          Python 侧测试
docs/adr/                       架构决策记录
```

项目内部计划、审计和实施切片不随公开插件包发布；公开功能边界以本文档为准。

## 许可证

本项目原创代码采用 GNU Affero General Public License v3 或更高版本（AGPL-3.0-or-later）。
完整文本见 [LICENSE](LICENSE)。第三方依赖继续遵循各自许可证。

## 安全默认值

- 原始消息采集默认关闭；
- 群聊采集必须显式加入白名单；
- 群聊的暂停、恢复、遗忘和保留期清理默认拒绝，必须配置真实发送者账号白名单；
- 原生 Rust 扩展不存在或数据库不可用时，不建立 Python 旁路存储；
- 原生扩展启动时必须报告匹配的协议与 Schema 版本；关闭异常也会立即切断 Python 侧能力；
- 回忆结果只作为工具返回值进入当前推理，不写入长期上下文；
- 回忆深度分别限制为 `glance=1/420 字`、`focused=2/620 字`、`deep=3/800 字`，只返回带事件证据 ID 的简报；
- `/mnemo forget` 的确认只保存在进程内，并同时绑定作用域和真实操作者；Rust 清除会同步删除稳定记忆、边和审计记录。

## 安装 0.1.1

当前可安装包面向 Linux x86_64 或 Windows x86_64、Python 3.10 及以上：

1. 在 AstrBot 插件管理中上传与系统匹配的 ZIP：
   - Linux：`astrbot_plugin_mnemokernel-v0.1.1-linux-x86_64.zip`；
   - Windows：`astrbot_plugin_mnemokernel-v0.1.1-windows-x86_64.zip`；
2. 压缩包内已直接携带 ABI3 原生运行时，正常情况下不需要 AstrBot 安装任何 pip 依赖；`native/` 下的 wheel 仅作为手动安装和诊断备用件；
3. 重载插件，先执行 `/mnemo doctor`；确认 Schema 为 4、三项能力均为 `true` 后，再按需打开采集、日记和回忆开关。

不要在 Windows AstrBot 中安装 Linux 包，也不要在 Linux AstrBot 中安装 Windows 包。若当前机器的架构、运行库或 glibc 不兼容，插件会 fail-closed，普通聊天仍可运行。

## 本地验证

Python 侧只依赖标准库：

```bash
python3 -m unittest discover -s tests -v
python3 -m compileall main.py mnemokernel_adapter tests scripts
```

项目使用 `rust-toolchain.toml` 固定工具链：

```bash
cd rust
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

原生扩展使用 `rust/crates/mnemokernel-py/pyproject.toml` 中的 Maturin 配置构建，
`./scripts/check-native.sh` 会把唯一当前 wheel 写入 `dist/native-current/`；
`scripts/package_release.py` 生成上传 ZIP、独立 wheel 和 `SHA256SUMS`。
当前 `episode_ready=true` 表示手动/事件驱动日记和稳定记忆卡物化已经可用；
`recall_ready=true` 表示按需回忆的 native 检索闭环可用，但仍需在配置中显式打开
`recall.enabled` 才会注册工具。

真实 AstrBot 收发钩子、模型调用判断和插件卸载仍建议在个人实例中做最终验收；0.1.1 保持保守记忆策略，不用未经证据支持的相似度启发式覆盖个人事实。
