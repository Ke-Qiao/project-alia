# V01 初始数据库构造取消 — 所有权审计

日期：2026-10-15（Asia/Shanghai）；基线 `aca6ca0`，上一轮实现 `dc3ec17`；分工 `/root/v01_fixture`。本轮先只读源码、锁定依赖与上轮四份 HANDOFF；Root 完成确定性 RED 后授权本分工实施测试 helper 修复，见文末。本分工未自行运行 Cargo / 数据库 / SSH / 硬件或提交。

## Checklist

- [x] 阅读上轮 Root / initialization / drain / review HANDOFF，区分已证初始化失败清理与未证整个构造 future 取消。
- [x] 审计 Fixture 构造、三个调用路径、原 45 秒起点与 CASES=3 的所有权。
- [x] 核对本地锁定 SeaORM 1.1.20 / SQLx 0.8.6 的连接和 SQL future 被取消后语义。
- [x] 向 Root 和 flow 交付可 join owner、ready 交付竞态及 CREATE 确认窗口建议。
- [x] 获授权后实现，配合 Root 复核一次确定性 RED → GREEN。
- [x] 核对最终源码身份、完整回归和资源证据，完成交接。

当前分工 **6/6 完成**；V1 失败独立保留，V2 完整回归、最终资源与源码提交均已核对，见文末。源码提交 `8bcda7af892ba36aea98b9de785afa68e3935940`。原期限、默认 libtest 并发、CASES=3、默认 shadow 与当前政策保持；R03 暂停，无新增依赖或生产框架。

## 确认的构造取消缺口

`packages/alia-server/tests/delivery_support/mod.rs` 的 `Fixture::new_with` 已在局部 partial Fixture 外捕获整个建库和初始化过程的 unwind。上轮三项失败回归证明错误 / panic 后会执行工作 pool、schema、admin pool 清理，并恢复原初始化失败。`catch_unwind` 不会拦截 future 被直接丢弃。

目前 `probe_initialization`、`with_db`、`with_db_context` 均先取得 CASES 许可，再 await 局部构造；原 45 秒从数据库构造完成后开始。若整个构造 future 在 WorkPool checkpoint（CREATE 已确认、工作 pool 尚未建立）或 UserSeed checkpoint（工作 pool 和迁移完成）被丢弃，局部 Fixture 随之丢弃，`cleanup()` 无机会执行。普通 pool Drop 不能作为本轮显式 close / join 完成证据，已创建 schema 更不会随 pool Drop 自动删除。

许可现在也由调用者局部变量持有；将清理独立化时，若仍由已取消的 waiter 释放许可，会允许新构造进入而旧资源仍在回收。修复需让原 owner 持有该许可直到最终清理结束，不改变数量 3。

这不是历史 Companion 25 秒超时的已证根因。本轮只有初始构造取消的具体所有权缺口；不得以此宣布历史 V01 已关闭。

## CREATE / pool connect 的真实取消边界

读取的是 Cargo 锁定版本的本地源码，未更新依赖：

- SeaORM `sqlx_postgres.rs` 的 `execute_unprepared` 先 await `pool.acquire()`，再在连接上 await 原 SQL；`connect` 先完整 await SQLx `connect_with`，成功后才把 pool 暴露为 `DatabaseConnection`。
- SQLx `pool/connection.rs` 的 `PoolConnection::drop` 会启动异步 `return_to_pool`；该过程通过 `ping().await` 恢复被取消的 executor / 协议状态。这是依赖自身的异步回收，不是调用者已经 join 的证明。
- SQLx Postgres `executor.rs` 在发送原 Query 时增加 pending ReadyForQuery 计数；后续同连接执行先 `wait_until_ready()`。丢弃等待 future 既不等于发出 PostgreSQL cancel，也不等于已执行事务回滚。
- SQLx `PoolOptions::connect_with` 在函数内部创建 pool，再尝试建立连接，最后才返回 pool。外层在该 await 完成前无法从 `Fixture.db` 取得内部 pool 并显式 close。

因此，CREATE 发出后取消，可能处于“服务端已经执行 / 仍在执行，但客户端尚未确认”的区间。立即在另一连接执行 DROP，再检查 schema 不存在，不能排除原 CREATE 后到。最低复杂度的规则是：**原 owner 不丢弃已经发出的数据库 future；等待它完成，确认结果后执行本次随机 schema 的清理。** pool connect 同样完成后把实际连接交给 owner，再关闭；不能取消 connect 后声称已经关闭未曾取得的 pool。

