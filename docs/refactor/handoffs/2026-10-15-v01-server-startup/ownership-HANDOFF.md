# V01 ServerStartup 初始化取消 — 所有权审计

日期：2026-10-15（Asia/Shanghai）；基线 `4ab48fe`，上轮源码 `8bcda7a`；分工 `/root/v01_fixture`。本分工先只读上轮恢复检查点、四份 HANDOFF 和实际调用链；Root 确认 RED 后授权实施最小生产修复，见文末。Root 统一执行验证和提交，本分工未运行 Cargo / PostgreSQL / SSH / 硬件。当前默认 shadow、政策、权限、原期限与并发不变，R03 暂停，不新增依赖、平台或硬件动作。

## Checklist

- [x] 阅读上轮检查点与四份 HANDOFF，区分已证 fixture 取消与本轮生产 ServerStartup 边界。
- [x] 审计 ServerStartup、ServerBuilder、RuntimeWiring、ServerRuntime、RuntimeCleanup / ShutdownHandle 及调用方。
- [x] 核对各资源在 await 前后的接管位置，区分已有保证、确认候选和尚未动态证明的风险。
- [x] 与 Root / flow 协调最小可复用方案及确定性复现条件。
- [x] 收到确认 RED 后按 Root 授权实施，并只读核对定向结果。
- [x] 核对最终冻结源码、完整回归和资源证据，完成交接。

当前分工 **6/6，真实 RED、授权实现、定向 GREEN 与完整 workspace 已核对**；源码提交身份和任务容器回收收据已由 Root 在文末补齐。生产修复在确认 RED 之后进行，没有预先修复后冒称复现。

## 上轮证据及本轮范围

上轮只修改 Delivery 测试 fixture：独立 owner 保留原数据库 future、许可与 Fixture，ready waiter 取消后显式等待同一个 completion；V1 测试观测失败和 V2 成功分别保留。最终完整 workspace 1266/0/5、675 项输入与提交一致，任务 PostgreSQL 和卷已回收。不能将这些测试内部保证直接套用于生产 ServerStartup，更不代表历史 Companion 25 秒超时根因已经确定。

本轮审计入口是 `bootstrap/builder.rs` 中 `ServerBuilder::start_build / build / build_owned` 和 `ServerStartup::wait / Drop`，后续接 `RuntimeCleanup`、`RuntimeWiring` 和 `ServerRuntime`。生产已有独立 startup task 与可保留 ShutdownHandle，不应为形式一致而照搬上轮测试 owner 或新建全局 registry。

## 已有资源接管路径

| 阶段 | 资源位置与 await 边界 | 结论 |
| --- | --- | --- |
| `start_build`（builder.rs:199） | 同步生成 child token、RuntimeCleanup，并 spawn 原 build task；返回 ServerStartup。Drop 只取消 child，不取消调用方 parent | 已有独立 owner；不需要另建 startup task 框架 |
| `wait(self)`（builder.rs:90） | 按值持有整个 ServerStartup 并借用其 JoinHandle await；wait future 即使未 poll 就被 drop，也会 drop 捕获的 ServerStartup | 与上轮 `ready(&mut self)` 不同；无需再次增加未 poll guard 修复 |
| HTTP / gRPC bind（runtime.rs:63 起） | gRPC 绑定 await 时，HTTP listener 仍为局部值；取消会同步 drop socket；两端绑定后，先注册 runtime_ingress，再准备 server keypair | 无已启动 listener 任务被遗失的证据；身份文件为持久配置，不当作临时资源删除 |
| 自有 pool（builder.rs:250 起） | `from_config` 才连接自己的 pool，返回后在下一 await 前注册 Database phase close；注入 pool 不注册关闭 | 已返回的 owned pool 有明确清理；borrowed pool 应继续保持由调用方所有 |
| 模块 init（lifecycle.rs:323 起） | `initialize_owned` 先把 Box 放在 rollback Slot 并注册 stop，再 await init；成功把 Box 移为 Arc 的过程中没有 await | 部分初始化和 init panic / cancel 已有 owner；沿用原 stop 合约 |
| channel 移交（builder.rs:1085 起） | 先 initialize_owned，再注册 channel；注册返回成功至 transferred.store 没有 await | 保留原 rollback / registry 接管，不添加重复 stop |
| expression / execution / delivery / companion / sensors | expression 在 attach_db 前注册；ExecutionService::new 只异步读取 build digest，尚不启动 worker；各后台模块在 start 前注册清理 | 没有仅凭 async 构造就成立的 worker 遗失缺口 |
| interaction / listeners | interaction 和 Services 均先获取保存句柄的 slot，再 spawn 并同步存入；cleanup 捕获原 slot | 取消锁等待不会丢失尚未保存的已启动任务 |
| shutdown | RuntimeCleanup 固定 phase 排序；完成的 stop Err 继续后续清理并保留失败；stop panic 保留 action 和下游资源且维持 Incomplete | 不把 pending / panic 改成“已完成”，不延长或刷新原期限 |

