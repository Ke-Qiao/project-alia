# Root / 交付核对、V01 与 R04 补充审查

日期：2026-10-04。任务依据为用户提供的 `Alia_R01_R04_Review_and_Next_Sprint_2026-10-04.md`，其 SHA-256 为 `af38f76e00122eb09b6d5c033688fbb159c1bbc8ab68f07292e0833c43d2b9ad`。该文件中的历史“通过”首先作为待核对报告；以下区分历史原件核验、本轮执行和未完成出口。

分工沿用三个 subagent：`audio_readiness_1004` 负责 R02，`motion_protocol_1004` 负责 R03，`linux_runtime_1004` 负责当前 Linux；Root 负责历史证据、V01、setup、冻结验证与提交。共享源码按文件分工；Cargo/PG 只有一个验证窗口。默认 shadow、两类暂停、来源撤销、预算、CAS、原 owner 与 unknown 语义保持；没有改用户部署或发真实动作。

## 交付身份链：已独立核对

开始时 branch=`codex/architecture-refactor`，HEAD=`8c1742287b75720a2530711ce78c447679369884`，工作区干净。只读脚本逐文件读取 Git 对象、现有输入清单和证据原件，结果见 [delivery-audit.json](delivery-audit.json)。

| 范围 | 源码提交 / 记录提交 | 输入核对 | 历史执行结果及本轮核对范围 |
| --- | --- | --- | --- |
| R01 live | `a79cb1f0b2c45847dce561dfde549f4318a1312a` / `75bf97c` | 630 项全部匹配，manifest `58c76e82adf71fed96fa6b113f51185412dd3bd0619a3db747835f3d81accd4a` | 历史 workspace 1036/0/5，另 PTY 3、模型 2；51 份索引证据哈希匹配。不是本轮重新执行 R01 原源码。 |
| R04 | `8f5bb83f4a313723428abf854c1e663b159c6c45` / `8c17422` | 633 项与源码提交及本轮开始工作区全部匹配，manifest `35efd1807b5c2820b3024fc6142bf769ace5973cc13275e0b5649f8bec011a8b` | 36 份索引证据哈希匹配；重新解析原 `workspace-v4.log` 得 1053/0/5。`8f5bb83..8c17422` 只有 docs 变更。 |
| 本轮 | 从上述干净 HEAD 补缺口 | 冻结清单与提交核对见 [验收摘要](validation-summary.json) | 本轮命令、退出码、二进制身份单列，不能继承旧源码通过数字。 |

原 workspace 日志 SHA=`c1dd568908b9157061d13fd4d8713375c25fe0ca5ee57c48132e263674521d19`。按 `target + test name` 去重有 1058 个已列出测试，其中 5 ignored；`module::lifecycle::tests::panicked_stop_retains_owner_and_dependencies_as_incomplete` 的子进程重复输出不增加计数。原模型日志包含两项不同 named test 通过。原 PTY 证据有实际 `isatty(0/1/2)` 元数据和当时手工观察记录，未保留原始重定向 transcript；本轮没有把它冒充可独立重放的完整原件。历史 1058=1053+3+2 的口径成立，R01 的 1041 不再相加。

633 是本轮沿用的输入清单规模，包含部分说明/样例，并非代码量、质量或全部运行资源保证。具体配置、proto、迁移、build.rs、锁文件与资源路径/哈希在 [build-input-inventory.json](build-input-inventory.json) 和冻结清单中；运行配置、native 归档、镜像、解析 feature、二进制另在 Linux 证据中。

### 5 个 ignored 的逐项映射

