# L10 域服务初始化交接

实施阶段的交接记录保留如下；当前完成状态和验证结果以末尾 **Root 最终验收** 为准。

日期：2026-10-10。基线：`cb19992`。分支：`codex/architecture-refactor`。

本子任务实施与 Root 验收 **8 / 8**；第 8 项由 Root 在全局装配完成后统一运行。此计数不代表 L10 已验收，也不代表整个项目完成率。

## Checklist

- [x] 审计六个服务实际读取的 `ModuleContext` 字段与初始化副作用。
- [x] Conversation、Cache 改为明确绑定数据库／取消令牌，无旧 Context 入口。
- [x] Persona、Skills 改为绑定数据库／ArtifactRegistry／已解析路径，无完整 ServerConfig。
- [x] Memory 使用私有 `MemoryInitialization` 保存本服务启动输入，持久政策写入仍在受清理保护的 `init()` 中。
- [x] Insights 使用专用 `InsightDependencies`，保留禁用和未编译语义。
- [x] 迁移 Conversation 和 Insights 内部 fixture；添加 Memory 初始化失败不发布运行时的回归。
- [x] 目标文件 rustfmt 通过；六个服务业务／运行时尾部与基线逐字对照通过；记录 API 与 Root 待办。
- [x] Root 完成调用者迁移、默认／无默认配置、真实 PostgreSQL 与 Linux 验证，回填最终源码与证据。

## 生产装配接口

| 服务 | 构造输入 | `init()` 行为与失败约束 |
| --- | --- | --- |
| `ConversationService::with_database(db)` | 唯一数据库 | 检查已绑定并记录日志；SQL 会话／消息／来源事务不变。 |
| `CacheService::with_runtime(db, cancel_token)` | 数据库、父取消令牌 | 执行原 `cleanup_expired(256)`；错误继续传播。已有 `with_database` 用于不启动后台清理的数据库 fixture。 |
| `PersonaService::with_database(db)` | 数据库 | 加载启用模板；查询／模板失败继续按原逻辑告警、使用默认模板。未绑定数据库的 `new()` 仍供纯渲染 fixture。 |
| `SkillRegistry::with_runtime(db, artifacts, skills_dir)` | 数据库、同一 `Arc<ArtifactRegistry>`、`Option<PathBuf>` | 原顺序：seed defaults → markdown 同步 → preferred tools → load；严格失败与告警降级边界不变。Bootstrap 先解析路径。 |
| `MemoryService::with_runtime(db, embedding, config, policy, cancel_token)` | 数据库、可选 Embedding、`MemorySection`、完整 `ContextPolicy`、父令牌 | 输入在私有启动结构中等待；`init()` 注册实际 profile → `initialize_or_load` → 绑定运行状态 → health probe。没有异步构造／提前 SQL。 |
| `InsightService::with_runtime(InsightDependencies)` | 下述本服务所需输入 | 保留 feature 检查；禁用时直接丢弃依赖且 `init()` 为 no-op。启用时 `init()` 绑定原状态，worker 仍在 `start()` 所有。 |

`InsightDependencies` 从 `modules::platform::insights` 导出，字段为 `config: InsightsSection`、`db`、`event_bus`、`cancel_token`、可选 `llm`、`memory`、窄 `CacheStore`、`embedding`、`scheduler`、`conditions`。它仅是本服务一次启动所需的输入，不再传播到业务消费者，也不提供任意查找能力。Bootstrap 负责按原 major → default → minor 顺序选择 LLM。

Memory 的 `ContextPolicy` 继续由 Bootstrap 使用 `ContextPolicy::from_server_config` 构建，保留 Cache、Appraisal、Conversation 历史政策，不能用 `MemorySection` 单独替代。已有 `attach_runtime_for_test` 保留其明确的测试政策与无后台依赖用途。

所有启动构造只保存依赖；Memory、Insights 的一次启动参数在 `init()` 中消费。Root 保持清理槽先登记，再调用无参 `ModuleLifecycle::init`；错误、取消与 panic 仍由原 cleanup 所有者收口，不将失败服务发布到注册表。

## 静态证据与待验证项

