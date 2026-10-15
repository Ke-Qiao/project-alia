# V01 旧 Companion 配对超时 — 夹具与等待审计

日期：2026-10-15；分工：`settings_audio`；基线：`c685e5a`。先只读审计，Root 完成两批基线并解除冻结后，仅修改 `tests/duplex_playback/companion.rs` 诊断及一项回归、写本文。Root 统一 Cargo、任务 PostgreSQL、二进制与输入冻结、结果归档及提交。没有真实模型、录放音、执行器或 SSH；R03 保持暂停。

## 旧失败与当前实际入口

历史证据见 [R04 Device HANDOFF](../2026-09-28-r04/device-health.md) 与 [2026-10-04 有界调查](../2026-10-04/runtime.md)。V3 workspace 中三项均超过原内层 25 秒；同 V3 二进制之后默认并行 9/9 通过。原 V3 failing artifact 后来已被构建覆盖，已保留的 V4 artifact 不能冒充原失败文件；此前 V4 三批通过也没有证明原因修复。

当前入口是 `packages/alia-server/tests/duplex_playback.rs`，其 `companion.rs` 子模块包含三项：

- `scanner_commits_one_fixed_invitation_through_real_voice_and_paired_playback`。
- `source_erasure_during_original_tts_prevents_first_media_and_preserves_charge`。
- `applied_resume_lost_ack_and_new_local_speech_handoff_to_h2_on_original_output`。

原 integration binary 为六项既有播放场景加上述三项，共九项。本轮最终诊断版增加一项 SQL/隐私回归，总数十项，原九项断言保持。它们共享真实 Delivery、嵌入 PlaybackService、两个本地 gRPC 服务、任务 PG、每例 SQLite 与绑定身份；仅 TTS、turn detector、native sample queue 是受控夹具，不构成声学/模型/硬件验证。

## 并发、阶段和期限

| 边界 | 当前事实 | 能解释什么、不能解释什么 |
| --- | --- | --- |
| libtest | 保留默认线程；每项 `#[tokio::test]` 默认 current-thread runtime | 九项可排队并行调度，不代表九项都已进入数据库夹具。 |
| 既有夹具准入 | `delivery_support::CASES` 原值为 3；permit 等待在 45 秒 timeout 之前 | 最多三份当前夹具同时活跃。不能通过改为 1 消除争用，也不能把排队时间算成内层 25 秒。 |
| PG 初始化 | 每例随机 schema/user；admin pool 上限 1，工作 pool 上限 4；迁移与初始 policy/source epoch 在 45 秒之前 | 三例各自 pool 和 schema 隔离，但共享 PG 服务/磁盘/CPU。实际连接数不是简单等于配置上限。 |
| 外层测试 | `with_db` 的 45 秒覆盖 `test(f.clone())`，其中含 paired Runtime 与 ServerBuilder | Fixture 初始化之前、最终 `f.cleanup()` 之后不在此 timer 中；命令自身仍需有界看守。 |
| Companion 开始 | 25 秒在 paired Runtime、临时原 Delivery 关闭、ServerBuilder、binding client 及独立 voice listener 已建立后开始 | 启动摘要/迁移慢不能直接被归为旧内层 25 秒失败；它们只能通过别的并发夹具/全局资源争用间接影响运行阶段。 |
| Voice 入站 | 25 秒中连接、Ready、启动每 100 ms capture heartbeat、启用测试政策、seed source/appraisal，然后等待 InitiativeAdmitted | 阶段名已保留；heartbeat 使用 MissedTickBehavior::Skip，sequence/PCM 不补写假帧。源只生成一次。 |
| Scanner | 5 秒 scan cadence、5 秒 scan_batch timeout、最多 32 行；live task 上限 4 | `ScanProgress` 只描述 shadow scan_batch。live discovery/runner 不被这些计数完整覆盖。 |
| 原机会 | Capture freshness 2 秒，VoiceOpportunity TTL 5 秒；actor mailbox 请求最多 5 秒 | 时钟新鲜度与 mailbox 排队不是同一边界；不能把刷新机会当作旧候选恢复。 |
| 准入 | 原 `admit_before=min(candidate expiry, opportunity expiry, last_capture+2s, next UTC day)`；最多三次同一 intent 核对、间隔 100 ms | 每次使用原冻结时限。PG commit deferred constraint 仍可拒绝到期事务；不得扩大 2 秒或把退休改成功。 |
| 媒体及控制 | 原 FixedTts semaphore、真实预算/解码/配对 playback；helper speech 1 秒、detector 2 秒、signal/eventually 3 秒 | 小 helper 超时会 panic；与完整 25 秒 timeout 的诊断路径不同。 |
| 清理 | 失败摘要先读，随后 cancel heartbeat、释放 TTS/resume 屏障、shutdown Runtime、关 voice/paired RPC；各 RPC join 3 秒 | `f.cleanup` 关工作 pool、DROP schema、关 admin。必须核对实际结束，不把发 cancel 当作已 join。 |

