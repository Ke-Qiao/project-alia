# V01 ServerStartup：取消与失败复现设计

日期：2026-10-15。基线：`4ab48fe`，上一轮源码提交 `8bcda7a`。分工：`v01_flow`。本分工已完成：RED 实际确认、首次定向 GREEN 与一次最终完整 workspace 全部核对。源码提交身份及容器最终清理由 Root 统一收口回填。本分工未运行测试或数据库。

## 已读范围与已有保证

已读 `bootstrap/builder.rs` 的 `ServerStartup`、`ServerBuilder::start_build/build_owned`，`bootstrap/runtime.rs` 的 wiring／runtime 清理，`modules/platform/lifecycle.rs` 的 `RuntimeCleanup`／`ShutdownHandle`／`startup_step`，现有 `tests/server_runtime.rs`、`tests/server_background.rs`，以及 paired `duplex_playback/companion.rs` 初始化与最终清理。

现有 `ServerStartup::wait(self)` 持有整个 startup owner；即使返回的 future 从未 poll，drop 仍会销毁其中的 owner 并请求取消。这与上一轮借用 `&mut self` 的 fixture ready waiter 不同，不应套用无关修复。`RuntimeCleanup::initialize_owned` 在 init 首次 poll 前注册 rollback slot，部分初始化模块及其 stop 所有权已有明确位置。

现有覆盖包括：

- `partial_init_and_start_failures_stop_workers_once_in_reverse_order`：init／start 返回错误，模块各 stop 一次、逆序清理，外部注入 pool 保持可用。
- `cancelling_or_dropping_build_future_rolls_back_partial_init`：实际达到受控 init gate 后，分别取消父 token、abort 外层 ready waiter，随后等待原 shutdown handle，确认 worker 退出且父 token 不被子取消污染。
- `failed_startup_returns_pending_handle_until_native_rollback_finishes`：启动失败且 native rollback 超过既有 grace，返回 `ShutdownIncomplete`，保留原 handle 与 owned pool，实际释放后失败仍可观察。
- `native_stop_deadline_keeps_owner_and_pool_until_actual_completion`、`joined_stop_error_does_not_skip_remaining_cleanup_or_become_success`：停止等待越界仍保留 owner；已 join 的 stop 错误不跳过后续资源清理。
- paired `CompanionFixture` 在调用 startup.wait 前保存 shutdown handle；其最终清理无论是否收到 runtime，都等待同一 shutdown handle 完成后才进行 SQL teardown。

这些是已有源码与测试设计覆盖；本分工本轮尚未重新运行它们。上一轮全量通过不能冒充本轮新边界已测试。

## 已确认缺口与确定性 RED

基线 `ServerStartup::wait` 在原 JoinHandle 返回 `Ok(ServerRuntime)` 后直接将 `handed_off` 设为 true，没有复验取消 token。实际 RED 已确认以下顺序：独立 owner 已成功完成、runtime 暂存在 JoinHandle 输出中；外部父 token 随后取消；等待方继续 poll，仍收到成功 runtime。

最小测试可复用现有 `server_runtime` helper、Probe 与 init gate，不加生产 checkpoint：

1. 保留默认 `DATABASE_CASES=2` 与原 45 秒 body 预算。创建带 `ProbeState::init_gate` 的 builder，保留父 token 和原 shutdown handle。
2. 先独立等 Probe 的真实 initialized 信号，原 `startup.wait()` future 由外层 context 保存且尚未 poll。准入与前置初始化不计入 ready 观察窗口；不提前以 5 秒猜测阶段是否发生。
3. 对同一个 `startup.wait()` future 仅手动 poll 一次，使用标准库 `Wake`＋Tokio `Notify` 注册 JoinHandle 唤醒，记录 pending，然后放行 gate。只有本次借用 future 的手动 poll 使用 `tokio::task::unconstrained`，防止 Tokio cooperative budget 自唤醒被误当作任务完成；独立启动 owner 的调度和默认并发不变。
4. 放行后在既有 5 秒观察上限内等待原 JoinHandle 完成唤醒，不再次 poll 消费成功输出。
5. 调用父 token cancel，再 poll 同一个 wait future。旧实现预计返回成功 runtime；先记录 `returned_ready`，不得立刻断言而遗漏清理。
6. 若旧实现返回 runtime，先显式 shutdown 该 runtime；无论返回成功或失败，都等待保存的原 shutdown handle。记录 worker 退出、stop 次数、owned pool 关闭或 borrowed pool 保持可用，再执行最终拒绝成功交付断言。

