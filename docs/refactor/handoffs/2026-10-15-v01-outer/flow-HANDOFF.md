# V01 外层超时与清理所有权审计 — 2026-10-15

分工 `/root/v01_flow`；只读审计起点 `99c946d`。本轮沿用原 25 秒业务期限、45 秒 fixture 期限、默认 libtest 并发和 `CASES=3`；默认 shadow、当前政策、精确取消和 R03 暂停保持。不运行真实硬件、模型、Cargo、PG 或 SSH；运行与提交由 Root 负责。

## 确认的边界

上一轮已完成内层失败摘要与三批有界配对验证，但没有复现历史 V3 超时。当前调查针对外层 drop 与失败清理，不能把发现的 fixture 缺口倒推为旧 V3 的实际原因。

`delivery_support::with_db` 的 45 秒包住整个 `test(f.clone())`，包括 paired 初始化、ServerBuilder、内层 25 秒及 case 自己的清理。到期时该 future 被丢弃，然后 helper 立即 `f.cleanup()`，依次关闭工作 pool、DROP schema、关闭 admin pool；原 fixture 的数据库清理本来就在 45 秒之外。许可等待、`Fixture::new()` 的迁移与初始化也不在这个 45 秒内。因此它从来不是完整生命周期的总上限，不能给 cleanup 添一个能强行释放仍被使用资源的“总期限”。

生产 `ServerRuntime::Drop` 已请求取消并启动独立 `RuntimeCleanup` owner；生产 `ShutdownHandle` 可在调用者放弃等待后继续观察实际结束。`ServerBuilder::start_build()` 也公开 `ServerStartup::shutdown_handle()`，再由 `wait()` 完成交接或 rollback。这些现有机制可以直接使用，不需要修改生产生命周期或新建框架。

## 已确认缺口与归属

| 缺口 | 代码事实与影响 | 归属 |
| --- | --- | --- |
| 外层先丢弃资源再回收数据库 | 原 `with_db` 仅持有 Fixture，case 内部的 ServerRuntime / paired Runtime 不能在 timeout 后显式 join；ServerRuntime drop 虽启动清理，但 fixture 未等待它。数据库 close / schema drop 可与清理并行。 | 测试所有权缺口；生产已保留独立 cleanup owner。 |
| RPC close 丢失 join | 原 `RpcServer::close` 把 `self.task.take()` 按值交给 3 秒 timeout；到期或 close future 被丢弃后，`RpcServer::Drop` 已没有 handle，不能 abort，原 JoinHandle drop 仅 detach。 | 测试 RPC guard 缺口。 |
| heartbeat 没有外层 owner | 原裸 `Option<JoinHandle>` 在 case future drop 时丢失；正常 cleanup 只在 `stop.cancel()` 后 await，但两处 `tx.send(...).await` 不感知取消，接收方保留而不消费时取消不能打断发送。 | 测试合成 capture worker；不是生产录音。 |
| 清理失败提前跳出 | heartbeat join、session revoke、channel stop、RPC close 等 unwrap 或 assert 可以跳过后续清理，并覆盖原业务 panic。原数据库 cleanup 也在第一项错误时跳过后两项。 | 测试失败传播。 |
| 部分初始化未进入 owner | paired `Runtime::new` 创建 listener guard 后还会 await channel init / Delivery start；仅在 `new().await` 成功后把 Runtime 存外层 Option，不足以覆盖中途 drop。 | 测试构建过程。 |

Tonic listener 的 abort 不是所有连接和 Playback worker 已 join 的证明。当前 paired cleanup 应继续由 PlaybackSession / Delivery 的现有 stop 方法完成真实 owner 清理，再回收 channel、listener 与目录。生产的 `shutdown_pending`、`ShutdownIncomplete` 和 `ShutdownHandle` 不能被测试代码解释为“已完成”。

## 最小实现建议与当前分工

Root 选择保留现有 `with_db` 接口，新增测试专用 `with_db_context(context, test, cleanup)` 变体：context 的所有权留在 timeout 外，body 仅借 `&mut Context`，但 **body 与 context cleanup 共用同一个原 45 秒绝对截止**。body 到期或 panic 后，先从仍在外层的 context 收拢资源，再做数据库清理，最后恢复原业务结果。cleanup 跨越原截止时，case 仍失败；同一个 pinned cleanup future 继续被排空，不被 drop 或重新构造。数据库 cleanup 仍在原 45 秒之外。使用现有 `futures::future::LocalBoxFuture`，不增加 `Send` / `'static` 限制或依赖。

Companion 局部 context 负责其 `Option` owners 和阶段，Root 实现；本分工获授权独占 `delivery_support/mod.rs` 和纯计时回归文件。生产已有 startup handle 应在 `startup.wait().await` 前存入 context；部分 paired 初始化的 listener / directory / service 也应在首次可能挂起的位置前有外部 owner。只把最终 Runtime 存 Option 不够。

cleanup callback 必须在内部按依赖顺序收集错误，保留尚未结束的原 join / ShutdownHandle，不能在第一个 unwrap 处跳出。generic helper 的 catch 只能保留错误并尝试后续数据库清理，不能替调用者证明一个提前 panic 的 cleanup 已清干净所有生产 owner。不能为了满足 45 秒而丢弃原 worker、刷新机会或跳过清理。