| target / 完整测试名 | 原因与运行依赖 | 历史补充证据 | 仍不能证明 |
| --- | --- | --- | --- |
| `alia-server lib` / `bootstrap::setup::tests::setup_terminal_worker_releases_input_after_cancellation` | 需要真实 PTY，独占终端；不能重定向子进程 stdin/stdout | R04 `pty-v4-result.json` 与 observation | R05 实际安装和长期运行；本轮 3 项真实 PTY 已通过并归档 transcript |
| `alia-server lib` / `channels::cli::tests::idle_terminal_worker_joins_and_restores_terminal_on_shutdown` | 同上，crossterm 全局终端状态要求该三项串行 | 同上 | 同上 |
| `alia-server lib` / `channels::cli::text::tests::real_console_write_proves_utf8_bytes_and_restores_terminal` | 同上，写入实际控制台 | 同上 | 同上 |
| `easy_turn_live` / `official_audio_through_production_rust_adapter` | 真实 Easy-Turn CPU 服务与官方 `backchannel_real_1` PCM16 | R04 `models-v4.log` / result | R02 声学、R05 Pi 整机；本轮未重跑实际模型 |
| `voice_stream_live` / `official_audio_over_grpc_preserves_original_response_across_backchannel_and_wait` | 同一真实服务、官方 PCM 目录、本任务隔离 PG | 同上 | 受控输入的模型/gRPC 证据不替代 AEC、双讲、物理停声或 R06 |

这些 ignored 是专门环境入口，未从普通 workspace 中忽略掉失败；本轮当前执行情况只取验收摘要，旧模型通过不计入当前源码总数。

## V01：有界调查完成，根因仍开放

旧 R04 V3 的三个 Companion 场景超过原 25 秒；随后同 V3 二进制 9/9 默认并行通过，V4 加诊断后完整通过。这些失败原件全部保留。

本轮核对发现：旧失败 binary 的记录 SHA 为 `b30c9aa9ac17f9fc8f5ab3b106f7945dad8c76f2cdc983e323170479d6151059`，原路径已经被 V4 编译复用。当前保留文件 SHA=`e529ff72ad2e1805040a07291cf152f1625b6c38c9aea4400ba35a801d6fe087`，107330192 bytes。本轮先把它另存为 `run/validation-20261004/duplex-playback-v4-retained`，不称为原 V3 artifact，也不声称能在完全相同初次负载下复现。V4 的关联来自目标路径、mtime、当时构建记录与当前匹配 V4 的源码；本轮未通过可复现构建反向证明这个旧二进制的全部输入。

在改源码、重新 Cargo 之前，仅执行三批 `duplex-playback-v4-retained --nocapture`；对应原 V4 633 输入，无漂移，默认线程/栈、原 25 秒测试期限、真实隔离 PG：

| 记录 | UTC 开始 | wall 秒 | 结果 |
| --- | --- | --- | --- |
| `v01-baseline-01` | 2026-10-04 10:08:39 | 19.69 | exit 0，9/0/0 |
| `v01-baseline-02` | 2026-10-04 10:10:47 | 15.31 | exit 0，9/0/0 |
| `v01-baseline-03` | 2026-10-04 10:11:03 | 15.45 | exit 0，9/0/0 |

27 次执行只有 9 个唯一用例；无超时，故没有该批次失败阶段/state/retirement_reason 可报告。初次 failing binary 缺失、冷启动/资源争用、瞬时数据库延迟和时钟偏差均未排除。没有无限空跑、刷新机会、延长 deadline、串行化配对或把退休改成成功。

实际代码路径：

1. `grpc/voice_session/initiative.rs` 在原 capture actor 排空 capture、复核当前权限/owner 后快照；收到 capture 的 Server `SystemTime` 新鲜度上限 2 秒，机会 TTL 5 秒。
2. `modules/companion/live/repository.rs::prepare_intent` 冻结 `admit_before=min(candidate expiry, opportunity expiry, last_capture+2s, next UTC day)`；准备与事务提交都重新检查原期限。`valid_until` 另保持原候选 expiry，暂停/重试不延长。
3. runner 只对同一不可变 intent 做最多 3 次、100 ms 间隔的有界核对；NoOpportunity、freeze failure、Expired 和输出失败有不同路径。2 秒内过期应拒绝，不能靠替代机会恢复旧候选。
4. 当前成功 fixture 在 builder、绑定/gRPC、Ready 与 capture heartbeat 建好后才启用隔离测试政策、生成一次真实来源。成功仍要求原一次 TTS/Response/Admission/Playback，源擦除仍在原任务排空后验证无迟到媒体。
5. 独立过期回归已经存在：`modules::companion::live::tests::commit_checks_original_short_admission_deadline_without_refreshing_output_deadline`。它持有真实生产事务至原 deadline，要求 COMMIT deferred constraint 拒绝、Context/Response/grant 回滚，同 intent 后续退休；本轮复用，不另造重复用例。