同名 channel 字符串在每个独立 Server state 中复用，不是进程全局 voice session key。live capacity advisory lock 由 `hashtextextended(current_schema(), constant)` 隔离，各随机 schema 不应竞争同一个逻辑锁；尚无动态证据表明该锁导致旧三例互阻。

## 时间与诊断解释

Actor 捕获到达时间由 Server `SystemTime` 记录，新鲜度使用该 Server 时钟。快照使用 Server UTC，`prepare_intent` 还要求该 `observed_at` 不在 PG `clock_timestamp()` 之后，并拒绝到达冻结期限；最终 commit 再由 PG 判断。现有 runner 的 monotonic `wait_ms` 仅测同一 Server 进程阶段，`remaining_ms` 仅是 Server UTC 对原期限的评估，不能据此推导 Server/PG/Embedded 之间的网络延迟或时钟同步误差。

若怀疑主机与 PG 容器时钟偏差，须在一次受控 SQL 往返前后记录宿主时间区间和 PG 返回时间，保留 RTT/测量不确定性；不更改系统时钟、不用未经测量的 offset 补偿生产政策。现有 `absent_opportunity_uses_database_time_while_future_observations_are_rejected` 已覆盖未来机会拒绝，`commit_checks_original_short_admission_deadline_without_refreshing_output_deadline` 已覆盖原短期限 commit 拒绝；它们是政策回归，不是旧 V01 根因证据。

当前 failure-only SQL 摘要最多 1 秒：ScanProgress、appraisal/response 计数、pending/admission 按 state/retirement_reason 分组、当前任务数据库 `pg_stat_activity` 的 state/wait_event_type 计数。没有 identity、source text、private grants 或原始 SQL 错误。`diagnostic_wait_ms` 包括 pool 获取、传输与 SQL，不能分解成单独 pool 排队时长。

确认的旧诊断覆盖限制：

1. paired 测试 filter 仅 `off,alia_server::modules::companion::runner=debug`，丢弃 `service` 中原有 scan deadline、live discovery 失败和 Failed(original_output/freeze_intent) 固定安全 warning。
2. `ScanProgress` 只在 shadow `scan_batch` 更新；live `retire_batch/discover_pending` 错误没有写入 `last_failure`。完成多个 shadow batch 不证明 live discovery 成功。
3. `timeout_state` 只在内层 `Ok(Err(timeout))` 调用；helper 的 1/2/3 秒 panic 或外层 45 秒终止不保证产生该摘要。摘要在清理前读取，但 panic 在清理后发出；不能把缺日志反推成未执行某阶段。

这些是明确的观测缺口，尚不是超时业务根因。现有 `report_live_outcome(RunOutcome::Failed("original_output"))` 发出固定安全事件；runner 的真实 PG rejected/lost Reserve 用例已确定产生这一 outcome，但直接调用 runner，不经过 service 的 report。Root 选择补最小诊断和真实 SQL/隐私证明，不新增镜像 filter 字符串的测试、日志框架或生产测试钩子。

两批基线结束后，本分工获授权修正上述诊断：filter 加 service=warn；摘要标注 `shadow_scan`，增加 `source_claims` 按 origin_mode 分组计数及 `rejected_sources` 计数；`failure_state` 接收现有 DB 引用与实际 ScanProgress 快照。`ScanProgress` 所在 service 模块未公开导出，私有 fixture helper 使用 `impl Debug` 接该返回值，避免为测试改生产 exports。

内层 timeout 和 catch_unwind panic 都读取原最多 1 秒摘要，并在 cleanup 前输出固定 stage 与脱敏摘要；最终 timeout panic 或原 panic resume 继续保留。外层 45 秒直接 drop 的路径仍无法在 helper 内恢复已丢弃状态，不声称该路径已补全。没有改变任何成功路径的机会、事务、播放或等待策略。

新增 `failure_diagnostic_reports_live_claims_without_private_values` 使用原真实 PG `with_db`：插入一条 enabled source claim 与 rejected marker，断言 shadow scan 仍为零时 SQL 摘要能独立显示 live 计数；逐项禁止 owner/source/appraisal UUID 与 generator 私有 sentinel 出现在摘要。随后只在本例 schema 重命名 origin_mode 列制造 SQL 错误，要求摘要返回固定 `unavailable`，不泄露原错误。没有启动 Companion worker、设备、模型或额外服务；原 schema cleanup 回收所有行与 DDL。`rustfmt --check` 和 diff whitespace 检查通过，没有整体展开旧紧凑 fixture 格式。

## 有界复现矩阵

