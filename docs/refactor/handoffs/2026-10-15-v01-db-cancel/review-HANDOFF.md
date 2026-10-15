# V01 初始数据库构造取消 — 独立审查

日期：2026-10-15（Asia/Shanghai）；基线 `aca6ca09771a66353ebf9209b0a49d0d4f88d800`；分工 `/root/v01_review`。本分工只读源码、依赖实现、官方协议和证据，只写本页；不运行 Cargo、测试、PG、SSH、模型或硬件，不提交。默认 shadow、政策、原期限和默认并发不变，R03 暂停，不新增依赖。

## Checklist

- [x] 核对上一轮 24 份归档原件的长度与 SHA-256，区分历史证明和本轮缺口。
- [x] 检查 constructor、SQLx 0.8.6 与 PostgreSQL 的取消／关闭／事务确认边界。
- [x] 提供最小所有权方案、有限验证范围和假阳性风险。
- [x] 交叉审查候选 diff，确认原 SQL future 与完成句柄保留、失败可观测。
- [x] 核对本轮原始 RED／GREEN／全量与资源清理证据，给出最终意见。

当前进度：5/5；V2 完整回归、资源回收和源码提交身份已独立核验，本轮可交付。历史 V01 25 秒根因仍开放，不能将本轮完成解释为整个 V01 关闭。

## 上轮证据身份

`../2026-10-15-v01-partial/evidence-index.json` 的 24/24 文件长度和 SHA-256 重算一致。按每个 Cargo target 的最后一条结果计数，67 个 Running 加 5 个 Doc-tests，共 1262 passed、0 failed、5 ignored、0 filtered；duplex 为 27/0/0/0，45.63 秒，完整命令 405.60 秒。日志另含三个子进程重入结果，不能再加进总数。

674 项源码 manifest SHA-256 为 `3f15e093efd3863143d5bcff0be0540ac4fb5c306b01b0650b69935abfd24747`，记录的源码漂移为空。保留二进制身份记录为 `a593537e58bceca40c2113456b6255c9e578601b8002b5acef7dfc3d07c104e4`（111180224 字节）；本轮未再次执行它。5 项忽略是既有 PTY 3 项和 Easy-Turn 2 项。

相同数据库未经重启，在命令结束约 0.584 秒后观测到 schema=0、other_clients=0；这是退出后的快照。随后按完整容器 ID 与 label 核验并执行 stop、rm -v，确认任务容器不存在，保留共享运行时及 astrbot。上一轮 RED 0/3 和定向 GREEN 6/0 不累加进最终总数。以上证明初始化 panic、部分 paired runtime 取消和普通 late-drain，不证明初始数据库 constructor 任意取消安全，也未关闭历史 Companion 25 秒超时根因。

## 本轮确认的缺口与协议边界

基线 `Fixture::new_with` 把 admin pool、随机 schema、work pool 和正在执行的 SQL future 都放在可取消的 constructor 内。`catch_unwind().await` 仅在 panic 展开时进入补偿；外层丢弃该 future 不会运行后续 `f.cleanup().await`。尤其在 `CREATE SCHEMA` 已发出、结果尚未交给调用者时，不能从客户端 future 消失推导数据库没有副作用。

已对照 Cargo.lock 锁定的本地 SQLx 0.8.6 实现：

- `sqlx-core/src/pool/connection.rs`：PoolConnection Drop 会调度 return-to-pool；普通归还经过 ping 来排空协议，但 pool 已关闭时直接关闭连接。不能以依赖内部调度代替本 fixture 明确拥有的完成凭据。
- `sqlx-postgres/src/connection/mod.rs`：`PgConnection::close` 发送 Terminate 后关闭 socket，不等待原业务 SQL 的最终结果；`wait_until_ready` 才排空 ReadyForQuery。
- `sqlx-core/src/transaction.rs` 与 `sqlx-postgres/src/transaction.rs`：Transaction Drop 只排队 rollback；它不同于 await rollback 成功，也不能消除已发送 COMMIT 的未知确认问题。