`ShutdownHandle::wait` 观察同一个 cleanup owner 的 watch 状态；waiter 被取消不取消原 cleanup。`wait_until` 超时返回带同一 handle 的 ShutdownIncomplete。既有 native-stop 回归已证明超时期间保留 module / pool，释放 gate 后继续等待并保留原失败，不需要为本轮重新设计。

## 初始审计候选：成功结果已就绪后仍可交付取消的 runtime

基线 `ServerStartup::wait` 在取得原 task 结果后直接 `task.take()`，并把 `handed_off` 设为 `result.is_ok()`，没有核对 child cancellation token。若后台 task 已完成并把 `Ok(ServerRuntime)` 保存在 JoinHandle 中，但调用方尚未消费结果，此时 parent cancel 会取消 child，却不会再经过 `start_build` 内部已经结束的 `startup_step`。随后 poll 原 wait 会返回这个已经被取消的 runtime，并标记 handed_off=true。

这是一个具体交付检查缺口；它不等于已证明资源永久泄漏。若调用方随后显式 shutdown 或 drop runtime，现有 Runtime Drop 会启动清理。问题是取消在 ready 交付前已经可见时仍报告 ready 成功，失败来源没有进入原 cleanup 报告。本分工初始审计时向 Root 和 flow 给出位置与确定性复现方案；后续真实 RED 见文末。

原 startup 构造仍未成功时，外层 biased `startup_step` 会优先响应取消并启动 rollback；已有 `cancelling_or_dropping_build_future_rolls_back_partial_init` 覆盖该分支。不能用它代替“task 已成功、结果尚未消费”的不同边界。

## 审计阶段的最小修复方向

只在 `wait` 取得 **Ok(runtime)**、正式交付之前检查取消。如果已经取消：先在原 RuntimeCleanup 记录固定 startup cancellation 失败，保留该 runtime 的原清理 owner，启动并等待原 cleanup；沿用现有 shutdown grace 和 ShutdownIncomplete 返回方式，绝不把 grace 到期解释为完成。成功消费或 drop runtime 必须只有一个清理 owner；正常路径原样交付，原 Err 路径保持其原错误，不让后到取消覆盖它。

可复用 `start_build` 已有的“record_failure → owner.start → wait_until(deadline)”错误收尾；若抽取极小内部 helper，应仅统一这两条完全相同的失败出口，不改变 RuntimeCleanup / ShutdownHandle 公共契约或所有模块初始化模型。`wait(self)` 的所有权已足以覆盖未 poll / 等待期间被丢弃，不能为了修复上述交付检查而引入第二套 ack、共享 ready 队列或全局 supervisor。

取消与返回在多线程中可以继续并发；该检查的承诺是交付时已经观察到的取消不会被返回为成功，不宣称未来取消能撤回已经交付的 runtime。

## 可复用的确定性证明