1. Root 对当前 workspace 执行原 feature 的 `--no-run` JSON 构建，固定实际 duplex binary、SHA、Cargo 输入与环境；保留默认 libtest、原 CASES=3 和期限。
2. 同一当前 binary、同一任务 PG 环境默认运行完整九项两批，分别保留首次/后续结果。第二批是缓存/资源状态对照，不称为绝对“冷启动/暖启动”，不增加 unique tests。
3. 前两批均未复现，Root 随后授权上述最小测试诊断补丁；最后完整 workspace 默认上下文运行一次。这第三批是十项诊断版，不是前两批同源 baseline。不人为并发其他独立 Cargo/PG workload，workspace 执行顺序本身也不是 2026-09-28 原始负载的完全复刻。
4. 任一失败立即转其原阶段、ScanProgress、pending/admission reason、runner wait/budget 和任务 PG 等待核对，不继续无限空跑。只有捕获具体等待/条件并用同一身份证据验证修复，才讨论关闭对应原因。

可以区分的观察：Ready 前为配对/认证/transport；seed 后 appraisal 为 0 为来源/评价链；shadow 在途而没有完成是 scan 等待；shadow 完成但 pending 为 0 仍需 live discovery warning；freeze ready=false 需区分库可用性与原机会时钟；admit deadline/Expired 是原窗口拒绝；admit Fresh 后无 InitiativeAdmitted/媒体是 reserve/output 分支。没有失效阶段就不猜测 CPU、锁、时钟或模型为根因。

## Checklist 与交接

- [x] 核对旧三项失败、当前九项 binary、历史原件身份限制。
- [x] 核对原并发、PG/SQLite/RPC 隔离、25/45 秒覆盖范围与清理。
- [x] 核对 scanner、机会/准入时限、时钟与现有确定性过期回归。
- [x] 列出安全诊断覆盖与盲区，获 Root 解除冻结后补最小诊断及真实 SQL/隐私回归。
- [x] 向 Root 提交不改期限/并发的有界复现矩阵。
- [x] 核对 Root 两批 baseline 和最终 workspace 原日志、摘要、冻结源码与保留二进制；本轮未复现，V01 保持开放。

当前分工 **6/6**；Root 两批原 binary 默认九项分别 **9/0/0**，14.40 / 13.28 秒，未复现。最终 workspace（诊断版）**1245 passed / 0 failed / 5 ignored**，总命令 436.72 秒；其中完整 duplex **10/0/0**，原日志 target 用时 13.77 秒。五项 ignored 是既有三项真实 PTY 和两项真实 Easy-Turn 前置用例，本轮没有增加 ignore 或 filter。两批 baseline 重复执行不加入最终 unique 总数。生产源码、政策、任何期限与并发均未修改；没有新增依赖或框架。默认 Shadow、精确取消、资源所有权、原一次机会/收费/Response/Playback 断言保持，V01 仍开放。

## 最终冻结证据复核

恢复后本分工只读复核当前 diff、完整调用点与清理次序、`delivery_support::with_db`、service 的固定 warning 字段，以及 Root 已完成的日志和身份文件；没有另跑 Cargo、PG、SSH、硬件或 Git mutation。

- [最终 workspace 日志](evidence/final-workspace.log) 明确包含新 SQL/隐私回归及三项 Companion 原用例通过；[汇总](evidence/validation-summary.json) 与 target 最终计数相符。
- [冻结输入](evidence/final-source-manifest.json) 的 669 项身份由 Root 管理；本分工逐字节核对改动文件 `companion.rs` 的 SHA-256 为 `5565c1262683334b86209eb038d144c5dcc9e20a21dbc3e0aed797d5744a88b4`，与最终输入一致。
- [保留二进制身份](evidence/final-binary.json) 对应 `run/validation-20261015-v01/final-duplex-playback`；实际文件字节数 `109059760` 与 SHA-256 `c26bf833583ad6b9dd8b737d2ddef8414d86ed498b49137d163658d51458fa33` 均一致。这是诊断版，不能代替 baseline，更不能代替已缺失的历史 V3 失败原件。
- SQL 摘要仅取 aggregate 与已约束的 state/mode/reason，`ScanProgress` 仅有数字和固定安全错误类别；新增测试证明 live claim 不依赖 shadow 计数，同时覆盖 SQL 错误返回固定 `unavailable`。本轮无失败，因此没有实际执行到完整 paired failure-cleanup 分支的动态证据；其清理前收集与输出次序通过源码复核，不夸为运行证明。
- 25 秒内 timeout 或 panic 都先获取最多 1 秒摘要、立即输出，再 cancel heartbeat、释放屏障和 join Runtime/RPC；原 panic 最后恢复。该 1 秒是既有 failure-only 上限，本轮没有延长成功路径或原期限。外层 45 秒、初始化和 cleanup 中断仍不保证同样摘要，已保留为诊断边界，不能据此声称旧超时根因已定位。

结论：当前修改和冻结证据可用于本轮提交；没有发现需再改生产或测试代码的缺口。下一次只在新失败证据到来时按原条件定位，不以继续空跑或延长期限代替根因证据。
