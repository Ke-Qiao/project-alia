# V01 部分初始化 — 审计与确定性复现建议

日期：2026-10-15（Asia/Shanghai）；基线 `af5b35b`；分工 `/root/v01_fixture`。先只读审计，随后按 Root 授权先添加定向 RED 回归，再根据真实 RED 实现统一初始化失败清理（见文末）。本分工未自行运行 Cargo/数据库/硬件/SSH，也未自行提交。Root 统一授权实现、验证和交付。原期限、默认并发、`CASES=3`、默认 shadow 与当前政策保持；R03 暂停，无新增依赖。

## Checklist

- [x] 读取上一轮四份 [Root](../2026-10-15-v01-outer/root-HANDOFF.md)、[Flow](../2026-10-15-v01-outer/flow-HANDOFF.md)、[Fixture](../2026-10-15-v01-outer/fixture-HANDOFF.md)、[Review](../2026-10-15-v01-outer/review-HANDOFF.md)。
- [x] 审计 `Runtime::initialize`、`CompanionFixture::initialize/close`、`Fixture::new/cleanup` 及调用方的真实 owner 保存位置。
- [x] 区分已修复所有权、缺少动态覆盖和新确认缺口。
- [x] 向 Root 报告文件行、可重现路径和最小确定性验证方案。
- [x] 获 Root 授权后完成定向 RED 回归、核对真实 RED，并实现统一初始化失败收尾。
- [x] 独立核对 Root 最终 workspace、三项初始化回归、共享fixture调用方、674项输入和资源回收证据。

当前分工 **6/6**，实现与验证已完成收口；只读复核后的最终结果见文末。

## 上轮已证明与未证明的范围

上轮真实 RPC RED 证实 timeout/cancel 丢失 JoinHandle，最终 workspace 1256/0/5、duplex 21/0/0。真实外层 45 秒用例在**全部 owner 完成初始化之后**注入 pending；它证明该阶段的失败清理和 pre-DB 顺序，但未动态触发每个构建 await。旧 V3 25 秒超时原因仍开放，不能把本轮 schema/初始化分析当作它的实际原因。

## 当前初始化所有权核对

| 入口与基线位置 | 已有保证 | 当前结论 |
| --- | --- | --- |
| `duplex_playback.rs:404` 开始 paired initialize | Directory 从创建起就有同步 Drop；SQLite open/bind 和 listener bind 之前没有已启动 Delivery/后台 channel；listener spawn 后立刻装入 RpcServer guard | 没有证据支持重新修复已存在的 RPC guard |
| `duplex_playback.rs:544` 保存 `*slot=Some(Runtime)`，随后 `:562` channel.init、`:565` Delivery.start | 外层 context 持有 scene、server、directory；cancel/panic 后 cleanup 可取得半初始化 Runtime；channel init 自身不启动其 start loops | 结构已补齐，主要缺少真正发生于这两个 await 的动态见证，不预设新生产缺陷 |
| `companion.rs:447` 保存 startup ShutdownHandle，`:448` 等待 startup | startup waiter取消会由生产 ServerStartup Drop请求rollback；handle已在Companion context，close最后wait；ServerRuntime建立后也先存Option再register/hydrate/bind | 与生产已有rollback机制一致，缺少本fixture的runtime None/shutdown Some分支动态覆盖 |
| `delivery_support/mod.rs:28` Fixture::new | admin连接后创建随机schema；只有migration返回Err时特判cleanup | 确认有未被上轮外层owner覆盖的失败缺口，见下一节 |

Companion 的 runtime/voice listener 在各自构建完成后、下一次 await 之前保存；正常异步取消不能插进同一次 poll 的同步赋值间隙。不能仅因源码出现 `Some(await?)` 就认为存在可调度的取消窗口。对同步 panic/OOM 的穷举也不属于本轮已确认故障。

## 确认缺口：数据库 fixture 构建失败可遗留 schema

基线文件 `packages/alia-server/tests/delivery_support/mod.rs`：