WorkPool checkpoint 只能证明 CREATE **已经确认完成**之后的取消，不等于真实 CREATE 在途。flow 当前准备的 WorkPool / UserSeed 两项确定性 RED 应维持这个准确命名。CREATE 在途的动态证明需要实际可观察的数据库等待门，不能用短 sleep 猜测到达点。

## 建议的最小 owner 协议

Root 已选独立测试 owner 方向，复用仓库已有“取消请求与可等待完成分离”的方式，但不引入私有生产 RuntimeCleanup 框架。建议一个仅测试内部的 `FixtureInitialization`：

1. `start_with(checkpoint)` 同步启动 worker，返回 ready 接收端、CancellationToken 和原 JoinHandle 的可克隆 completion。worker 自己获取 CASES 许可，并持有至清理结束；尚未取到许可且没有资源时可以响应取消。
2. worker 保留 admin、随机 schema 身份和工作 pool。真实 connect / CREATE / migration / seed / policy / epoch await 均不被取消分支 drop；步骤完成后检查取消，阻止后续阶段。仅测试 checkpoint 的 pending hook 可以 select 取消。
3. 成功时发送 Fixture clone，但 worker 保留原 Fixture，等待 close / cancel。ready 已发送、接收者却未消费就取消，也仍有原 owner 可清理；不需要额外 handoff ack 或移交许可。
4. `ready(&mut self)` 自身使用可撤销的取消 guard：等待 future 被丢弃时请求取消，成功同步接收 Fixture 后解除 guard。Owner Drop 同样只请求取消，不声明清理成功。
5. `close()` 请求取消后 await 原 completion。completion 是原 JoinHandle 的共享结果；外部回归可在取消之前保留它，随后等待同一个 worker 退出，再检查 schema、pool 和资源，而不是轮询 token 或任意 sleep。
6. 原 `with_db` / `with_db_context` 的业务闭包仍在当前任务运行，不增加 Send 要求。只有私有 checkpoint 改为 Send BoxFuture 供初始化 worker 使用。原 45 秒仍从 ready 完成后开始；最终数据库 cleanup 接到 owner.close，保持 body / context / DB 顺序。

初始化 panic 要在 cleanup 后通过 ready 原样传回；清理失败不能转成成功。原失败仍优先，次生 cleanup 失败需要保存在 completion 结果中。工作 pool / schema / admin 的全部清理尝试保持现有顺序和汇总规则。

## 证明范围与待确认点

独立 worker 在 waiter 被丢弃后继续持有资源，不代表那一时刻已完成清理。本轮完成标准应是：取消构造 waiter 后，**预先保存的 completion 已等待原 worker 与清理结束**，然后独立观察 schema 不存在及工作 pool 已关闭。常规入口也必须显式 close + join。

若外部把整个 `with_db` wrapper 和所有 completion clone 一起丢弃，则没有调用者接收 join 结果；不能把 Drop 请求取消写成任意外部强制取消后的完成证明。本轮不为这个未被接管的边界建立全局 supervisor，也不改变原 45 秒起点。

只读审计阶段没有执行本轮 RED / GREEN，也没有实际 CREATE 在途证明。Root 和 flow 负责确定性复现与统一运行，后续授权和证据见下节；不能把连接建立在途或任意网络未知结果写成已覆盖。

## Root 授权后的实现

已只读核对 Root 的 `cancellation-red.log/json` 与 `after-red-resources.json`：旧实现两项确定性 RED **0 passed / 2 failed / 0 ignored / 27 filtered**，target 0.26 秒、命令 12.04 秒，exit101。直接 drop 整个旧构造 future 后均记录 `checkpoint_dropped=true`、`schema_remains=Some(true)`，UserSeed 另记录 `work_pool_remains=true`，最终均在 schema 遗留断言失败。输入 SHA-256 `0243d535656e1ffb6d7ccae8631d08f51d5265ccdcd915f52b63431b939c0a49`，`source_drift=[]`。原证据先记录再兜底回收，任务容器 `4d48658088cc273140bc77eeeb493e33b00085648ae731a08dd88289550b1833` 在退出后快照为 0 schema / 0 其他 client。本分工没有重跑或查询数据库。

据此 Root 授权本分工独占修改 `delivery_support/mod.rs`。原私有 `Fixture::new/new_with` 删除，统一使用 `FixtureInitialization` 的独立 worker，正常业务闭包不进入 spawn。三个 helper 入口均改为 `start → ready → 原业务与 context 清理 → close`，`close` 请求取消并等待原 Shared JoinHandle；失败构造的 ready 也先 join owner，再恢复原 panic。