当前 helper 已提供原 45 秒的 `run_context`、原失败优先的 `finish_case`，以及不改变原入口约束的 `with_db_context`。数据库三阶段改为先分别尝试再统一报告固定阶段错误。初版把 context cleanup 完全移出成功判定窗口，Root 审计发现这会允许 body 44 秒加 cleanup 2 秒仍通过；最终实现已改用上述共同绝对截止，并补两项回归防止该缺口。共六项纯计时 / panic 回归，两个负责文件的 rustfmt 与 diff whitespace 检查通过；本分工没有自行编译或运行，已独立核对 Root 的最终完整验证结果。

## 确定性复现与验证设计

1. 用现有 Tokio `test-util` 暂停时间，让 body 持有 Drop sentinel 后永久 pending。驱动原 **45 秒**，要求 body 先 drop、仍保留的 context cleanup 其次、模拟 DB cleanup 最后；最终仍报告原 45 秒失败。此测试验证 fixture 所有权顺序，不需要 PG 或真实耗时。
2. body 产生可辨认的原 panic，context cleanup 再 panic，数据库 cleanup 仍应被调用；最终恢复原 panic payload。另覆盖 body 成功但 cleanup 失败、仅数据库 cleanup 失败，避免把清理错误变成成功。
   另以 body 44 秒、cleanup 2 秒证明第 45 秒即失败，第 46 秒排空同一个 cleanup 后才数据库清理；cleanup 仅构造并执行一次。原 body panic 配合同样跨截止 cleanup 时，仍优先恢复原 panic payload。
3. RPC RED 使用 pending task 与 Drop sentinel，驱动原 **3 秒** close；旧代码会在报告超时后留下未 join task。修复必须保留失败结果，同时证明 abort 后 await 到原 task 实际结束。Root 统一运行这一证据。
4. heartbeat 的 mpsc 队列填满并保留 receiver 后取消 token，要求发送方退出；只取消 token 而不接入 send 等待的旧写法会稳定挂起。该用例只涉及合成帧和进程内队列。

不混用 paused time 与真实 PG 的时钟事实；无数据库的测试只证明控制流与所有权，真实配对 / PG 验证仍由 Root 在当前原期限与并发下运行。

## Checklist

- [x] 检查原外层 45 秒、内层 25 / 32 秒覆盖范围及数据库 cleanup。
- [x] 区分生产独立清理 owner 与 fixture 丢失 join 的行为。
- [x] 定位 RPC、heartbeat、初始化和错误覆盖的实际代码分支。
- [x] 提出可复用现有 startup handle 的最小 context 方案。
- [x] 提交原时间与并发不变的确定性复现设计。
- [x] helper 和六项纯计时 / panic 回归实现完成，保留共同原 45 秒截止，格式化后交 Root 验证。
- [x] 根据 Root RED / GREEN、最终源码身份和资源清理结果收口。

当前分工 **7/7**。历史 V01 根因仍未复现；本轮新增测试 fixture 结论不能扩写为真实 Linux / 硬件或线上稳定性验证。

## 最终证据独立核对

本分工只读复核 `run/validation-20261015-v01-outer/final-v2-workspace.log`、同名 JSON、`validation-summary.json` 和 `final-v2-source-manifest.json`，未重跑测试。最终 `cargo test --workspace --offline --locked --no-fail-fast` 在原默认测试并发下退出 0，命令历时 **347.91 秒**，总计 **1256 passed / 0 failed / 5 既有 ignored**；duplex target **21 / 0 / 0**，其中以下六项 helper 回归在原始日志中逐一为 `ok`：

- `outer_deadline_drops_body_then_cleans_context_before_database`
- `cleanup_crossing_original_deadline_fails_after_draining_once`
- `primary_panic_survives_cleanup_crossing_original_deadline`
- `primary_panic_survives_both_cleanup_failures`
- `cleanup_panic_still_attempts_database_cleanup_and_fails_case`
- `database_cleanup_panic_fails_otherwise_successful_case`

最终 manifest SHA-256 为 `021b2fb3d46d64ddd98c2344cb078647b66bc143dc1f41d4b5d1326e19b6a6d2`；本分工重新读取当前全部 **671** 个输入的字节数及 SHA-256，全部一致。负责的 `delivery_support/mod.rs` 为 12585 字节、SHA-256 `c947c749be974be7c5e443f4b952d9bb6b5f75053f420da79f0269e5ea2fd567`；`db_cleanup.rs` 为 6278 字节、SHA-256 `4ffda8c863cf78da63004e476703e9275ba4147a191a42716b04d145b757d4dd`。

`final-resources.json` 记录 **0 个测试 schema、0 个其他 client**，与 `database-create.json` 的 container ID 一致；采样在命令结束后约 56 秒，不能把它描述成进程退出瞬间的资源快照。原始 duplex 日志同时记录真实 PG 外层 45 秒回归和 heartbeat 清理失败回归通过；它们由 Root 执行，未用 paused time 代替数据库实际等待。

剩余边界：纯 helper 回归证明控制流与错误优先级，不证明所有资源类的任意 panic 都可恢复；任意提前 panic 的 cleanup callback 仍须由调用方自己完成所有权收拢。Fixture 初始化仍在原 45 秒之外，尚未穷举全部部分初始化、poisoned native lock 或 supervisor panic。保留原 owner 继续等待可能超过失败判定期限，进程 watchdog 不是 join 或数据库可回收的证明。当前工作修复测试夹具已确认缺口；历史 V01 保持 open，真实 Linux / Pi / 模型 / 硬件未验证。