- `:38` CREATE SCHEMA 已成功；`:46` 随后的工作 pool connect 使用 unwrap。该步骤失败时尚未构造完整 Fixture，没有路径 DROP 已创建的 schema。
- `:54–56` 仅捕获 migration 的返回 Err，并手动 cleanup；migration panic/cancellation不在这个返回值分支内。
- `:58` INSERT user、`:67–68` initialize_policy、`:70–71` capture_source_epoch 的错误使用 unwrap。完整 Fixture 虽已在局部变量中，但它没有异步 Drop；panic直接退出后不会调用cleanup。
- `with_db :233` 与 `with_db_context :251` 均先 await `Fixture::new()`，之后才进入原45秒限时/catch。因此后续 `finish_case` 不能代替构建失败回收；用户业务closure尚未执行时也可留下schema。

这是静态确认的具体回收路径缺失，尚未运行RED。除schema外，pool最终依赖其自身Drop，不能在没有join/close证据时声称该时点全部连接已经退出。原 migration Err路径如果cleanup也panic，还可能覆盖原初始化错误；修复应沿用原失败优先语义，不能把清理错误变成成功。

该点与 paired late-drain 不同：前者在任务DB fixture 构建阶段，原45秒之前；后者在已构建owner停止阶段，由Root/另一分工负责，不在本分工只读阶段改动。

## 最小确定性验证建议

### DB 构建失败

复用真实随机schema/原migration顺序，仅在测试fixture增加 per-call 固定checkpoint（不要使用进程级环境开关或全局可变hook）。在 migration完成、INSERT user之前制造可辨认的初始化panic或SQL错误，保留该schema名及独立admin作观察/兜底。

用例捕获原失败后，通过独立admin查询同一随机schema仍否存在；旧实现应稳定留下schema，RED用例须先保存这个事实，再自行DROP原schema和关闭自身pool，最后断言失败，避免故意复现污染后续并发。修复后的同一用例要求schema不存在、已建立的work pool已关闭、原panic/错误保持；清理断言需要最外层独立标志，避免被原错误优先规则遮蔽。

最小修复方向是把**整个已创建资源的初始化过程**纳入统一失败收尾，而不是继续逐个unwrap增加特例。若仅包 migration之后的bootstrap，会留下CREATE SCHEMA之后connect失败的原缺口；应明确处理范围。取消安全需要仍位于被取消future之外的partial owner，不能只加catch_unwind就声称支持任意future drop。

CREATE SCHEMA的调用等待被取消时存在“服务端已经执行但客户端未确认”的窗口；若进一步覆盖此边界，partial owner须在发SQL之前保存本次随机schema身份，回收使用仅针对该身份的安全操作。不要把一次构建失败测试写成已穷尽所有网络未知结果。

### Paired 与 Companion 已存在 owner 的覆盖

- paired：在slot已保存、channel.init尚未完成时取消或失败；优先复用真实recover_expired SQL等待/固定fixturecheckpoint。确认slot仍Some，未start的Delivery也可shutdown，session/guard/listener/directory回收，而DB尚可SELECT。此测试补动态证据，不预设旧实现必然RED。
- Companion：在startup.wait未返回、runtime None但shutdown Some时通过已有构建取消/rollback机制停止；close必须等待已有ShutdownHandle，而不是只观察cancel。生产 `server_runtime.rs` 已有 `cancelling_or_dropping_build_future_rolls_back_partial_init` 和 `failed_startup_returns_pending_handle_until_native_rollback_finishes`；先复用其注入方式/现有extension接口，不增加生产hook。
- 已构建但 register/hydrate/bind voice失败时，已有Option owner应负责收尾；只有发现实际清理漏项才继续修改，避免为每个await建立新框架。

真实PG和Tonic用例使用真实时钟；不得把暂停时钟推进到SQL/policy/系统UTC中。保持默认测试线程、CASES=3、所有原成功截止和执行政策。新故障注入只跑一次有界RED/必要修订后的GREEN，不能重新启动旧随机三批求绿。