[PostgreSQL 官方协议 54.2.8–54.2.9](https://www.postgresql.org/docs/current/protocol-flow.html#PROTOCOL-FLOW-CANCELING-REQUESTS) 明确取消请求可能没有效果，客户端仍须等待原查询回复；非显式事务中的写入也可能在数据库发现断连前完成提交。因此，先关闭 pool，再在另一条连接执行 DROP IF EXISTS，不能仅凭 close 已完成证明原 CREATE 不会晚到。

## 接受的最小修复方向

仅调整测试 fixture 的生命周期：同步建立 startup／completion 所有权，独立 worker 持有 CASE permit、原 connect／CREATE／后续 SQL future 和 Fixture；调用者丢弃 ready waiter 只撤销交付资格并请求取消。真实数据库操作应继续由原 future 排空，在其完成后的确定边界检查取消，再清理同一个 schema，最后完成原 JoinHandle 的 receipt。测试专用 pending hook 可以接受取消，不能把这条便利扩展成丢弃真实 SQL future。

正常 ready 仅交付 Fixture clone，worker 仍持有清理所需原 owner，直到显式 close；整个初始化、使用等待和清理期间都保留原 CASE permit。completion 可参考既有 ServerStartup／ShutdownHandle 的原 JoinHandle Shared 方式复等，不新增生产框架。Drop 请求取消本身不等于清理完成，必须 await 原 completion 才能作出完成结论。

后续 diff 必查：ready send 与 receiver drop/token cancel 的竞态；worker 捕获原 panic 后仍执行 cleanup；原 panic 和 cleanup failure 都有可观测出口；任何 shutdown waiter 被取消后仍能等待同一 receipt；worker 绝不能在 cleanup 前释放 CASE permit。45 秒业务／context deadline、原 Companion 25 秒与默认并发不改变；初始构造原本位于 45 秒之外，不借此新增或延长业务期限。

## 有界验证与假阳性风险

旧实现只需针对确认的 constructor drop 点形成有限 RED；修复后验证同样取消点，并增加一个真正的 in-flight CREATE：用另一条连接的未提交同名 schema 制造锁等待，从 pg_locks 或等价服务端状态确认原 CREATE 正在等待，再丢弃 ready waiter。必须在释放锁后排空原 CREATE 并观察 worker cleanup 完成，不能只用 hook 到达表示 SQL 已执行，也不能先手工 DROP 后声称自动清理成功。

保留外部、独立的完成与资源断言。原 panic 优先传播会遮蔽 cleanup 内的断言，若检查位于其 catch 内，须用外层 receipt／标志再次确认。schema 与 client 的失败观测必须先于兜底清理记录；兜底成功、容器重建或 watchdog 杀进程均不等于实现通过。pending 期间 completion 不能提前完成、permit 不能提前归还；释放真实阻塞后原 completion 必须能完成。证据应使用同一随机 schema 身份，避免查错对象导致假绿。

合理上界是本轮确认点的一次 RED、一次定向 GREEN 和源码冻结后的完整 workspace；失败才按具体证据修订，不追加历史无变化的超时循环。保留原日志、source manifest、命令退出码与准确快照时间。默认测试并发、依赖与平台范围不变，不暴露数据库 URL／密码／用户配置。

## 边界

本分工没有执行测试、数据库或硬件操作。进程强杀、runtime teardown、数据库不可达导致无法排空，以及生产 supervisor panic/running 标志异常，仍不能由本 fixture 的普通取消路径直接推定已清理。

## 候选审查进展

已只读检查 `delivery_support/mod.rs` 初版候选：独立 worker 持有原 connect／SQL future 和 CASE permit；测试 hook 接受取消，真实数据库操作不放进取消 select；成功 ready 发送 clone 后 worker 等待显式取消，初始化 panic 先清理再由 ready 传播，cleanup panic 由共享原 JoinHandle 单独记录。业务 45 秒起点及 context 清理顺序保持原样。

Root 发现并修正 `async fn ready` 在首次 poll 前未构造 DropGuard 的缺口，改为同步创建 guard 再捕获进 async future。本分工另发现已缓冲 ready clone 在取消后重等仍可交付的问题；Root 已接受成功分支检查 cancel，并安排以实际 oneshot 唤醒证明缓冲后取消的回归。最终实现已核对，结论见后文，运行证据仍待核验。

已读取本轮 `cancellation-red.log/json`：旧实现两处 constructor drop 均失败，0 passed／2 failed／0 ignored／27 filtered，测试 0.26 秒、命令 12.04 秒，退出码 101，默认测试并发，源码漂移为空。两处均先观察 `checkpoint_dropped=true` 且 schema 仍存在；work pool 已建立的情形还观察到 `SELECT 1` 成功。之后才执行兜底，资源快照 0 schema／0 other clients。RED 只证明原缺口，不计入最终通过数，也不能把兜底后的 0/0 算作原实现清理成功。

## 首次 GREEN 前最终候选结论

已读最终 `delivery_support/mod.rs`、`duplex_playback.rs` 接线、`initialization.rs` 枚举适配和完整 `initialization_cancel.rs` 四项回归；没有新的源码阻断，GO 进入首次定向验证。

两处 ready 修正都已落实：同步创建的 guard 随尚未 poll 的 future 一起 drop 时也请求取消；已取消后的 `Ok(Ok(fixture))` 会丢弃 clone、等待原 completion，再报告取消。这里 completion Ok 只证明 owner 清理完成，不能解释为初始化成功；原初始化 panic 仍由 ready 单独传回，若清理也失败则共享原 JoinHandle 返回错误。

四项测试分别覆盖：WorkPool 阶段丢弃整个 owner 和 ready waiter；UserSeed 阶段创建但完全不 poll ready 后丢弃 waiter；实际 PostgreSQL 同名 schema 的未提交事务使原 CREATE 处于锁等待，取消后确认同一 PID 仍等待且 close／completion 都 pending，再 rollback 放行并等待同一 completion；仅手工 poll 一次 ready 注册自定义 waker，等发送通知但不再次消费，drop 后重等须明确取消。

真实 CREATE 测试先捕获所有等待／pending 观测，再释放阻塞事务并 drain owner，最后查询原 backend 退出和 schema 不存在，之后才执行兜底；没有把关闭连接或 DROP IF EXISTS 单独当作原 CREATE 已排空。buffered-ready 测试的 waker 仅注册在 ready receiver，无新生产 hook。断言位于最外层且在资源兜底之后，不会被原 panic 优先逻辑遮蔽。5 秒仅约束新测试的状态观测，没有改动原业务 deadline。

本轮没有额外动态注入 admin connect 中断、permit 饥饿、双重 init＋cleanup 错误或 runtime 强杀；这些不应在最终文字中称为已实测覆盖。CASE permit 持有到 worker 退出、所有实际 SQL await 不被取消 select 包裹，目前由候选源码直接确认，后续完整回归验证共享 helper 的现有调用路径。

## 首次定向 GREEN 核验

已独立读取 `initialization-green.log/json`，四项新取消回归与三项原初始化失败回归全部通过：7 passed／0 failed／0 ignored／24 filtered；target 0.62 秒，命令 11.62 秒，exit 0，默认 libtest 并发，source_drift 为空。只核算这七项本次执行，不与 RED 或随后全量累加。

GREEN manifest SHA-256 为 `eda0048366781af3fcd7a1602c7d84ee2fd5810a92c3125383a7cd9c037d5561`；当时 V1 全量冻结 manifest 为 `d406435ee26b4e303476a61aeddc5c1204251f320278530479559b6241c7eada`。两者均以 `aca6ca0` 为基线、包含 675 项输入；逐文件长度与 SHA 完全一致，整份 manifest 的差别来自快照元数据。本分工又按最终 manifest 重算当前 675 项，未发现源码漂移。

`after-green-resources.json` 记录同一任务数据库 schema=0、other_clients=0；按命令开始时刻加 elapsed 推算，快照在命令结束约 0.152 秒后取得，不能称退出瞬间观测。当时尚待首次完整 workspace，本条不是最终回归结论；后文已收口 V1 失败与 V2 最终结果。

## 首次全量发现的观测缺口

已只读核对运行中的首次 `final-workspace.log`：duplex 为 30 passed／1 failed／0 ignored／0 filtered，45.44 秒；失败为新 buffered-ready 测试。其 retry 已报告初始化取消，随后 `evidence.take().unwrap()` 得到 None。该证据证明 UserSeed checkpoint 尚未进入，不能从日志单独断言恰好停在 CASE permit：原 5 秒同时包含了三槽排队、admin connect、CREATE 和迁移。

定向 7 项通过并未覆盖完整 31 项并发下的排队；本分工首次静态 GO 没有识别这一观测起点问题。全量失败必须保留为失败记录，不以重跑覆盖或与修订版成功合并。

接受的最小修订：仍只 poll ready 一次来注册 receiver waker；另外用 UserSeed entered/release 的独立 oneshot 握手，确认该 checkpoint 已进入并放行后，才开始原 5 秒通知观测。三槽限制、默认 libtest 并发和所有原业务期限保持不变。此次五秒仅衡量本测试所声称的 ready 通知，不能把尚未获准进入构造的排队当作该通知失败。

失败兜底也需收口：当前 None unwrap 发生在显式 observer.close 之前，跳过观察连接的 await close。应保留 Option，Some 执行原观察后兜底，None 也先显式关闭 observer，再在外层断言 checkpoint 到达；entered/release 的错误亦应先记录，drop ready、await 同一 completion 并清理 observer 后才断言。全量尚运行时保持源码冻结，没有重复测试或修改源码。

## V2 冻结前复审

已核对首次全量最终原件，按 Cargo target 最后一条结果重算为 1265 passed／1 failed／5 ignored／0 filtered，67 Running＋5 Doc-tests；命令 368.88 秒、exit 101、source_drift 为空，资源快照为 0/0。本轮先前定向通过、这一完整失败、随后 V2 完整回归应始终分开记录。

已将保留的 `v1-initialization-cancel.rs.txt` 与修订源码逐行比较：仅 buffered-ready 测试增加 UserSeed entered/release 阶段门，把原 5 秒 ready 观测起点移到该阶段放行后，并收口 None 的 observer 清理。entered/release 错误先保存而非 unwrap，随后 drop ready、捕获重等结果、await 原 completion，最后清理 observer 并在外层断言；Some 路径沿用观察先于兜底，None 路径显式 await observer.close。

原 CREATE drain 测试及独立 owner helper 不变，没有改变三槽并发、默认 libtest 调度、原业务期限或五秒长度。没有发现新的源码阻断，GO 进入一次 V2 完整 workspace；不追加相同源码的定向批次。此次修订以实际全量失败为依据，不把上一批失败变成忽略项。

## 最终 V2 独立核验与提交

已逐 Cargo target 重算 `final-v2-workspace.log`：67 Running＋5 Doc-tests，共 1266 passed／0 failed／5 ignored／0 filtered；duplex 31/0/0/0，46.53 秒。完整 `cargo test --workspace --offline --locked --no-fail-fast` 命令 285.11 秒、exit 0、默认 libtest 并发、source_drift 为空、900 秒进程 watchdog 未触发。5 项忽略仍为原 PTY 3 项与 Easy-Turn 2 项，没有新增忽略或降低并发。

最终 675 项 manifest SHA-256 为 `51b11511010fedfa47cf8f01b814eb17731ded45b5404fb7ad01ff2e4ce9c553`。本分工独立用 `git cat-file --batch` 读取源码提交 `8bcda7af892ba36aea98b9de785afa68e3935940` 中每一项 blob，逐项长度／SHA 与冻结 manifest 一致，675/675 无差异；提交仅包含四个测试文件，不含生产代码或依赖变更。`commit-verification.json` 的 checked_inputs=675、mismatches／extra_inputs／missing_inputs 为空，与独立核对相符。

保留二进制也已重新计算：V2 `final-duplex-playback` 为 112289696 字节，SHA-256 `30cbf2e52f05ca128342b45d011e791c52ccf4b51294081bfbb20bc85aa5ab45`；首次失败 `v1-duplex-playback` 为 112289376 字节，SHA-256 `362fed52e55cb93756f047fa5d69f99dea6ac92a665d01d34f70274b26a4b8ef`。RED 0/2、定向 GREEN 7/0、完整 V1 1265/1/5 均独立保留，不累计到最终 V2 1266/0/5。

`final-v2-resources.json` 在同一任务数据库、未经重启记录 schema=0 和 other_clients=0；按命令时刻计算，快照位于退出后约 0.175 秒，不是退出当刻。容器身份始终为 `4d48658088cc273140bc77eeeb493e33b00085648ae731a08dd88289550b1833`。`cleanup.json` 先核对该完整 ID 与 `alia.validation=v01-db-cancel-20261015`，再成功 stop、rm -v，并查询确认任务容器不存在；共享 astrbot 前后身份一致并保留运行。回收发生在资源快照之后，不能替代快照或掩盖泄漏。

最终独立意见：本轮普通初始化取消与 ready 交付边界的确认缺口已修复，证据足以交付；默认 shadow、当前政策、原期限／精确取消要求不变，R03 暂停。没有将未知 SQL 结果、Drop 请求、watchdog 或取消后的 completion Ok 冒充初始化成功。此结论不覆盖前述强杀、永久数据库丢失或生产 supervisor 异常边界，也不关闭历史 Companion 超时根因。Root 后续归档索引仍可核验复制件身份，但无需再执行测试。
