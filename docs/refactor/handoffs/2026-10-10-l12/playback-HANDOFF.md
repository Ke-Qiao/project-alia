# L12 Playback cleanup HANDOFF

负责人：`/root/l12_playback_timeout`。基线 `d5481b5`，分支 `codex/architecture-refactor`。本文件记录实现和诊断边界；Root 持有唯一 Cargo、数据库与容器验证窗口。下列运行证据均由 Root 执行，subagent 只做源码修改、只读分析和诊断脚本准备。

## 已完成的源码工作

- [x] 读取 L11 Root HANDOFF、Linux V4 初次失败、定向复查和完整复跑原始日志。
- [x] 追踪 child 父进程期限、Fixture 创建/清理、migration 锁、ServerBuilder 首次执行器身份 SHA-256、NativePlayback、精确 cancellation panic、保留的 media future、retirement 和 shutdown quarantine。
- [x] 在 `tests/server_runtime.rs` 增加只在 `ALIA_TEST_RUNTIME_PHASES` 下启用的 `runtime_phase` 观测；目标 child 显式启用它。每条 stderr 日志用固定 `[runtime-phase]` 前缀，含进程 PID、Unix 毫秒和进程内单调 elapsed_ms。
- [x] 对进入及完成阶段分别记录：PG admin/schema/pool、migration、ServerBuilder、Response、output bind、native blocking enter/return、精确 cancellation panic、Unknown 持久化观察、native release、retirement、quarantine、DB ping、fixture pool close/schema drop/admin close、child exit。父进程 kill 时保留最后一个 await 所处阶段。
- [x] 按 outbound agent 的最终契约迁移 `NativePlayback` 与 `PanicFencePlayback` 的 `DeliveryTransport` 实现及 `DeliveryTarget`，保留 ingress `ChannelAdapter` 的 capability/type 方法和原始测试语义。
- [x] Root V1 Linux 定向运行，记录完整阶段及任务数据库每秒活动/等待采样。
- [x] 用匹配真实测试可执行文件 fingerprint 的同一 sha2 rlib、相同 64KiB 生产算法独立测量，验证摘要与 `sha256sum` 完全一致。
- [x] 证明 V1 大部分预算用于未优化 sha2 的整文件身份计算；Root 仅为现有 sha2 增加 dev package `opt-level = 3`，没有扩充依赖或 feature。
- [x] Root V2 重测真实 child、实际 V2 binary SHA probe 和保留 V1 binary 的同字节 SHA probe。
- [x] Root V4 默认并发完整 Server 验证通过，最终同源证据已归档。

## 保留的保证

既有父进程 **20 秒总期限**、45 秒 fixture body 期限、native 信号等待、shutdown 的 40ms 答复预算、所有资源/数据库/媒体保留断言不变。没有增大超时、降低测试并发、忽略测试或把 hash 提前到父预算之外。没有改生产 hash、migration 锁、精确取消或 native owned cleanup 路径；没有新依赖。

观测不产生新的后台任务、数据库连接或持久状态。schema 名用于 Root 在任务数据库中定位等待；不输出连接串、凭证、查询内容或用户数据。

## 已知证据与待证明假设

L11 原失败只证明 cancellation panic 已触发且父进程总期限耗尽。无阶段时间戳，无法确定是在 setup 消耗大部分预算后进入 panic，还是在其后的等待/fixture cleanup 停滞。定向复查 16.40 秒通过，随后完整默认并发复跑通过；不能据此称为修复。

两个源码支持的候选目前仍需运行证据：

1. `ExecutionService::new` 等待进程级 `OnceCell` 对整个真实可执行文件 SHA-256。新的隔离 child 不共享父进程缓存；L11 二进制 461182688 字节。该步骤在 `server.build.begin` / `done` 范围内。
2. `migrations::apply_in_transaction` 在每次 migration 获取固定 `MIGRATION_LOCK_KEY` 的 PG transaction advisory lock。同一任务数据库的不同测试 schema 会争用同一个锁，而 child 不共享父进程的 `DATABASE_CASES = 2` semaphore。显式首次 migrations 与 ServerBuilder 内再次迁移均可能等待。

如果阶段停在 build，仅凭 build 耗时不能断言 SHA-256 是唯一原因。优先关联同时间段任务 PG 的 `wait_event` / `pg_blocking_pids`；需要更细分时先协调，避免扩大生产观测与测试范围。