新实现应在取消已发生时拒绝 ready 交付，并使取消失败通过原 shutdown handle 可观察。不得重新创建 cleanup owner、丢弃原 JoinHandle、用更长期限或更低并发让测试通过。

Root 授权后，本分工先独立实现 `packages/alia-server/tests/server_runtime/startup.rs` 中唯一一项 `cancelled_buffered_startup_rejects_ready_and_rolls_back`。测试使用 from_config owned pool，通过独立 application_name 查询其连接退出；fixture 的独立 pool 必须仍可 `SELECT 1`；原 Probe worker 活跃数归零且 stop 恰好一次；HTTP／gRPC 已绑定端口在清理后可重新绑定。原实现成功交付的布尔证据先保存，全部清理与资源观察完成后才进行 RED 拒绝交付断言。

本分工只读核对 `run/validation-20261015-v01-server-startup/startup-red.{json,log}`：0 通过／1 失败／0 ignored／81 filtered；目标用时 0.61 秒、命令 16.09 秒，默认并发、`source_drift=[]`。日志显示 `ready_notified=true`、`returned_ready=true`；cleanup finished、无 pending、worker 归零、stop 一次、owned pool 与 HTTP 释放均成立，最终仅“取消后交付 buffered ready runtime”断言失败。`after-red-resources.json` 显示同一任务 PostgreSQL 为 0 schema／0 其他连接。

端口采用本地临时预留后释放，再交 builder 绑定。该预留交接存在 OS 端口争用窗口；若发生实际冲突，作为新失败记录，不延长期限或同源重跑掩盖。

## RED 后授权补齐的三项守卫

除原 RED 回归外，Root 已授权并实现以下三项，当前共四项新回归，均已在首次定向验证通过：

- `buffered_startup_cancellation_retains_incomplete_native_rollback`：复用原 Probe 的 native stop gate及 Fixture 既有 3000 毫秒 grace；buffered 成功被取消后必须返回 typed `ShutdownIncomplete`。原 handle 仍 pending、pool 与 worker仍保留；手动 poll 后取消一个 handle waiter，外层 context 放行同一 native gate并重等原 handle，typed handle与原 handle都最终完成且取消失败仍在报告中。未改原 grace。
- `dropping_unpolled_startup_wait_rolls_back_partial_initialization`：等到实际 partial init后，刻意 drop 从未 poll 的 public startup.wait future；原 shutdown handle留在外层 context并等待完成，验证子取消不污染父 token，worker／owned pool／listener 清理。
- `buffered_startup_failures_survive_later_parent_cancellation`：Probe 注入 init 错误与 worker join后的 stop 错误；确认原 JoinHandle 完成通知后再 parent.cancel。返回结果必须继续包含原 init＋stop 错误，取消前后的原 handle失败列表不变，不能被后到取消覆盖。

上述三项验证组合边界与原保证，不另行宣称发现三项生产缺陷。共用代码仅为局部阶段同步、资源观察和外层 context托管，未修改原 Probe、Fixture 或生产 hook。所有场景先实际清理并收集资源证据，再进行最终断言；native gate存放在 context中，close先 release再 await原 waiter／handle，body超时也不会把停止线程留在受控 gate后。

## 有界与兜底约束

所有“已经进入某阶段”的断言必须来自显式 semaphore／oneshot 或原 JoinHandle 的实际唤醒，不用固定 sleep 猜测。短观察计时只在对应真实阶段确认后开始，避免上一轮将 CASES 排队计入 ready 窗口的错误。

失败先保留观察值；先 release 受控 gate、等待同一 startup／cleanup owner、关闭测试独立 observer，再断言。若尚未获得资源身份，不能直接 unwrap 跳过 observer close，也不能用广泛 DROP 清理其他测试数据。