flow 提出的方案可复用 `server_runtime.rs` 现有 Probe、init_gate、ShutdownHandle 和外层 fixture：

1. 保持既有 DATABASE_CASES=2 与测试 45 秒，不改 Delivery CASES=3 或默认 libtest 并发。
2. 原 Probe init gate 确认初始化到达；手动 poll `startup.wait()` 一次注册自定义 waker，再释放 gate。原 5 秒只从真实阶段放行后开始观察 task completion 唤醒，不覆盖 admission / 初始建库等待。
3. 收到原 JoinHandle 的完成唤醒后不消费输出，先 cancel parent，再 poll 同一个 wait。旧实现应返回 Ok(runtime)，先保存该事实；无论结果如何，都保留原 ShutdownHandle 并完成资源兜底后再做外层断言。
4. 修复后要求 ready 为明确取消失败、原 ShutdownHandle 可等待并保留取消报告；probe worker / stop 次数、listener、owned 或 borrowed pool 按实际 fixture 的所有权核对，不能只看 cancel flag。若 cleanup 超时，继续等待原 handle，原失败不变成功。

已有 init / start 失败、native rollback pending、原 handle 重等和 borrowed pool 保留用例可作为回归覆盖，不必为每种返回值添加同构测试。Root 统一决定具体目标和一次完整回归；本分工不运行测试。

## 尚未动态证明的其他窗口

- **from_config connect 返回之前**：外层 startup_step 包住整个 build_owned，内层连接也用 select；取消可能丢弃尚未返回 DatabaseConnection 的 connect future。cleanup.add(pool) 尚未执行，原 ShutdownHandle 仅证明已登记资源的清理。SQLx 内部 pool / socket Drop 的真实行为不能自动当作明确 join，也不能仅凭这一静态窗口宣称实际连接泄漏。需要单独可观察的连接门和复现，不能直接扩张为重写整个 build 链。
- **数据库在途 SQL**：已登记 pool 关闭不等于收到任意已发 SQL 的业务最终结果。初始化政策 / 迁移的持久副作用是否允许保留是另一个契约；本轮不能未经确认把它整体回滚或改变政策。
- **discovery 注册失败后的局部清理**：`ServerDiscoveryAdvertisement::start` 成功路径在同步注册后返回，再由 assembled 注册 owner；失败路径先 await 本地 advertisement.shutdown。如果该 await 被外层取消，Drop 会请求本地清理，但 RuntimeCleanup 尚未持有它的 join。这里有局部完成凭据缺口的静态风险，尚无本轮动态失败，也不启用真实 mDNS 来扩大现场范围。
- **所有 handle 丢弃 / runtime 强制销毁 / cleanup panic / 永不返回的外部操作**：继续保留未证明完成的状态，不能从 Drop 或 watchdog 推导安全结束。

这些窗口交给 Root 明确记录范围，不作为本轮预先改动理由。历史 V01 Companion 25 秒根因继续 open；R02 硬件 AEC 资料门槛、R03 暂停与 R05 现场出口保持。

## 真实 RED 与授权实现

只读核对 `run/validation-20261015-v01-server-startup/startup-red.log/json`：Root 运行 `server_runtime` 中的 `startup::cancelled_buffered_startup_rejects_ready_and_rolls_back`，结果 **0 passed / 1 failed / 0 ignored / 81 filtered**，exit101，target **0.61 秒**、命令 **16.09 秒**。默认测试并发，源清单 SHA-256 `1212dd000134d14f998c3547333e6b8ae952e9649864055948c264c921b67b76`，`source_drift=[]`。

原始日志明确记录 `ready_notified=true`、`returned_ready=true`；同时 cleanup_finished、pending_empty、owned_pool_closed、http_released 均 true，workers_left=0、stops=1。测试先完成旧实现返回 runtime 的兜底回收与资源断言，最后才在“cancelled startup delivered a buffered ready runtime”失败。因此 RED 证明交付契约错误，没有把兜底资源回收伪称实现正常拒绝 ready。`after-red-resources.json` 记录同一任务容器 `857ab743c07698eed70640d54a17a83de59a45c883fc2df23a72a17aaff80a33` 为 0 schema / 0 其他 client；本分工未连接数据库。

