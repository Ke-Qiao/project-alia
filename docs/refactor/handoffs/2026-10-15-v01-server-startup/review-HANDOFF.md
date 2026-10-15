# V01 ServerStartup 初始化取消与失败清理 — 独立审查

日期：2026-10-15（Asia/Shanghai）；基线 `4ab48fea79046b9fb80e2bf9dc5c468a463c872f`，上轮源码 `8bcda7a`；分工 `/root/v01_review`。仅独立只读源码、既有交接与证据；只写本页，不运行 Cargo、测试、数据库、SSH、模型或硬件，不提交。原期限、默认并发、政策和 shadow 不变，R03 暂停，无新依赖／平台。

## Checklist

- [x] 复核上一轮 31 份归档身份与四份 HANDOFF 的证明边界。
- [x] 检查 ServerStartup、ShutdownHandle、RuntimeCleanup 与关键调用方的所有权及失败顺序。
- [x] 提出本轮最小有界范围、可区分证据和假阳性风险。
- [x] 交叉审查 Root 确认缺口后的候选 diff 与回归断言。
- [x] 独立核对实际 RED／最终验证、资源与提交身份，完成交接。

当前 5/5；目标 RED、最终候选、八项定向 GREEN、一次完整 workspace、任务资源回收、源码提交及 25 份归档身份均已独立核验。本轮可交付，历史 V01 根因仍开放。

## 上轮证明边界

重新核对 `../2026-10-15-v01-db-cancel/evidence-index.json`：31/31 文件长度与 SHA-256 全部一致。上轮四份 HANDOFF 区分旧实现 RED 0/2、定向 GREEN 7/0、首次完整 V1 1265/1/5 与最终 V2 1266/0/5；最终 duplex 31/0，675 项输入与源码 `8bcda7a` 一致。V2 退出后约 0.175 秒同库资源为 0/0，随后精确回收任务容器／卷；失败批次未计入最终通过总数。

上轮只处理 Delivery 测试夹具初始数据库 constructor 的普通取消、原 SQL 排空、ready 交付及 completion 等待。它没有证明 ServerStartup 所有初始化点、所有调用方失败路径、runtime 强杀、永久数据库失联或生产 supervisor panic 的安全完成。历史 Companion 25 秒根因依然开放。

## 当前生产所有权契约

`ServerBuilder::start_build` 同步建立独立 startup task、child CancellationToken 和 RuntimeCleanup。`ServerStartup::wait(self)` 消耗整个 owner；即使 wait future 未被 poll 就被丢弃，捕获的 self 仍会 Drop 并请求取消，因此不应照搬上一轮借用 `ready(&mut self)` 的未 poll guard 缺口。Drop 不 abort 原 worker，调用者预先取得的 ShutdownHandle 可在 ready waiter 消失后继续等待清理。

`startup_step` 刻意通过 biased cancellation select 丢弃该阶段 future，并捕获阶段 panic 为失败。`RuntimeCleanup::initialize_owned` 在 init 被 poll 前注册包含原 module 的 rollback slot，取消 init 后 stop 仍能取得部分初始化对象；这与上一轮不能随便 drop 在途 SQL 的 fixture 设计不是同一个契约。对模块而言，部分资源必须存入 module 字段并由 stop 负责 join。不能只证明 init future Drop 就推定底层 worker 已停止。

RuntimeCleanup 按 Ingress、Producers、Execution、Delivery、Channels、Resources、Database 排序，同 phase 逆注册顺序。stop 返回 Err 的契约仍要求其 worker 已结束，错误保留后继续依赖清理；stop panic 则明确保留 action 与下游资源，状态 incomplete，不重试、不伪造 finished。ShutdownHandle 的 wait 可取消和重新等待，期限到只返回携带同一 handle 的 ShutdownIncomplete；期限不是资源释放许可。

`ServerRuntime` 的 Drop 请求 cleanup，不能等同 await shutdown 完成。成功 runtime 留在未消费 JoinHandle 输出中随后被 drop 的情况也必须由原 runtime Drop／cleanup handle 证明，不以“ready task 已返回”代替清理完成。

## 初审相邻风险（本轮不扩大）

当前 `tests/server_runtime.rs::with_fixture` 只 catch body 的 45 秒 timeout／panic，随后立即调用 `fixture.cleanup()`。业务 future 内部的 ServerStartup／ServerRuntime Drop 会请求后台 rollback，但外层没有统一保留 ShutdownHandle 并等待它。因此 body 失败后，fixture 可能先关闭数据库并删除 schema／目录，而部分初始化模块仍在 stop；fixture cleanup 的 panic 还可能抢先掩盖原 body panic。