## 初始审计交接（历史阶段）

只读审计时已向Root报告schema缺口的精确位置和三类测试建议；后续授权、实际RED、修复和定向GREEN见文末。初始审计本身没有动态结果。Linux/Pi、模型、录放音、执行器均未操作；R03暂停。历史V01根因仍开放。


## 授权后的 RED 准备

Root 授权本分工独占 `delivery_support/mod.rs` 与新 `duplex_playback/initialization.rs`；Root 添加模块声明并统一运行。RED准备阶段只加入测试所需 checkpoint，没有提前加失败catch或修复cleanup。

`Fixture::new_with` 使用每次调用自己的 `InitializationStep::{WorkPool,UserSeed,Policy}` hook，普通new使用no-op；`probe_initialization` 使用原CASES=3，不修改并发。CREATE成功后的partial Fixture持有原admin/schema、Disconnected工作pool，用于观察尚未连接阶段；只有成功迁移的原Err分支仍有cleanup。测试只通过schema_name和已有pub db取得这次fixture的观察身份，不引入生产导出或全局fault flag。

三项准备好的测试：

- `work_pool_initialization_panic_removes_created_schema`：CREATE已成功、工作pool未连接时注入固定panic。
- `user_seed_sql_failure_removes_schema_and_closes_work_pool`：只在本随机schema重命名users.password_hash，让原seed语句真实报错。
- `policy_sql_failure_removes_schema_and_closes_work_pool`：只在本随机schema重命名context_policy.policy，让原policy初始化真实报错。

每例在最外层保存schema是否仍在、工作pool是否仍能SELECT的事实，再用自身独立observer和保存的work pool兜底清理原实现遗留，最后检查原失败内容及资源断言。RED不会故意把泄露留给下一例；cleanup内部assert也不会被原失败优先规则遮蔽。本分工没有运行测试，两个源码文件在RED之前冻结供Root统一执行。


## 真实 RED 与最终实现

Root 执行 `cargo test -p alia-server --test duplex_playback initialization:: --offline --locked`，`initialization-red.log/json` 记录 **0 passed / 3 failed / 0 ignored / 21 filtered**，exit 101，命令12.54秒、测试0.32秒；输入manifest SHA-256为 `8212333794bb5d34b94386269ab6fb9129a0f65cead61e89739d37c43f8fe108`，`source_drift=[]`。三项均在验证原初始化失败符合预期之后，报 `failed initialization left its schema behind`。Root记录RED兜底后schema/client为0/0，本分工没有连接数据库。

据此 Root 授权实施修复：partial Fixture改为在CREATE之前保存原admin、随机schema身份和Disconnected work pool；一个catch覆盖CREATE、工作pool连接、migration、seed、policy、source epoch及checkpoint。初始化panic或unwrap(SQL错误)发生时统一尝试cleanup，即使cleanup失败也恢复原panic。原migration专用cleanup分支删除，避免重复和遗漏。

cleanup对Disconnected work pool不调用close；对本次随机schema使用DROP SCHEMA IF EXISTS，随后仍尝试关闭admin pool，最后统一报告固定阶段成败。CREATE失败也位于catch内，因此不会跳过admin关闭。该实现没有改变任何原期限、准入并发、生产政策、依赖或生产exports；没有声称catch能覆盖整个constructor future被外部直接drop的任意取消。

仅授权的两个文件修改；新测试先于修复实际跑过RED。`delivery_support/mod.rs` 与新回归文件已完成限定rustfmt并冻结。Root完成定向GREEN后运行完整workspace，最终结果见文末。


## 定向 GREEN 与最终冻结身份

只读核对 Root 的 `boundary-green.log/json`，命令保留默认测试并发，选择 `initialization::`、`partial_runtime::`、`late_drain::` 三组：**6 passed / 0 failed / 0 ignored / 21 filtered**，exit 0，命令21.88秒、目标用时11.53秒。三项初始化回归逐一为ok；另外两项paired真实初始化错误/取消和一项late-drain也通过，但其实现归属及详细证据由Root管理。