## L12 V1 的真实定位证据

Root 定向执行通过 **1/0**，child **16.551 秒**；其中 `server.build` **16.249 秒**，首次 migrations **118ms**、native release/retirement **2ms**、fixture `DROP SCHEMA` **29ms**。任务 PG 采样共 73 条、无错误、全部无 active/waiting 记录；其中 build 时间窗内 15 个采样为空。这只能排除采样命中的长等待，不声称排除一切亚秒 PG 活动。

为将 build 进一步细分，准备了 ignored evidence 中的 `build-digest-probe.rs` 与 `measure-build-digest.py`，由 Root 在同一 Linux holder 执行。脚本从实际 `server_runtime` fingerprint 的 sha2 dependency hash 自动寻找唯一匹配 rlib，而非随便选择缓存中的一个文件；直接 rustc 用同一 rlib、默认 opt0 编译与生产一致的 64KiB read/update/finalize 循环。65,537 字节跨 buffer 向量先与 Python SHA-256 对照，再对真实测试二进制计算并与 `sha256sum`、前后文件 identity 对照。

- V1 实际 binary：462699808 bytes，SHA-256 `e3dab6f2f4586861b66c589eda3db87f4ecd775277adde569da8a482127bb2a5`。
- 精确 rlib：`libsha2-94ad25fc377d8918.rlib`，351602 bytes，SHA-256 `00a116b40cc7cd9e1a74912e087dd5d0a19a24713a4948c6c9c917fd1ccbf836`；features 为 `default,std`。
- 同生产算法实测 **15.416193875 秒**；系统 `sha256sum` **0.864939500 秒**。所有 digest/bytes/前后 identity 完全一致。

这证明当前 ARM64 debug child 的启动预算主要消耗在现有 sha2 整文件计算，并非测得 native 释放或 fixture 清理卡住。Root 决定只为既有 sha2 crate 增加 `[profile.dev.package.sha2] opt-level = 3`：不改变 production digest 算法、输入、输出、OnceCell、release profile、依赖或 features，也不改变 20 秒测试期限。V2 需测同一份保留的 V1 binary 来消除 binary 大小变化的比较偏差，再测完整 child。

**历史边界：L11 超时没有阶段观测，不能追认其每一毫秒均由 hash 引起，也不能从 V1 定向无 PG 等待推出全量并发永远无锁等待。** L12 定位的是可复现且已测量的主要启动预算消耗原因。

## V2 优化对照

Root V2 定向执行通过 **1/0**，仍保持父 **20 秒**期限。真实 child 从 **16.551 秒降至 1.280 秒**，`server.build` 从 **16.249 秒降至 1.014 秒**；native release 仍 **2ms**，schema DROP **30ms**。全部原始保留断言及阶段序列通过。

独立 probe 使用 V2 实际 target fingerprint 指定优化后的 `libsha2-1eee5cca70ceabdd.rlib`（858224 bytes，SHA-256 `6e241f40da4c16e0bd0be954e2b5d0d39ef7682041e9d97b8b8bf6e36ab32552`）。对保留的 **同一 V1 binary**（462699808 bytes，仍为 `e3dab6f2…127bb2a5`）测得 **0.833610583 秒**，相比原 **15.416193875 秒**约快 **18.49 倍**。同字节前后摘要、Python SHA-256 和系统 sha256sum 均一致，因此结果没有依赖 V2 binary 大小差异。

V2 实际 binary 为 462511360 bytes、SHA-256 `f09535930fbaf726f136bc47095ab087fd5b79013c2209f0d6263a7473d3de33`，自身 probe **0.846088208 秒**。没有改变 SHA-256 算法或弱化工具身份完整性。V1 诊断脚本保留原版；V2 脚本新增 `--dependency-target` 明确分开“选择已编译 rlib 的 target”与“被 hash 的保留 binary”，日志记录二者身份。

已证明的 debug SHA 启动预算问题已有受测修复。长期系统调度与未采样的短暂 PG 等待仍不应被描述为绝对排除；完整默认并发运行由 Root 做最终验收。

## V3 验收发现的 setup 关闭断言问题