worker 从原 CASES=3 获取许可，在许可等待阶段可直接取消；取得许可后保留原 admin、随机 schema 和工作 pool。所有已发出的数据库 future 都被等待到结果，步骤完成后才决定是否继续；只对本次测试 checkpoint 允许取消。新增 `CreateSchema` checkpoint 位于保存 admin/schema 后、发出 CREATE 之前。该 checkpoint 的实际锁门测试由 flow 编写，不在本分工文件范围。

ready 发送的是 Fixture clone，worker 始终持有原 Fixture 到 cancel/close 后清理完成，因此发送与接收之间不存在唯一清理 owner 移交。ready waiter 的 DropGuard 在成功接收后解除；只丢弃 waiter 和丢弃整个 owner 都会请求取消。原初始化 panic 在清理后原样发回；次生 cleanup panic 同时通过 completion 的 JoinError 保留，不能变成成功。

实现已限定 rustfmt 并通过本文件 diff whitespace 检查；没有格式化其它主测试文件。API 同步给 flow 和独立 review 后冻结源码；后续定向 GREEN、CREATE 在途证明、完整回归与最终资源证据见下文。

Root / review 继续检查后收紧了两个真实 waiter 边界：`ready()` 改为同步函数，在返回 future 之前就构造 DropGuard，因此未被 poll 的 waiter 被丢弃也会请求取消；如果 oneshot 已缓存成功 Fixture，但此前 waiter 已被取消，则再次等待不会返回这个正在回收的 Fixture，而是先丢弃 clone、等待原 completion，再报告固定取消错误。原初始化 panic 分支继续优先保留。flow 的 UserSeed 回归创建 ready 后完全不 poll，仅等待独立 entered 信号后丢弃 ready，已实际覆盖未 poll guard；另有缓冲 ready 的独立回归。两者仍在本轮四项新用例内，不另增加测试计数。本分工没有自行运行。

## 定向 GREEN 与首次全量输入冻结：5/6

已只读核对 `initialization-green.log/json`：Root 使用原默认测试并发，选择 `initialization::` 和 `initialization_cancel::`，结果为 **7 passed / 0 failed / 0 ignored / 24 filtered**，exit0，target 0.62 秒、命令 11.62 秒，`source_drift=[]`。7 项包含上轮三项初始化失败回归与本轮四项取消回归；不把这些定向结果追加为完整 workspace 的唯一测试数。

- WorkPool / UserSeed 两项旧 RED 均转 GREEN，分别覆盖整个 owner 构造等待被丢弃、创建后从未 poll 的 ready waiter 被丢弃；均在等待原 completion 后检查 schema 和工作 pool。
- CREATE 在途使用同名未提交 schema 作为真实 PostgreSQL 锁门，按精确 CREATE SQL 和 backend PID 观察等待。取消 ready 后，同 backend 仍等待原锁，close waiter 与 completion 均 pending；rollback 释放锁后等待原 completion，再证明 schema、backend 消失且没有进入后续 checkpoint。该用例已经补足真实 CREATE 在途的受控动态证明，不能再称只有 CREATE 已完成后的 checkpoint 覆盖。
- 缓冲 ready 使用已注册 waker 确认消息已发送但尚未被消费；取消后重试返回固定取消错误，并在原 completion 后确认 schema 与工作 pool 回收。

GREEN 清单 SHA-256 `eda0048366781af3fcd7a1602c7d84ee2fd5810a92c3125383a7cd9c037d5561`；首次完整 workspace（V1）清单 SHA-256 `d406435ee26b4e303476a61aeddc5c1204251f320278530479559b6241c7eada`。两份清单均 **675 项**，本分工逐项比较清单内路径 / 内容哈希一致；元数据不同导致清单文件整体哈希不同。自身 helper SHA-256 `c6de195f95f384716fc6b6d3ee4693eb6ac4de551c8b4280f3f1aedcd63df95d`，与两份清单一致；没有重新运行构建或改变源码。

`after-green-resources.json` 记录同一任务容器 `4d48658088cc273140bc77eeeb493e33b00085648ae731a08dd88289550b1833` 为 **0 schema / 0 其他 client**。这是定向命令结束后的快照，不称进程退出瞬间观测。

## 首次完整失败与 V2 版本边界