Root 据此授权仅修改 `packages/alia-server/src/bootstrap/builder.rs`。实现已完成：

- 新增私有 `rollback_startup`，逐字保持原 `record_failure → start → wait_until(deadline)` 和 error context 规则；原 `start_build` Err 分支改为调用它，没有改变原错误处理行为。
- `ServerStartup::wait` 消费原 task 结果后，仅当 `Ok(runtime)` 且 token 已取消时，记录 `server startup cancelled before ready delivery` 并使用同一个 helper 回滚。runtime 保留跨越原 grace 等待，随后 drop；若等待本身被取消，runtime Drop 与已启动 RuntimeCleanup 仍会继续管理原资源。
- 原 `Err` 分支不进入新取消分支，仍保留原 init / stop 失败；正常 ready 路径保持原 handed_off 语义。typed ShutdownIncomplete 继续通过原 anyhow context 保留，可以下转为原类型并取得原 handle。

只对授权 builder 文件执行 `rustfmt --config skip_children=true` 和 diff whitespace 检查，未格式化其它文件；源码已通知 Root / review 冻结。flow 补充未 poll wait、原失败保留与真实 native rollback pending 的回归，本分工没有自行运行测试。定向 GREEN 与完整验证核验见下文。

## 新回归 context 交叉审查

已只读检查 flow 冻结的四项 `server_runtime/startup.rs` 回归。新 `with_startup` 仍使用原 DATABASE_CASES=2 与 45 秒；复用既有 `run_context` 保留 body 之外的 StartupContext，context 持有原 waiter、ShutdownHandle、runtime、init gate 和 native gate。`close` 先 cancel 并释放两个 gate，再等待原 waiter、可能交付的 runtime.shutdown 和同一 cleanup handle；数据库 teardown 位于最后。body 被取消只结束借用 future，不丢弃 context 中原 waiter。

各例的资源断言均在 `owner.close()` 完成后；较早的观察错误或 body 失败仍由外层 context 释放 gate 并排空 owner。native stop 的真实 gate 跨 body 取消保留在 context 中，避免阻塞 stop 时提前进入数据库清理。未 poll 用例只主动 take / drop 公共 wait future，原 ShutdownHandle 继续外持；native 用例取消一次 handle waiter后重等原 handle，typed ShutdownIncomplete 不当成完成。未发现需要修改生产之外其它框架的确认缺口。

## 定向 GREEN 与输入身份：5/6

本分工只读核对 `startup-green.log/json` 和 `after-green-resources.json`。Root 选择四项新 startup 回归与四项既有 init / start / native rollback / stop error 回归，共 **8 passed / 0 failed / 0 ignored / 77 filtered**，exit0，target **4.34 秒**、命令 **40.38 秒**，默认并发，`source_drift=[]`。77 filtered 是本次定向选择，不替代完整 workspace 验收。

原始布尔日志确认：

- 缓冲成功后取消：`ready_notified=true`、`returned_ready=false`、`cleanup_finished=true`，原 RED 的错误交付变为明确失败。
- 未 poll wait 被丢弃：`rollback_finished=true`、`parent_cancelled=false`。
- 原初始化和 stop 失败后再取消：`finished_before_cancel=true`、`returned_ready=false`、`failure_report_unchanged=true`。
- 真实 native stop 穿过原 3000ms grace：`incomplete=true`、`unfinished=true`、`pending_module=true`、`pool_retained=true`、`wait_pending=true`，释放 gate 后 `typed_handle_finished=true`，原失败继续保留。
- 四例均记录 owned_pool_closed、shared_pool_usable、http_released、grpc_released 为 true，workers_left=0、stops=1；没有用取消 token 替代实际 owner / pool / listener 结束证据。