本轮增补的是诊断，不是根因修复：runner 只记录固定 phase/outcome、同进程 monotonic wait 和 Server 对原合同 UTC deadline 的剩余预算；不记录 source/user/device ID、原文、grant 或原始 SQL。配对测试只启用该 runner namespace 的 debug，失败时仍限 1 秒读 ScanProgress、分组 state/retirement reason，并加本任务数据库 `pg_stat_activity` 的 state/wait_event_type 计数。诊断查询耗时包括 pool+transport+SQL，不能宣称单独量出了连接池等待。

时钟边界：OS `Instant` 只衡量同一 Server 进程内阶段；Server UTC 剩余量是既有 deadline 的本地评估，不是数据库/设备间延迟。持久时限由现有 PG `clock_timestamp()` 判断；来源 expiry 还包含存储时间，跨进程 UTC 同步是假设，本轮未独立校准。不会用宿主开始时间减 PG/Embedded 时间来声称测得排队或网络延迟。

当前冻结源码经完整 workspace 构建后，将实际 paired binary 另存，SHA=`32a8c0f51c9df30ea343e4f1bc5bc821882f93547515aec23b888051e0dc2a59`；再独立运行一次 `--nocapture` 验证诊断可用，exit 0、9/0/0、17.73 秒 wall、drift=[]。三个匿名 attempt 均 Fresh，原预算剩余量分别为 1924/1939/1955 ms；snapshot wait 为 7/5/3 ms，freeze 为 3/1/1 ms，admit 为 66/51/50 ms。这是默认并行下三个固定阶段样本，不按输出顺序关联具体场景、不推断可靠性或延迟分布。原始短日志已归档 [paired-diagnostic-v1.log](paired-diagnostic-v1.log)。完整 workspace 同时通过既有确定性过期回归；成功与过期仍分开断言。

**V01=open**。本轮新二进制诊断回归与既有确定性过期测试的结果见验收摘要；它们不能弥补原 V3 二进制缺失。扩大无人值守主动投递前，仍需具体根因修复证据，或项目负责人明确接受有边界的发布限制；部署默认 shadow 不变。

## R04 补充审查与修改

### setup