新测试遵照 Root 分配，在局部引用现有 `delivery_support::run_context`，仍获取原 `DATABASE_CASES=2` 并调用现有 `Fixture::new`。外层 context 持有 parent、init gate、shutdown handle、原 startup.wait future 与可能返回的 runtime；body 只借用原 waiter。原 wait 真正返回之后才在同一同步段 take，不能先 take 再 await。body 超时或 panic 后，context 先取消 parent、放行 gate、等待原 startup waiter、shutdown 已返回 runtime并等待同一 handle，随后才调用原 Fixture.cleanup。复用原 45 秒 body＋context cleanup 同一绝对 deadline，超限仍保留清理 future 排空并报告失败；没有改全局 helper 或其他调用者。

真实 `from_config` connect 在途、数据库操作未知确认、进程／runtime 强制终止尚未被本轮证明。migrations 已使用事务与 advisory lock，不能只见 `startup_step` 取消就推定 schema 泄漏。优先验证最小可区分 ready 交付边界，不擅自扩张数据库／平台故障注入范围。

默认 shadow、当前政策、原期限和默认并发保持；R03 继续暂停，无新增依赖、平台或硬件动作。本轮结论不归因为历史 Companion V3 超时根因。

## 首次定向 GREEN 与最终状态

本分工只读核对 `run/validation-20261015-v01-server-startup/startup-green.{json,log}`：四项新回归与四项既有生命周期测试共 8 通过／0 失败／0 ignored／77 filtered，目标 4.34 秒、命令 40.38 秒、退出码 0、默认测试并发、`source_drift=[]`。

四项新用例资源证据均为 owned pool关闭、fixture独立pool可用、HTTP／gRPC释放、worker为 0、stop次数为 1。buffered成功取消为 `returned_ready=false`；native rollback实测保留 typed incomplete、原 pending module、pool及 pending waiter，最后 typed handle完成；未 poll取消没有取消parent；buffered原失败在后到cancel后报告不变。

`after-green-resources.json` 显示同一任务 PostgreSQL 为 0 schema／0 其他连接。最终冻结清单含 676 项输入，本分工重算测试和 builder 的内容摘要均匹配。

## 最终完整验证

本分工只读核对 `final-workspace.{json,log}`、`validation-summary.json` 和 `final-resources.json`，并按顶层 Cargo target 的最终结果独立重新汇总：**1270 通过／0 失败／5 既有 ignored／0 filtered**；`server_runtime` **85 通过／0 失败／0 ignored／0 filtered**。本分工四项新回归均逐项显示 `ok`。内部子进程测试输出没有重复计入顶层总数。

完整命令为一次 `cargo test --workspace --offline --locked --no-fail-fast`，耗时 **505.00 秒**、退出码 0、默认测试并发、`source_drift=[]`。最终清单 SHA-256 为 `34595bfb7a942b17a3941474ae8c2c7392caa8c95f4f2e3847fdbf997011f205`，676 项输入；本分工在完整验证后再次重算 builder 与 startup.rs 字节，仍匹配该清单。

同一 PostgreSQL 在命令退出约 **0.248 秒**后记录 **0 schema／0 其他连接**。Root已补齐提交身份、逐blob验证及任务容器回收收据，见文末；本分工没有自行提交或操作容器。未出现新增失败或源码修订，本轮没有追加同源重跑求绿。

## Checklist

- [x] 读取 ServerStartup、生命周期清理和已有 bootstrap／paired 测试。
- [x] 区分已有部分初始化保证与 buffered ready 候选缺口。
- [x] 提出基于真实阶段与原 JoinHandle 的最小 RED 和取证后兜底顺序。
- [x] 记录观测起点、原期限／并发及未覆盖范围。
- [x] Root 确认方案并分配测试源码文件。
- [x] 实现／核对 RED、修复与最终验证证据，更新进度。

当前分工进度：设计审计 4／4；实施与证据 2／2，合计 6／6。RED、首次定向 GREEN、最终全量及数据库资源退出均已独立核对。源码保持冻结；提交身份与基础设施清理已由 Root 在文末统一收口。

## Root 最终身份与资源补录

源码提交 `9e3a93401aea481f729b21512431b980dfc3b02c`；[commit-verification](evidence/commit-verification.json)确认676项冻结输入逐blob一致，无额外/缺失输入。任务容器`857ab743c07698eed70640d54a17a83de59a45c883fc2df23a72a17aaff80a33`按label核验后stop/rm -v且不存在，共享服务保留，见[cleanup](evidence/cleanup.json)。本分工无剩余执行工作，历史V01与未验证边界按上述记录保持。