GREEN 清单 SHA-256 `99e269a7aae1f750decfc7777d12f14e976babd56806292b92999c8a8b458c8f`，最终冻结清单 SHA-256 `34595bfb7a942b17a3941474ae8c2c7392caa8c95f4f2e3847fdbf997011f205`，均为 **676 项输入**。本分工重新哈希负责的 `builder.rs`，43093 字节、SHA-256 **`322fc96d83a3ee147144246b9194cc9b1e2b52a2ca8651e818c0da5d18b802fe`**，与两份清单一致；没有再次修改源码。

定向结束后同一任务容器 `857ab743c07698eed70640d54a17a83de59a45c883fc2df23a72a17aaff80a33` 快照为 **0 schema / 0 其他 client**，不称退出瞬间观测。随后 Root 单次运行完整 workspace，结果见下一节；本分工没有自行运行 Cargo 或数据库操作。

## 最终完整回归独立复核：6/6

已只读核对 `final-workspace.log/json`、`validation-summary.json` 与 `final-resources.json`，并按每个 Cargo target 的最后结果独立重新汇总 72 个 target（67 Running + 5 Doc-tests）：**1270 passed / 0 failed / 5 既有 ignored / 0 filtered**。完整 `cargo test --workspace --offline --locked --no-fail-fast` 只执行一次，exit0、**505.00 秒**，默认测试并发，`source_drift=[]`，进程 watchdog 未触发。RED、定向 GREEN 和子进程重入均不重复累计。

完整 `server_runtime` 为 **85/0/0/0 filtered，80.42 秒**；原始日志逐项确认四项新 startup 回归、四项定向既有回归，以及原 native stop、独立实例、运行时 Drop 等相关目标成功。5 个 ignored 仍为既有 PTY 三项和真实 Easy-Turn 两项，本轮没有新增忽略或启用这些外部条件。

最终 manifest SHA-256 **`34595bfb7a942b17a3941474ae8c2c7392caa8c95f4f2e3847fdbf997011f205`**。本分工重新哈希 **676/676** 当前输入，与清单全部匹配；生产 builder 仍为上述 `322fc96d…`。独立重哈希 retained `final-server-runtime`：**122971584 字节**、SHA-256 **`8c71aee1321865e7dfa80df087746712cd5148cdad656948f011eca0069caf6a`**，与原运行摘要一致。Root已补齐源码提交与逐blob核验，见文末。

同一任务 PostgreSQL 在命令结束后约 **0.248486 秒**的快照为 **0 schema / 0 其他 client**，容器 ID 与 RED / GREEN 一致；这是结束后快照，不是退出瞬间。Root已按exact ID/label完成任务容器与卷回收，见文末收据。

本轮证据关闭的是“成功结果已缓冲、交付前已取消却返回 ready”的确认缺口，并补齐未 poll wait、原失败保留和同一 rollback handle 的 selected 边界。早期 connect、任意 SQL 未知确认、discovery 局部失败清理、runtime 强制销毁、所有 handle 丢弃及未完成 native / supervisor 异常，仍不能扩大解释为全部安全。**历史 V01 Companion 25 秒根因继续 open**。原期限、默认并发、CASES、shadow、政策与权限保持；R03 暂停，无依赖、硬件、平台或真实模型扩张。

本分工 **6/6**；提交与回收身份已由 Root 补齐，源码保持冻结，不重复运行已通过场景。

## Root 最终身份与资源补录

源码提交 `9e3a93401aea481f729b21512431b980dfc3b02c`；[commit-verification](evidence/commit-verification.json)确认676项冻结输入逐blob一致，无额外/缺失输入。任务容器`857ab743c07698eed70640d54a17a83de59a45c883fc2df23a72a17aaff80a33`按label核验后stop/rm -v且不存在，共享服务保留，见[cleanup](evidence/cleanup.json)。本分工无剩余执行工作，历史V01与未验证边界按上述记录保持。