macOS V3 workspace 验证的 Server lib 出现 **320/1/3**，唯一失败为既有 `bootstrap::setup::tests::cancellation_rejects_an_accepted_save_whose_body_finishes_during_drain`：完成结果 `None`、target 文件不存在、目录不存在均通过，最后“对原地址新建 TCP 连接必须失败”的断言失败。Root 用同一 V3 binary 定向复查 **0.15 秒通过**；这只能说明该次复查通过，不构成原全量失败已修复的证据。

只读检查确认：`run_setup_on` 在原 axum server future 完成或丢弃之后才返回；当前 axum 0.8.9 graceful shutdown 路径明确 `drop(listener)`，随后等待所有连接任务结束。`HttpListener` 直接拥有一个 Tokio listener，没有复制 listener 的路径。新连接成功只证明测试时该地址可连接，不能识别接收者是否仍是原 owner。其它并行测试重新绑定临时端口是可能原因，但 V3 日志没有记录当时 peer 身份，**不能宣称已经复现或证明端口重用，更不能凭单次复查排除真实泄漏**。

Root 批准用更直接的原资源身份断言修复测试：

- `run_setup_on` 仅 `cfg(test)` 接收可选 oneshot sender；测试包装实际传入 axum 的 `HttpListener`。
- `SetupListenerProbe::drop` 先 `take` 并 drop 真正的 inner listener，然后发送 witness，避免只观察 setup 作用域离开。
- 两个真实 HTTP setup 回归均在 serving JoinHandle 已结束之后 `try_recv`，要求原 listener 当时已经释放；随后原 accepted socket 在既有 1 秒内达到 EOF，或明确的 `ConnectionReset`。其他 I/O 错误和超时均失败。
- 保留 `100 Continue` 证明请求已被接收、取消结果、无文件/无目录及现有服务/连接期限。非测试生产资源生命周期、路由、取消和 drain 行为保持。
- 不把同地址再次 bind 加入默认测试：同步执行也不能阻止其它 libtest 线程竞争端口，会重新引入地址归属歧义。ignored run 中的地址复用草稿只用于记录曾评估的诊断方式，未运行、未纳入产品测试。

subagent 仅生成 ignored `setup-listener-drop-proposal.patch` 并用 rustfmt 解析；V3 workspace 完成后由 Root 应用，随后冻结 V4。这项改动验证的是原 listener 和原 accepted socket 的实际释放，而不是放宽关闭要求。V4 完整同源验收由 Root 记录。

## Root 验证入口

定向测试：`cargo test -p alia-server --test server_runtime delivery::panicked_playback_fence_retains_original_native_future_and_dependencies -- --exact --nocapture`。Root 设置任务专用 `ALIA_TEST_DATABASE_URL`，沿用 Linux ARM64 现有验证镜像与默认并发策略。之后完整 Server 测试保持默认 libtest 并发。

保存完整 stderr/stdout，不剥离 `[runtime-phase]` 行。按 `unix_ms` 与任务 PG 样本时间对齐；进程内部按 elapsed_ms 计算差值。若 child 被 kill，正常 fixture cleanup 不保证执行，Root 仍须核对并关闭任务遗留 schema/连接，不能操作用户数据库。

Subagent 未运行 Cargo、PG 或容器。Root 已完成 V1/V2 同算法、同字节与真实 child 对照；V4 完整同源验收已由 Root 完成，见最终补记。L11 历史超时和 V3 setup 地址可连接的具体原因均不能无观测地完全追认归因。

## Root V4 最终验收补记

源码 `d9f97216a6599742c45e0a6389110b6b465e8461`，V4 manifest `bc542c0718b07fc45c1f7e09084a51fddfa3350014eee089514e6a582f9e75ea`，641/641 Git blob 一致。

macOS workspace **1060/0/5**、真实 PTY **3/0/0**、无默认 availability **2/0/0**、提前准入 **1/0/0**；Linux ARM64 Server 默认并行完整测试 **778/0/5**。两端无默认全目标检查及 Linux release/native link 均通过。计数依次为 passed/failed/ignored，子进程摘要不重复计入。

最终 V4 默认并行 child 2.983 秒，其中 build 1.833 秒、native release 2ms、schema DROP 87ms。 setup 定向 4/0/1，完整双平台均通过；原 V3 失败及原二进制定向复查保留。详见 [Root HANDOFF](root-HANDOFF.md)。