GREEN manifest SHA-256为 `25bfee8a75f0782b5668194ac9a34c740ff0429ec6239834f3d29e7ed9e043ef`，`source_drift=[]`。`after-red-resources.json` 与 `after-boundary-resources.json` 均记录schema/client **0/0**，容器ID同为 `29686cde7e5efc7ddf5939398c32dd5db0c3d1c42c7604d2a2a9579d0e652c25`。本分工只读这些收据，没有启动新查询。

Root随后冻结的完整workspace输入为 **674项**，`final-source-manifest.json` SHA-256 **`3f15e093efd3863143d5bcff0be0540ac4fb5c306b01b0650b69935abfd24747`**。本分工重哈希自身两个文件，确认GREEN、final manifest与当前文件一致：

- `delivery_support/mod.rs`：`d165d6e18d72997bdb9a8526dfa8f7ac0729f97171ed5418fbeb11158e9a8e34`。
- `duplex_playback/initialization.rs`：`0e77daa29cb067dac7aaad720e63df440c70e3c75a70accaa52c2bd306e99060`。

工作pool尚未连接的panic、seed真实SQL错误及policy真实SQL错误都已得到RED→GREEN证明。CREATE错误路径的admin关闭、migration/source epoch错误共用同一catch/cleanup的保证由源码审查支持，本轮没有逐种强制注入；不将其写成额外通过用例。整体constructor任意外部drop的取消语义仍不在本次保证内。

定向GREEN阶段保持源码冻结；最终全量已完成，详见下节。定向6项不追加到全量unique总数，没有重复跑相同绿色场景。


## 最终独立复核：6/6

本分工只读 `final-workspace.log/json`、`validation-summary.json`、`final-resources.json` 和 `cleanup.json`，未重新执行测试或连接数据库。最终完整命令 `cargo test --workspace --offline --locked --no-fail-fast` exit0，**405.60秒**，默认libtest并发，`source_drift=[]`，900秒进程看守未触发。最终总计 **1262 passed / 0 failed / 5既有ignored / 0 filtered**。

原始日志逐项确认三项初始化回归全部ok：工作pool连接前panic、user seed SQL错误、policy SQL错误。完整duplex为 **27/0/0，45.63秒**，已包含这三项及Root的partial Runtime/late-drain用例，不再累计定向GREEN。

本分工另检索全部 `mod delivery_support;` 的9个集成入口并对照最终结果：body_delivery **8/0/0**，delivery_repository **15/0/0**，delivery_service **12/0/0**，delivery_text **6/0/0**，delivery_text_service **4/0/0**，duplex_playback **27/0/0**，output_control **13/0/0**，output_lifecycle **11/0/0**；voice_stream_live为**0/0/1既有ignored**，需要真实Easy-Turn，不能写成该入口运行通过。其余4项ignored同样是既有PTY/Easy-Turn边界，未新增忽略。

最终manifest SHA-256仍为 `3f15e093efd3863143d5bcff0be0540ac4fb5c306b01b0650b69935abfd24747`。本分工逐项重新读取当前**674/674**输入的字节数和SHA，均匹配，无漂移；两个负责文件与先前GREEN/final一致。

资源快照 `2026-10-15T09:17:32.035430Z` 记录 **0 schema / 0 other clients**。按命令开始时间加405.60秒计算，实际采样是退出后 **0.584013秒**，不是事后重启数据库制造的空状态。`cleanup.json`完整ID与 `v01-partial-20261015` label核对一致，stop、`rm -v`和不存在复查均exit0；任务容器/卷已回收，共享runtime/astrbot未修改。

本分工交接完成，停止修改。已动态证明三类初始化失败的资源回收与原失败保留；CREATE/migration/source-epoch的每一种失败、constructor future任意外部drop、Linux/Pi或真实硬件未穷举。**历史V01超时根因仍open**，不以本轮人工故障和1262通过回推旧失败原因。