- 已实现：`finish_setup` 复用现有 watch 写锁，覆盖准备/文件发布与唯一完成通知。HTTP、CLI 创建及取消共享一位完成状态，失败保持 None 可重试；输家不再写文件，不覆盖 ConfigReady。没有第二套锁/任务/配置框架。
- 已实现：共享 cancellation token 在完成门内检查；drain 前封闭取消状态。已接收但未完成 body 的 HTTP 请求不能在关闭后发布；CLI 普通竞争失败不把 HTTP 成功变成整个 setup 失败。
- 已实现：允许 Host 的 `127.0.0.1` / `localhost` / `[::1]` 字面 authority；有 Origin 时必须精确等于 `http://Host`，拒绝异常/重复 Host、跨来源和 opaque Origin。JSON extractor 保留 Content-Type 检查；无 Origin 的本地 curl/SSH tunnel 仍可用。边界依据是本机操作者可信，**不是对同机恶意进程的身份认证**；未引入 token 系统或反向代理信任。
- 已有实现保留：实际 loader/fragments/role 校验、0600 临时 base、file sync、rename/hardlink、canonical symlink 目标、loopback-only 与原 2 秒 drain。保存业务失败继续 HTTP 200 + `success=false`；Host/Origin 拒绝 403、错误媒体类型 415 属于请求入口拒绝。
- 已核对但开放：没有父目录/新 fragment 目录 fsync；CLI fragments 使用普通 write。只能声明原子 base 发布，不能声明整树断电持久。文件 sync 不保证目录项落盘，见 [fsync(2)](https://man7.org/linux/man-pages/man2/fsync.2.html)。本轮未做真实断电或进程 kill 冒充断电测试。
- 已核对但开放：路径由可信本地操作者选择，进程权限是实际可写范围；不建议以 root 运行。既有 fragments/symlink 可由同 UID 外部进程在验证和发布间改变；本轮单次门只解决本进程 HTTP/CLI 竞争，不提供敌对文件系统快照或多进程安装事务。

Host/Origin 与非 simple JSON 的组合用于浏览器来源约束；相关原理见 [OWASP CSRF 来源/Content-Type 检查](https://cheatsheetseries.owasp.org/cheatsheets/Cross-Site_Request_Forgery_Prevention_Cheat_Sheet.html)。本轮没有以 loopback 或 CORS 一项宣称完整访问控制。

新增原 fixture 回归为 `concurrent_http_saves_publish_one_configuration_and_one_completion`、`cli_completion_or_cancellation_prevents_late_http_writes`、`setup_rejects_foreign_hosts_origins_and_non_json_without_writing`，以及实际 setup lifecycle 的 `cancellation_rejects_an_accepted_save_whose_body_finishes_during_drain`。最后一项用 HTTP 100 Continue 确认服务器等待 body 后再取消，不依赖 sleep。

### health

源码确认 `ModuleRegistry::health` 仍顺序调用，router 可能再次探测 minor LLM，并跨该 await 持有自己的 read lock。没有聚合总预算、同 profile 去重、并发请求限额或 revision 绑定缓存。OpenAI 单次 5 秒不构成整个 `/health` 的上限，外部接口成功也不是生成/语音健康证明。

实际消费者包括 CLI 操作文档、discovery 的 health 元数据、现有 frontend `useHealth` 15 秒刷新和 `useProbeServer`（只读确认，未修改前端）。本轮保留 API 与默认 `health_probe_enabled=false`，没有启用付费服务。后续应先决定正常巡检与显式深度诊断的兼容契约，再在现有模型内处理预算/重复探测；当前没有足够理由引入新健康框架或无 revision 缓存。此限制已进入原 R04 checklist。

### device

REST online 仍只代表本次公开 `GetBindingStatus` 查询成功且 ID 非空；遥测缺失保持 null。它没有进入 Body/Playback 的准入 gate。真实 loopback unbound/foreign binding/reconnect、新 boot 与原身份拒绝证据见 [motion.md](motion.md)，Linux release 两真实进程的 status 路径见 [linux.md](linux.md)。均不替代硬件 pin 的现场核对或物理反馈。

## 目标架构能力 → 源码 → 测试 → 缺口

下表是具体能力映射，不以某个提案候选组件是否安装判定架构完成。测试名称/目标证明所述软件边界；实际本轮执行状态以验收矩阵为准。

| 能力 | 当前源码 | 既有测试或本轮补证 | 明确缺口/延期 |
| --- | --- | --- | --- |
| Rust 模块化单体、受管启停 | Server `module/builder`、`bootstrap/runtime`、`module/lifecycle`；Embedded builder/runtime | `server_runtime`、Embedded `runtime`，新 Linux release lifecycle | container 不等于 Pi；升级/真实模型共存/24h 未验收 |
| 自然全双工、临时避让/明确暂停 | `grpc/voice_session`、`modules/interaction`、`playback_session`、Core control types | `output_control`、`output_lifecycle`、`duplex_playback`、真实模型旧证据 | AEC/双讲/声学尾音与用户体验待 R02/R05 |
| 原输出 owner 与旧 generation 失效 | Core output/playback control、Embedded PlaybackService、Server Delivery | `duplex_playback` 原 9 项；native/gRPC/lifecycle 故障测试 | 不证明 DAC 静音或真实机构停止 |
| Context 规范来源与不可换权 | `modules/memory/context`、source/capture；事务内验证 | `context_lease`：`validated_transaction_serializes_clear_until_effect_commit`、`authority_cannot_be_refreshed_or_rebound_and_notice_is_fixed` | 更广外部工具/模型仍按具体契约接入 |
| 规范记忆/派生索引/隐私删除 | memory service/repository/embedding、privacy epoch、pgvector | `memory_privacy`、`memory_embedding_provenance`、`memory_embedding` | 备份恢复后的删除策略、真实语料召回质量未由这些事务测试证明 |
| Appraisal→关系/情绪/Regulation | Appraisal provenance、expression reducers | `appraisal_provenance`、`expression_appraisal`、`expression_regulation` | 判断是有来源的解释，不是用户事实；真实体验评价开放 |
| Companion shadow与有限 live | `companion/{service,runner,live}`，Context purpose、voice initiative actor | `companion` lib 事务/runner、paired scanner/TTS/H2、REST CAS | V01 open；默认 shadow；固定邀请不是自由主动工具/动作 |
| 一份 Response、多种投递证据 | `modules/delivery`、BodyDelivery ledger、media/control owner | `delivery_repository/service/text`、`body_delivery`、duplex paired | 前端与多终端产品体验 D01/D04；邀请供后续 history 只读引用仍开放 |
| 执行审批、事务 outbox、副作用 unknown | `modules/execution` repository/service | `execution_repository`：approval/outbox 原子回滚、取消/claim 单赢家、过期 dispatch→unknown；`execution_service` | 具体新外部服务/Temporal/Worker 需用例和独立验收，不按提案列名加入 |
| 签名 Body、唯一 writer、部分提交 | Embedded `grpc/body_service`、`module/motion`；Server body runtime/store | 本轮 2 项真实 endpoint 拒绝 + 原 pin/ACK/账本测试 | 资产内容未签名，MCU boot/ACK/反馈/限位未知；R03 未关闭 |
| setup、授权、诊断 | OOBE、当前 owner/admin 设备 API、settings、health | 原 R04 HTTP tests + 本轮单次完成/来源/取消 tests | health 总预算与目录断电持久仍开放；TLS/现场权限不能由 loopback 测试推导 |
| 逻辑角色与部署位置分离 | 两端库/入口、gRPC、Provider endpoint、Cargo features | 旧 no-default check + 本轮默认 Linux release/link/run | Cargo features 只选编译能力；C04/C05 同机/端侧模型/动态迁移需另验，Easy-Turn 不迁 Pi |

## 当前验收与恢复入口

本轮源码提交 `010f7aa5ae2c39a5c90c7f22979391a6b1bbbf23` 与 633 个冻结输入逐项匹配，见 [提交核对](committed-source-verification.json)。macOS workspace 1062/0/5 + 真实 PTY 3；Linux 默认 release/link/run 与 8 目标 578/0/3。两平台不相加，两个真实模型用例本轮未执行。

本轮冻结和实际运行结果收录于 [validation-summary.json](validation-summary.json)，四份 HANDOFF 总入口为 [README](README.md)。[证据索引](runtime-evidence-index.json) 将原件、归档副本和哈希相连；[资源状态](resource-cleanup.json) 记录两个新容器停止、任务 schema/连接为 0，旧资源未修改。归档 PTY transcript 保留真实 CRLF/终端控制字节，配对诊断日志保留原始末尾空行；完整 staged whitespace 检查因此报告这两个日志的原始空白。未归一化原件，作者编写的代码/文档 whitespace 检查通过，最终文档检查仅排除这两份按哈希保存的原始日志。原失败日志/旧快照不覆盖；当前新增资源停止状态单独记录。R01/R04 初始软件包 2/6 完成不变，不换算完成百分比；R02/R03 本轮只关闭有直接证据的软件前置项。
