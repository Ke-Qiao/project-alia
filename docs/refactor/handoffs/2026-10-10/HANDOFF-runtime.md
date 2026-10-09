# L10 Runtime / 扩展依赖移交（2026-10-10）

实施阶段的交接记录保留如下；当前完成状态和验证结果以末尾 **Root 最终验收** 为准。

负责人：`/root/l10_runtime_dependencies`。基线：`cb19992`。实现与静态审查 **8/8**；统一构建、真实 PostgreSQL、平台验证及提交由 Root 执行，本文件不声称这些已通过。

## 交付 checklist

- [x] 审计调用者、实际初始化依赖、失败路径、内部 fixture。
- [x] Scheduler 改为构造时接收自己的 DB / EventBus / cancellation / SchedulerSection / ConditionRegistry / ActionRegistry；无参数 `init()` 仅恢复数据库状态。
- [x] Rules 改为构造时接收自己的 DB / EventBus / cancellation / RulesSection / ConditionRegistry / ActionRegistry；无参数 `init()` 仅恢复数据库状态。
- [x] Tools 显式绑定 DB / EventBus / ArtifactRegistry / 可选 Memory 和 Scheduler，保持 SQL 清理与 catalog 同步在 `init()` 内。
- [x] CLI 显式绑定 account DB / cancellation / ConversationStore；Embedded 仅接收自身 runtime dependencies，初始化副作用及顺序保留。
- [x] Home Assistant 显式绑定本插件配置、PluginStorageService 和 ToolRegistry；保持 init 内验证配置、创建 manifest scoped storage、持久化脱敏配置、注册执行器。
- [x] 删除 owned 范围的 ModuleContext 引用；只读依赖和固定配置去除多余 Mutex；保留实际内存模式与 transport fixtures。
- [x] Rustfmt 与逐项行为审查完成，接口移交 Root。

## 最终接口

```rust
AutomationScheduler::with_runtime(
    db: DatabaseConnection,
    event_bus: EventBus,
    cancel_token: CancellationToken,
    config: SchedulerSection,
    conditions: Arc<ConditionRegistry>,
    actions: Arc<ActionRegistry>,
) -> Self

EventRuleEngine::with_runtime(
    db: DatabaseConnection,
    event_bus: EventBus,
    cancel_token: CancellationToken,
    config: RulesSection,
    conditions: Arc<ConditionRegistry>,
    actions: Arc<ActionRegistry>,
) -> Self

BuiltinToolRegistry::with_runtime(
    db: DatabaseConnection,
    event_bus: EventBus,
    artifacts: Arc<ArtifactRegistry>,
    memory: Option<Arc<MemoryService>>,
    scheduler: Option<Arc<dyn SchedulerBackend>>,
) -> Self
// Sensor tools 的已有装配改为实例链：registry.with_sensors(sensors)。

CliChannel::with_runtime(
    self,
    db: DatabaseConnection,
    cancel_token: CancellationToken,
    conversation: Arc<dyn ConversationStore>,
) -> Self

EmbeddedChannel::with_runtime(self, runtime: EmbeddedRuntimeDependencies) -> Self
// embedded::{EmbeddedRuntimeDependencies} 导出；字段：
// db, sensors: Arc<SensorRegistry>, server: ServerSection,
// server_keypair_path: PathBuf。

HomeAssistantPlugin::new(
    config: HomeAssistantPluginSection,
    storage: Arc<PluginStorageService>,
    tools: Arc<dyn ToolRegistry>,
) -> Self
```

Rules 的无真实独立调用空 `new` / `Default` 已移除，保留 `with_registries` 的真实内存规则评估。Scheduler 保留现有内存 `with_registries` / `Default`，供纯调度契约及 Insights 部分注册回滚测试使用。BuiltinToolRegistry 保留真实手工执行器目录 `new` / `Default`。CLI / Embedded 的 `new` 保留既有独立终端或 transport 测试用途；生产调用者必须接 `with_runtime`。

## 生命周期与行为证据

- Scheduler / Rules 的依赖在构造时绑定后，不再以空 Option 作为已初始化状态；`initialized` 仅在全部 SQL 恢复成功后置位，`start` 拒绝尚未完成恢复的对象。
- Scheduler 的 tick interval / task timeout / concurrency 下界仍为 1，用户任务和订阅上限语义不变。任务注册、source epoch 捕获、执行限时、OwnedBackground 启停、取消及 durable delete 错误传播保持。
- Rules 的 max rules、模式匹配、条件注册、来源捕获与执行路径保持。Condition/Action 构造必需，不再有未初始化的可选注册表与无意义锁。
- Tools 执行器 revision tombstone、执行授权、实际 dispatch 事实、Artifact 依赖事务与 catalog 更新均保持；只删除固定 EventBus 上的 Mutex。
- Embedded `init` 顺序仍为：account DB → BodyDeliveryRepository recover → body store → sensor registry → keypair → server identity/endpoint → sensor read provider 注册。失败依旧由外层 cleanup 持有；没有前移 SQL、密钥读取或设备动作。
- Home Assistant 从 init 参数获得的数据改为显式构造绑定；环境覆盖、URL 认证信息拒绝、token 脱敏快照、storage manifest 验证、执行器版本与确认要求不变。
- 没有新增依赖、feature、数据库迁移或 HTTP/硬件操作；没有 Disconnected DB 代替生产资源。