- `run/layering-20261010-static/domain.json`：六个服务无 `ModuleContext`；Conversation Store、Cache start 与后续 SQL、Persona 公共渲染、Skills 公共匹配、Memory start／settings 与后续行为、Insights start／settings 与后续行为，均与 `cb19992` 对应片段逐字相同。
- 已运行目标九个 Rust 文件的 `rustfmt --check --edition 2024 --config skip_children=true`，通过。未运行 Cargo、PostgreSQL、容器或服务。
- Conversation 的实际 PostgreSQL 来源／epoch fixture 改为 `with_database`。
- Insights 的第二个 cron 注册失败回滚测试经正式 `with_runtime` → `init` 路径装配，不再手工改写私有字段。
- 原 disabled 行为测试经正式构造／初始化确认不持有数据库、配置或待启动依赖；无 Insights feature 仍保留启用报错测试。
- 新 Memory 测试用 `DatabaseConnection::Disconnected` 验证构造没有发布数据库，初始化政策失败后依然未发布，且可以停止。

没有新增依赖、平台、模型调用或硬件动作；未访问用户 5433 数据库。默认 Shadow、业务政策、事务 SQL 和后台停止算法没有改动。Root 尚需对实际装配和编译执行验收；本交接不能代替运行证据。

## 追加分工：Bootstrap factory 集成

Root 于本轮追加独占 `packages/alia-server/src/bootstrap/factory.rs`，现已完成静态迁移并释放文件。

- [x] LLM／Embedding 按现有 `LlmProviderProfileRef` 枚举取得真实 provider section，再调用各实现 `new(profile_id, profile, provider)`；保留 profile kind、缺 feature 和不支持 embedding provider 的原错误。
- [x] `create_stt(config, name)` 只投影 Whisper section；`create_tts(config, name)` 只投影 GPT-SoVITS section、默认 voice、voice profiles、expression styles。
- [x] 删除三个固定实现工厂 `create_rule_engine`、`create_scheduler`、`create_tools_with_sensors`；Root 的 Builder 直接装配并负责 executor 注册。
- [x] `create_channel` 显式接受 db、取消令牌、ConversationStore、SensorRegistry 与仅在 factory 使用的 ServerConfig。CLI 绑定 db／cancel／conversation；Embedded 绑定 db／sensors／ServerSection／keypair path。
- [x] 内部 feature 错误测试采用 `include_str!` 解析 example TOML；无 Embedded feature 的构造错误测试使用 Disconnected 参数，错误发生于任何初始化前，不作为真实运行时数据库。
- [x] factory 的 rustfmt 与 `git diff --check` 通过。未运行 Cargo。

尚需 Root 编译确认所有外部调用者采用新函数签名、固定 Tools executor 注册仍存在、无默认 feature 构建无遗漏。factory 不再调用 `for_profile` 或被删除的固定工厂；全量配置不传入任何组件。

## 独立审查修正：Insights feature 提前准入

迁移后的 `with_runtime` 需要已初始化依赖，因此生产构造位于原初始 feature 检查之后。独立审查指出：仅在该构造中检查 feature 会使错误晚于 SQL、TTS 初始化或插件副作用。

已将原 feature 条件与错误提取为 `InsightService::check_available(enabled)`（`pub(crate)`），`new` 复用同一个检查。Root 在 Builder 原 early construction 位置调用它，并负责无 feature 启用的提前失败回归。`with_runtime` 的检查继续保留，防止其它调用者漏过准入。未新增依赖绑定形式，未运行 Cargo。

静态对照范围仍有效：此修正发生在构造区，未修改既有 start、stop、settings 或业务执行尾部；domain.json 所列六项业务片段不变。

## Root 最终验收（覆盖前述待验证状态）

源码 `4519e757542f644e9eb385d4055551546bf7abd3`；最终 V2 `d361dad3057717c4efb08b62ef8d98084754b276e420d0c2f842923a0e0f3e41`，**637/637** 输入匹配 Git blob。macOS workspace **1058/0/5**、真实 PTY **3/0**、无默认 Insights availability **2/0**、新增提前拒绝回归 **1/0**；Linux ARM64 默认 Server 全包 **776/0/5**。两端 Server 无默认 all-targets、Linux release/native link、fmt 和边界正负自检均通过。所有最终运行均无源码漂移。

本轮 **10/10**，L09—L12 **2/4**，R01—R06 仍 **2/6**。专用 PostgreSQL 和两个 Linux holder 已停止，数据库零残留测试 schema、零其他连接；无待接回的 Cargo/PG 作业。V1 无 feature 回归的报告包装断言失败已原样保留；V2 仅改这条测试断言，636 个输入及全部生产源码与 V1 相同。详见 [最终验证](../../layering-2026-10-10.md#最终验证与证据) 与 [证据索引](evidence-index.json)。

下一轮从 L11 跨域事务所有者开始，L12 和 Skills reload TODO 保留。真实模型、Pi/声学/机械、Linux 全工作区与部署 smoke 未在本轮验证；用户 5433 未访问，默认 Shadow 和政策保持。未推送或创建 PR，未使用 reset 卡。