已有 `cancelling_or_dropping_build_future_rolls_back_partial_init` 在正常测试控制流中显式等待 handle，并检查 stop／active／pending；`failed_startup_returns_pending_handle_until_native_rollback_finishes` 检查 grace 到期后回滚仍持有 owned pool。这些是应保留的证明，但都不能覆盖外层 body 自身失败后局部 completion handle 被丢弃的调用方路径。Companion fixture 已在外部 context 保存 startup.shutdown_handle，再 await startup.wait，提供了可复用的局部所有权模式。

建议先用既有 Probe 的 init_gate、native_stop、stop 计数和真实数据库可用性复现这一调用方路径；只修复已确认的测试 owner／夹具边界，不预设需要修改 ServerStartup、RuntimeCleanup 或增加通用生产 supervisor。保留 `DATABASE_CASES=2`、默认 libtest 并发和 body 原 45 秒；cleanup 超过成功期限应保留原失败并继续 drain，不能刷新成功期限。

上述是初审建议。Root 随后明确本轮优先复现生产 ServerStartup 已缓冲成功结果在 parent.cancel 后仍由 wait 交付 Ok 的边界；全局 with_fixture 修复会扩多个入口，因此此处相邻风险仅记录，不作为本轮已证根因或扩大修改范围。本轮新测试必须自行在外部 context 保留原 waiter／ShutdownHandle，保证 fixture DB 不抢先关闭。

## 验证与假阳性约束

1. 通过实际 module init 的 entered 事件确认阶段，再取消／制造 body 原失败。观察窗口不能从 fixture admission、构建身份摘要或整个数据库初始化前开始，避免重演上轮 V1 将排队时间混入五秒观测的错误。
2. 在 native rollback 真正等待期间，保留原 ShutdownHandle，记录 pending、active 和数据库可执行 SELECT；释放原 gate 后等待同一 handle，确认 stop 仅一次、active=0、pending 为空，再允许外层 DB／schema／目录清理。Drop 请求和 watch `is_finished=false` 不能当作完成。
3. 失败注入后的证明必须位于最外层或通过独立 receipt／标志二次确认，避免“原 panic 优先”吞掉 cleanup 内的断言。既要证明原 panic／失败没有丢失，也要证明清理已实际执行；仅检查错误字符串不够。
4. 回滚原错误与清理次生错误分别记录。当前启动错误已进 ShutdownHandle.failures，因此 handle.wait 返回 Err 可能是预期原启动失败；应同时检查 finished、pending、真实 worker／依赖资源，不能把任何 Err 都解释为未清理，也不能把 Err 忽略后宣称成功。
5. 失败观测先于兜底，gate 释放与同一 cleanup wait 要覆盖断言失败路径。若 handle 仍未完成，不先删除数据库／目录来强行结束；watchdog 或 runtime 退出只能记录未证明完成。

合理运行范围是一批针对确认缺口的 RED、修复后必要的定向验证及冻结源码完整 workspace；新增失败才有针对性修订。无需重跑已归档历史 V01 三批或扩张硬件／模型／平台矩阵。

## 暂不扩大

生产 builder 的真实 connect／migration 取消仍使用 startup_step；不能仅据上一轮 fixture 的 SQLx 观察就声称本轮已证明这些所有网络／事务窗口。当前模块 stop panic 的保守永久 pending、host runtime 被强制销毁、所有 completion handle 被丢弃、原 startup worker 在协议外崩溃也不是本轮默认可关闭项。需要明确的新复现与最小独立证据才推进这些边界。

## 已冻结 RED 候选复审

已只读审查 `tests/server_runtime/startup.rs` 和主文件接线，首次 RED 前结论为 GO。

测试沿用 `DATABASE_CASES=2`，复用既有 `case_support::run_context` 的绝对 45 秒与先 context 后 DB 顺序。外部 StartupContext 持有 parent、init gate、ShutdownHandle、原 wait future 和可能已交付的 ServerRuntime。body 只借用这些 owner；取消或原断言失败后仍可继续等待原 future／cleanup。

真实 Probe init entered 后才对原 ServerStartup waiter 手工 poll 一次，该单次 poll 通过 Tokio unconstrained 排除协作预算唤醒冒充任务完成；释放 init gate 后才开启原五秒 JoinHandle 通知观察。此时 waiter 不再次被 poll，所以成功输出仍缓冲在原 JoinHandle。取消前额外记录 worker active、按随机 application_name 识别的 owned pool 与监听器存在。