已只读核对 V1 的 `final-workspace.log/json`、`v1-failure-summary.json` 和 `final-resources.json`：首次完整 workspace **1265 passed / 1 failed / 5 既有 ignored / 0 filtered**，exit101，368.88 秒，`source_drift=[]`；duplex **30/1/0/0 filtered，45.44 秒**。唯一失败是新 `cancelled_buffered_ready_cannot_be_retried_as_a_closed_fixture`：首先捕获预期的固定取消 panic，随后因 UserSeed 资源证据尚未取得而在 `Option::unwrap(None)` 失败。原观测窗口从 admission / 初始化前启动，不能据此证明消息已到缓冲 ready 状态；精确停留阶段未被当次证据捕获，不能归因为某个具体锁或固定排队时间。V1 退出后同任务 PG 仍为 0/0，原失败及 binary `362fed52e55cb93756f047fa5d69f99dea6ac92a665d01d34f70274b26a4b8ef` 已保存，未覆盖或记为通过。

flow 根据实际失败仅修订这个测试：增加本次调用的 UserSeed entered / release 门，先确认达到阶段，再启动原 5 秒 ready 通知观察窗口；没有拿到资源证据时显式关闭独立 observer，再报告失败。没有延长窗口、改变业务期限、降低并发、忽略测试，也没有修改共享 owner helper。

V2 冻结清单 SHA-256 **`51b11511010fedfa47cf8f01b814eb17731ded45b5404fb7ad01ff2e4ce9c553`**，仍为 **675 项**。本分工逐路径比较 V1 / V2 清单，唯一内容变化是 `duplex_playback/initialization_cancel.rs`；helper 仍为上述 `c6de195f…`。当时 Root 先报告 duplex 31/0/0/0 filtered、46.53 秒；本分工没有用局部进度替代完整结果，后续全量与资源收口如下。

## V2 最终独立复核与交付：6/6

本分工只读核对 `final-v2-workspace.log/json`、`validation-summary.json`、`final-v2-resources.json`、`cleanup.json` 与 `commit-verification.json`，没有执行测试、数据库或硬件操作。独立按每个 Cargo target 的最终结果重新统计 72 个 target（67 Running、5 Doc-tests），得 **1266 passed / 0 failed / 5 既有 ignored / 0 filtered**。V2 命令 `cargo test --workspace --offline --locked --no-fail-fast` exit0，用时 **285.11 秒**，默认测试并发，`source_drift=[]`，进程 watchdog 未触发。

完整 duplex **31/0/0/0 filtered，46.53 秒**。原始完整日志逐项确认三项初始化失败回归与本轮四项取消回归均为 ok。共享 helper 的九个 integration 入口对应结果：body_delivery 8/0/0、delivery_repository 15/0/0、delivery_service 12/0/0、delivery_text 6/0/0、delivery_text_service 4/0/0、duplex_playback 31/0/0、output_control 13/0/0、output_lifecycle 11/0/0；voice_stream_live 为 0/0/1既有ignored，未冒称通过。RED、定向 GREEN、失败 V1 与子进程重入都不加到最终 unique 总数。

已重新哈希当前 **675/675** 构建输入，与 V2 清单全部一致。`commit-verification.json` 核对源码提交 `8bcda7af892ba36aea98b9de785afa68e3935940`：675 项输入无缺失、额外或不一致。本分工另从该提交只读取得 helper 内容，SHA-256 仍为 `c6de195f95f384716fc6b6d3ee4693eb6ac4de551c8b4280f3f1aedcd63df95d`。最终 retained duplex binary 独立重哈希为 **112289696 字节**、SHA-256 **`30cbf2e52f05ca128342b45d011e791c52ccf4b51294081bfbb20bc85aa5ab45`**，与 summary 一致；V1 binary 与失败日志继续独立保留。

同一任务容器 `4d48658088cc273140bc77eeeb493e33b00085648ae731a08dd88289550b1833` 在 V2 命令结束后约 **0.175254 秒**的快照为 **0 schema / 0 其他 client**，不将其写成退出瞬间。`cleanup.json` 先核对完整 ID 与 `alia.validation=v01-db-cancel-20261015` 标签，再 stop、`rm -v`，随后确认任务容器不存在；共享 runtime 和其它容器保留。这里只复核 Root 保存的收据，没有重做数据库查询或清理。

本轮关闭的是选定的构造取消 / 原数据库操作排空 / ready 交付 / 显式完成等待边界。所有完成句柄都被丢弃、运行时直接销毁、数据库不可恢复或永不完成的外部故障，仍不能因 Drop 请求取消而宣称完成；未增加全局 registry 或任意取消生产框架。真实连接建立在途等其它网络窗口未逐一强制注入。原期限、CASES=3、默认 shadow、政策和权限不变，R03 暂停；没有新增依赖、平台、录放音或执行器动作。**历史 V01 Companion 25 秒超时根因仍 open**，不能用本轮通过代替历史根因证明。

本分工 **6/6** 并停止写入。文档归档和最终恢复检查点由 Root 统一收口；不再修改已验证源码或重复运行绿色场景。