## 调用面与验证移交

Root 负责 `bootstrap/{factory,builder}.rs`、`api/channels.rs`、`modules/platform/lifecycle.rs`、所有外部测试。Dynamic channel 创建仅存在 Builder 和 API 两条生产路径，两者均需显式 runtime 装配，并保留取消、panic 捕获与 rollback 行为。

`ServerBuilder::with_extension` 现有生产实现为注入预构造 ModuleLifecycle；真实调用集中在 `tests/server_runtime.rs`。其 Probe 使用初始化所得 DB 证明 rollback 后连接释放，此断言应通过窄显式依赖继续保留，不能删除来迁移签名。

优先回归：server_runtime 失败/取消/连接释放与动态 channels、server_background scheduler/rules/Embedded、rules 纯匹配、Insights 两项 cron 注册回滚、tools/execution_service、CLI 真实 PTY、duplex_playback 与 Embedded 身份/body lease 测试、Home Assistant 原有输入校验测试。

未执行 Cargo、PG、容器、真实设备或提交；Root 统一窗口负责验证。未触及用户 5433 数据库。


## 第二轮只读审查与回归修复

- 审查 `lifecycle.rs`：Initializing rollback slot 在轮询 init 前登记；取消和 panic 路径仍保留实例；资源 phase 与 channel ownership transfer 保持。
- 审查 Builder / factory：Memory 在持久政策投影后绑定；Insights major LLM 选择等价旧生产路径；Scheduler / Rules / Tools 直接构造保留原副作用与关闭顺序。
- 发现并交 Root 修复：Insights enabled 而 binary 无 insights feature 的校验因后移构造而延至 TTS / Tools / extensions 初始化之后。Root 将 `check_available` 提前至原创建阶段。
- 在 `tests/server_runtime.rs` 新增无 insights feature 专用 `unavailable_insights_fail_before_tts_initialization`：复用生命周期 Probe 作为 TTS，检查原错误文案、初始化信号为 0、后台工作数为 0。Probe 的两个 TTS 方法禁止真正合成。
- Probe 的 owned pool 证据迁移经审查接受：成功构建后显式绑定 runtime DB 保留 stop 内 SELECT 1；失败 startup 使用独有 application_name 和参数化 pg_stat_activity 查询观察连接存续及最终释放，不增加生产 hook。
- 将连接清零查询的 yield 循环调整为 10 ms 间隔，保留 2 秒总期限；没有访问数据库。

上述追加回归尚未由本 agent 执行，仍由 Root 在无默认 feature 验证窗口运行；本 agent 只完成格式与 diff 静态检查。

## V1 / V2 验证试验记录

Root 已实际运行 V1 的无 insights feature admission 回归。V1 失败原因是新增测试错误假设 `root_cause()` 只返回 feature 文案；Runtime 会汇总 startup / shutdown 错误，实际报告包含 `server shutdown failed: startup: assembling server runtime: enabled insights require feature 'insights'`。这次失败不是生产能力检查顺序回归，不能将其计为该保护失效。

Root 仅调整 `server_runtime.rs` 中该断言，改为检查汇总报告包含同一 feature 拒绝原因；TTS `initialized == 0` 与 `active == 0` 两项关键断言继续保留，生产源码未改变。

V2 冻结包含 **637** 个输入，相比 V1 仅 `server_runtime.rs` 改变。Root 提供的 V2 manifest 为 `d361dad3057717c4efb08b62ef8d98084754b276e420d0c2f842923a0e0f3e41`。V2 的统一验证结果待 Root 汇总；本 agent 本次只更新此试验记录，没有修改源码，也没有执行验证。

## Root 最终验收（覆盖前述待验证状态）

源码 `4519e757542f644e9eb385d4055551546bf7abd3`；最终 V2 `d361dad3057717c4efb08b62ef8d98084754b276e420d0c2f842923a0e0f3e41`，**637/637** 输入匹配 Git blob。macOS workspace **1058/0/5**、真实 PTY **3/0**、无默认 Insights availability **2/0**、新增提前拒绝回归 **1/0**；Linux ARM64 默认 Server 全包 **776/0/5**。两端 Server 无默认 all-targets、Linux release/native link、fmt 和边界正负自检均通过。所有最终运行均无源码漂移。

本轮 **10/10**，L09—L12 **2/4**，R01—R06 仍 **2/6**。专用 PostgreSQL 和两个 Linux holder 已停止，数据库零残留测试 schema、零其他连接；无待接回的 Cargo/PG 作业。V1 无 feature 回归的报告包装断言失败已原样保留；V2 仅改这条测试断言，636 个输入及全部生产源码与 V1 相同。详见 [最终验证](../../layering-2026-10-10.md#最终验证与证据) 与 [证据索引](evidence-index.json)。

下一轮从 L11 跨域事务所有者开始，L12 和 Skills reload TODO 保留。真实模型、Pi/声学/机械、Linux 全工作区与部署 smoke 未在本轮验证；用户 5433 未访问，默认 Shadow 和政策保持。未推送或创建 PR，未使用 reset 卡。