parent.cancel 后记录 wait 是否返回 Ready。旧实现即使返回 Ok，也先由 owner 保存 runtime，再 shutdown 并 await 原 ShutdownHandle；确认 worker=0、stop=1、owned pool 关闭、fixture 独立 pool 仍可用、HTTP／gRPC 监听释放后，最后才断言拒绝 Ready、错误含 startup cancelled、原 handle 保留取消失败。失败观测不被兜底覆盖，也没有为得到 RED 提前关闭数据库。

首次 RED 前没有源码阻断，预期应明确失败在“cancelled startup delivered a buffered ready runtime”；若更早的资源／阶段断言失败，不能冒充交付缺口已复现。当时要求后续修复保持原初始化失败优先，取消后的 rollback 超过原 grace 则保留 typed ShutdownIncomplete 和同一 handle；最终复审与运行见后文。

## 目标 RED 与 builder 初版复审

已独立读取 `run/validation-20261015-v01-server-startup/startup-red.log/json`：0 passed／1 failed／0 ignored／81 filtered，target 0.61 秒、命令 16.09 秒、exit 101、默认 libtest 并发、source_drift 为空。原始输出明确 `ready_notified=true`、`returned_ready=true`；worker、pool、监听回收断言全部先通过，唯一失败为预期的 `cancelled startup delivered a buffered ready runtime`。因此本轮已确认生产 ready 交付缺口，而非阶段计时或资源清理断言误判。

`after-red-resources.json` 同一任务容器记录 schema=0、other_clients=0，按命令时刻推算在退出后约 0.178 秒取得。这是先保存交付事实、执行兜底后的资源快照，不把旧实现返回 Ready 解释为正确。

已只读审查 builder 初版最小 diff：`wait` 仅在 `Ok(runtime)` 且 child token 已取消时调用 rollback，再 drop runtime 并返回取消错误；原 `Err` 分支保留既有错误，未被后到取消覆盖。私有 `rollback_startup` 原样复用原 record_failure、start、wait_until(deadline) 顺序和错误 context，grace／typed ShutdownIncomplete 路径没有改动。等待 rollback 的同一 future 若被丢弃，仍拥有的 runtime Drop 和独立 RuntimeCleanup 继续请求／执行清理，外部 ShutdownHandle 仍是完成证明。

builder 初版 diff 未见阻断；当时尚待四项最终测试复审，重点是超过原 grace 后不得声称清理完成、必须保留 typed handle，以及次生清理失败不能遮蔽原失败断言。后续结果如下。

## 四项最终候选与八项验证入口

已只读复审冻结的 `builder.rs` 与完整 `server_runtime/startup.rs`，无阻断，GO 进入一次八项定向 GREEN。四项新用例加四项既有 partial init/start、取消 build、pending native rollback、joined stop error，随后冻结源码完整 workspace；不降低默认并发或反复运行同源用例求绿。

局部 `with_startup` 沿用 DATABASE_CASES=2，并通过既有 run_context 保存完整 StartupContext 至清理完成；原 waiter 只在 await 得到结果后才从 slot 移除。context close 先请求取消、放行原 init/native gate，然后等待原 waiter／runtime shutdown／原 ShutdownHandle；没有提前释放数据库。所有资源断言均在该轮真实 close/join 后进行，错误结果不被当作完成凭据。

- 已缓冲成功用例保留原 RED 的实际阶段、JoinHandle 通知及资源存在证据，取消后拒绝 Ready，原 shutdown report 记录取消。
- native rollback 用例从原 fixture 配置读取 3000ms grace，要求 wait 返回 typed ShutdownIncomplete 且 elapsed 已跨原 grace；当时 module pending、worker active、owned pool 和原 handle wait 都仍保留。取消一个 handle wait 不丢 owner，释放原 gate 后重等原 handle 与 typed handle 都完成并保留失败报告。
- 未 poll wait 用例在 module 已初始化但仍阻塞时直接 drop 原 public wait future；先记录 parent 尚未取消，再 await 原 shutdown handle。之后 owner.close 才取消 parent，避免因兜底动作污染 child 不反向取消 parent 的判断。
- 已缓冲失败用例同时制造原 init failure 和已 join 后的 stop error，确认原回滚报告已 finished 后才取消 parent；wait 继续返回原两类错误，报告前后相同，不新增 startup cancelled 来覆盖原失败。

最终生产 diff 仍仅在成功交付前检查已取消 token，并复用原 rollback helper；typed error context 与原 Err 路径保持。测试阶段观测从真实 entered 后开始，手工 unconstrained 只作用于一次 poll，不改变 owner 调度或默认测试并发。静态审查没有发现原 panic 优先吞掉本轮资源断言的假阳性；当时尚待实际运行，后续证据如下。

