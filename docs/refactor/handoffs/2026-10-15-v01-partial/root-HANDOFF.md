# V01 部分初始化与 paired late-drain — Root HANDOFF

2026-10-15，分支 `codex/architecture-refactor`，基线 `af5b35b`（前轮实现 `a73e6e8`）。本包 **10/10 完成**，源码提交 `dc3ec17ff7da45ed02f7d69d3615de69217bd352`。

## 范围和已确认缺口

先检查前轮四份 HANDOFF、源码和证据，再由三个 subagent 分别负责数据库初始化、paired late-drain 和独立审查。Root 负责真实 paired 初始化 SQL 失败/取消、统一验证及交付。没有生产源码、配置、依赖、权限或默认 shadow 变更。

共享 `delivery_support::Fixture` 原本只在 migration Err 时清理；CREATE 成功后到返回前的其它 panic/unwrap Err 会遗留 schema。一次确定性 RED 的三个测试全部命中：工作 pool 连接前注入 panic、用户种子 SQL 列缺失、政策 SQL 列缺失。测试先记录遗留，再自行回收，仅在回收后外层断言，因此 RED 未污染后续。原始日志和0/0资源快照已保存。

修复把未完整的 Fixture（admin/schema/Disconnected work pool）保留在统一 catch 之外；CREATE、pool connect、迁移、seed、policy、epoch 失败后均先尝试 cleanup 再恢复原 panic。未连接 pool 跳过 close，随机 schema 用 DROP IF EXISTS，schema 失败仍尝试关闭 admin。没有新全局故障开关、依赖或生产 shutdown 框架。

共享 fixture 被九个 integration target 引用：delivery_service、output_control、body_delivery、voice_stream_live、delivery_text、duplex_playback、output_lifecycle、delivery_text_service、delivery_repository。正常调用 API 不变；因此最终执行一次 workspace 回归，保留默认 libtest 并发和原 CASES=3。

## 新增覆盖

- 初始化三项 RED → GREEN，最外层核对原 panic、schema 消失与 work pool 关闭。
- paired 初始化错误：只在本次随机 schema 改表名，触发实际 channel.init SQL Err；cleanup 证明保存的 Runtime、RPC join、session、目录先清理，DB 此时仍可用，结束后 DB 已关闭。
- paired 初始化取消：用实际 PostgreSQL 表锁阻塞 channel.init，查询本 schema 的 pg_locks 见未授予锁后 drop 原 initialize future；没有固定短 sleep 猜测阶段，也没有新增初始化钩子。已发布的部分 Runtime 留在外层 context，先 rollback 测试锁后清理。外层独立 AtomicBool 防止原 body panic 掩盖清理断言。
- paired late-drain：保持真实 SubmissionWrite，在真实时间中借用同一个 close future 观察 11 秒，超过原 10 秒 Delivery shutdown；记录 owner pending、DB/目录/listener 仍保留后，finish 原写入并 join 原 close/dispatch。独立证据最后断言首次 paired Delivery cleanup failed 仍保留、native quiescent、supervisor 已退出、RPC/目录释放，DB 最后关闭。

Companion 现有 `ServerStartup` 的 ShutdownHandle 已在 wait 前保存；现有 server_runtime rollback 测试继续复用，没有增加另一套框架。

## 最终验证

- RED：0 passed / 3 failed / 0 ignored / 21 filtered，测试 0.32 秒，命令 12.54 秒。三个预期失败；source_drift=[]。
- 定向 GREEN：6 passed / 0 failed / 0 ignored / 21 filtered，测试 11.53 秒，命令 21.88 秒；source_drift=[]。随后原任务 PG 0 schema / 0 其他 client。
- 全量：冻结 674 源码输入，SHA-256 `3f15e093efd3863143d5bcff0be0540ac4fb5c306b01b0650b69935abfd24747`，一次 `cargo test --workspace --offline --locked --no-fail-fast` 退出0，**1262 passed / 0 failed / 5既有ignored / 0 filtered**，405.60秒；67个Running目标与5个Doc-test。duplex **27/0/0**，45.63秒；RED/定向/子进程重入不累计。

## 保留边界

原 25 秒 Companion、32 秒其它 paired、45 秒 body+cleanup 成功截止、3 秒 RPC、1 秒诊断、10 秒 Delivery shutdown 及 CASES=3 不变。晚于截止的清理继续等待原 owner，不能转成成功。

任意外部直接 drop 整个初始 Fixture::new future 不在本轮安全保证中；初始建库/迁移及最终 SQL teardown 仍在原45秒窗口外。未穷举 native 锁中毒、supervisor panic 或永不退出后代；running=true/nohandle 不等于已证明 quiescent，不能通过清旗标或 watchdog 杀进程宣称完成。

历史 V01 25 秒根因仍开放，仅有新增失败、原失败 artifact 或可区分条件时再启动有界定位，不重复历史批次求绿。R02硬件 AEC 优先资料门槛保持；R03 暂停；R05 仍待现场出口，六轮仍 2/6。没有 Linux/Pi、硬件、真实录放音、模型或长稳验证。

## 提交、资源与证据收口

源码 `dc3ec17` 中674个输入与冻结清单全部匹配（缺失/额外/不一致均0）；独立审查无阻断。一次RED、一次六项定向GREEN、一次完整workspace均保存，没有靠重复历史场景求绿。

同一任务PG未重启，命令退出约 **0.584秒** 后快照为 **0 schema / 0其他client**。核对完整container ID与`v01-partial-20261015`标签后stop并`rm -v`，确认任务容器不存在；共享服务和运行时保持。原件见[资源时点](evidence/resource-timing.json)和[回收](evidence/cleanup.json)。

[evidence-index.json](evidence-index.json)归档24份原始文件，包含RED/GREEN/最终日志、运行身份、三阶段源码清单、摘要、资源和提交核对；均保留原字节。大binary只留在ignored run目录：111180224字节，SHA-256 `a593537e58bceca40c2113456b6255c9e578601b8002b5acef7dfc3d07c104e4`。归档脚本保留原run目录假设，不应直接从文档目录执行。源码格式和diff核对通过；原始日志EOF空行不改写，静态记录单独注明。

恢复从[本轮检查点](../../resume-checkpoint-v01-partial-2026-10-15.md)继续。三个subagent已核对各自用例/独立证据；没有push，正常提交源码和文档。