## 八项定向 GREEN 独立核验

已完整读取 `startup-green.log/json`：四项新回归与四项既有相关回归逐项 ok，8 passed／0 failed／0 ignored／77 filtered，target 4.34 秒、命令 40.38 秒、exit 0、默认 libtest 并发、source_drift 为空。

原始布尔证据对应四项新用例：buffered 成功时 ready_notified=true、returned_ready=false、cleanup_finished=true；native rollback 的 incomplete、unfinished、pending_module、pool_retained、wait_pending、typed_handle_finished 均 true；未 poll waiter 用例 rollback_finished=true 且 parent_cancelled=false；原双失败用例 finished_before_cancel=true、returned_ready=false、failure_report_unchanged=true。四次资源记录均 owned_pool_closed／shared_pool_usable／HTTP released／gRPC released=true，worker=0、stop=1。不是仅根据汇总通过数推断清理。

GREEN manifest SHA-256 `99e269a7aae1f750decfc7777d12f14e976babd56806292b92999c8a8b458c8f`，完整回归冻结 manifest `34595bfb7a942b17a3941474ae8c2c7392caa8c95f4f2e3847fdbf997011f205`；两者 676 项输入逐文件长度／哈希完全相同，差别来自快照元数据。本分工又重算当前 676 项内容，未发现源码漂移。

同一任务 PG 的 `after-green-resources.json` 为 schema=0、other_clients=0；按命令时刻计算，快照位于退出后约 0.181 秒。此为定向退出后的资源证明，不是完整回归最终快照；当时完整 workspace 尚待结果，未提前记为通过。

## 最终完整回归与资源复核

已独立按 `final-workspace.log` 每个顶层 Cargo target 的最后结果重算，67 Running＋5 Doc-tests，共 1270 passed／0 failed／5 ignored／0 filtered；server_runtime 为 85/0/0/0，80.42 秒，duplex 为 31/0/0/0，45.61 秒。完整 workspace 命令 505.0 秒、exit 0、默认测试并发、source_drift 为空，900 秒进程 watchdog 未触发。5 个 ignored 仍是既有 PTY 3 项及 Easy-Turn 2 项，RED 0/1、定向 8/0 与子进程重入均未累加到最终总数。

本分工重新计算当前 676 项输入，全部匹配最终 manifest `34595bfb7a942b17a3941474ae8c2c7392caa8c95f4f2e3847fdbf997011f205`。保留 `final-server-runtime` 二进制为 122971584 字节，SHA-256 `8c71aee1321865e7dfa80df087746712cd5148cdad656948f011eca0069caf6a`，与 summary 一致；未执行该二进制。

`final-resources.json` 的同一任务 PG 为 0 schema／0 other clients，按时刻推算在命令结束后约 0.248486 秒取得，不能称结束当刻。`cleanup.json` 随后先核验完整容器 ID `857ab743c07698eed70640d54a17a83de59a45c883fc2df23a72a17aaff80a33` 和 label `v01-server-startup-20261015`，再成功 stop、rm -v，查询确认任务容器不存在；共享 astrbot 保留运行。回收晚于资源快照，不以删除容器代替清理证明。

执行证据无阻断。本轮只关闭已缓冲 ServerStartup 成功结果的取消交付缺口及选定回归边界，不关闭历史 V01 根因或未复现的早期 connect／SQL 确认／discovery 等路径。

## 提交与归档身份：最终 5/5

源码提交 `9e3a93401aea481f729b21512431b980dfc3b02c` 仅改 builder、server_runtime 接线及本轮 startup 回归三个文件。本分工使用 `git cat-file --batch` 独立读取提交中全部 676 项输入 blob，与最终冻结 manifest 逐项长度／SHA 比较，676/676 一致；`commit-verification.json` 的无 extra／missing／mismatch 记录与核验一致。

`evidence-index.json` 的 25/25 归档文件长度与 SHA-256 全部一致；又按原 run 目录逐份比较字节，25/25 与原件一致。Root HANDOFF、README、恢复检查点的 RED 0/1、GREEN 8/0、完整 1270/0/5、676 输入、提交号及资源时刻均与原件一致，未混算批次，未将相邻静态风险写成本轮已证根因。

最终独立意见：本轮已确认的 ServerStartup 缓冲成功交付取消缺口可交付；原失败、grace／typed incomplete、重复等待、资源清理和政策边界均有相应证据。默认 shadow、R03 暂停、无新依赖／硬件／平台范围保持。本分工没有运行测试、数据库或硬件，没有提交源码；现完成 5/5 并停止写入。
