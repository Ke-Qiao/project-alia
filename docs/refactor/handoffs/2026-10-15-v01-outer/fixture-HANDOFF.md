# V01 外层截止与失败清理 — 夹具审计

日期：2026-10-15（Asia/Shanghai）；基线 `99c946d`；分工 `/root/v01_fixture`。本分工先审计，随后按 Root 授权实现 `RpcServer::close` 的最小修复和三项 RPC 回归；之后冻结源码，仅更新本文。Root 实现其余 fixture owner 与数据库清理协调，并统一执行测试、证据归档、资源回收和提交。本分工没有自行运行 Cargo、数据库、SSH、硬件或 Git mutation。

默认 shadow、当前政策、原期限、默认测试并发和 `CASES=3` 保持；R03 继续暂停。没有新增依赖或生产生命周期框架。

## Checklist 与当前状态

- [x] 读取上一轮 [Root](../2026-10-15-v01/root-HANDOFF.md)、[flow](../2026-10-15-v01/flow-HANDOFF.md)、[fixture](../2026-10-15-v01/fixture-HANDOFF.md)、[review](../2026-10-15-v01/review-HANDOFF.md)。
- [x] 核对 paired Runtime、Companion、shared `delivery_support` 的期限范围和清理顺序。
- [x] 核对生产已有 `ServerRuntime`/`ServerStartup` 清理 owner、`ShutdownHandle`、Delivery join 保留及当前 Tokio test-util。
- [x] 先写三项 RPC 回归，读取 Root 的旧实现 RED，确认两项具体 handle 丢失。
- [x] 修复 RPC join 保留，冻结源码，并复核 Root 最终 owner/真实时钟方案。
- [x] 独立核对 Root 最终修订版 workspace 原日志、三项 RPC 结果、671 项输入、保留 binary 和数据库资源快照。

当前分工 **6/6**。确定性 RPC RED 已完成；最终修订版 workspace **1256 passed / 0 failed / 5 既有 ignored**，duplex **21/0/0**，含本分工三项 RPC 回归通过。详见文末独立复核。

## 基线缺口与修复范围

基线 `RpcServer::close` 把 `self.task.take()` 按值移入原 3 秒 timeout。超时或 close future 被取消时，JoinHandle 随 future 丢弃，而 `RpcServer::Drop` 已找不到 handle，因此只能 cancel token，不能 abort/join 仍存活的任务。这是本轮可独立重现的 fixture 所有权缺口。

本分工完成的修复只在原 close 方法：3 秒 timeout 改为借用 `self.task.as_mut()`；显式超时后先保留 handle、abort 并 await 同一任务，终止结果收集后才 take；最后仍报告原 timeout 或 JoinError。close future 中途被取消时，既有 Drop 仍拥有 handle，可以 abort 任务。没有改变 Drop 本身、3 秒预算、生产代码或依赖。显式 close 超时路径具有 join 证据；close waiter 被取消后的回归证明实际任务析构与 listener 释放，不把 Drop 发 abort 描述成同步 join。

基线 `with_db` 的原 45 秒直接包住整个测试 closure，超时会 drop 含有 paired/Companion owner 的 future，随后开始数据库回收。正常 cleanup 因而可能被跳过，或 production ServerRuntime 的独立 cleanup 尚未结束就遇到 pool 关闭/schema 删除。paired Runtime 原本没有独立 Drop 收尾；Companion 内层 25 秒只覆盖实际业务，不能保护其前方初始化和后方清理。

Root 的最终实现把实际 context owner 保留在被限时 body 之外，paired 初始化及 Companion 已创建的资源都存入该 owner；收尾复用现有 revoke、release、stop、shutdown_handle/wait 与 RPC close。`with_db_context` 的 body 和 context cleanup 共用**同一个原 45 秒截止**；到期即使 cleanup 后来完成仍判失败，但不丢掉正在收尾的 future，不抢先关闭数据库。普通 `with_db` 调用保留其入口；cleanup 和数据库 cleanup 的 panic 分别收集，原 body panic 优先保留。这里只概述核对到的最终代码，详细 owner 审查和统一运行由 Root/其他分工负责。

重要区别：截止后继续释放和 join 资源是失败后的收尾，不是延长成功期限。清理若永久不能完成仍需外部看守中止整个测试并保留证据；不能将该看守 kill 当作正常回收。

## RPC 的确定性 RED 与三项回归

Root 执行旧实现时，本分工已经只添加测试，未提前改实现。读取 `run/validation-20261015-v01-outer/rpc-red.log` 和 `rpc-red.json`：默认 libtest 并发，**1 passed / 2 failed / 0 ignored / 10 filtered**；命令 exit 101、49.55 秒（含编译），目标测试用时 0.00 秒。输入 manifest SHA-256 为 `e4ea72ed07b62ce6c6cc7e9d7098bf63bbbd1e321142aab73e3635a4527009d7`，`source_drift=[]`。

| 回归 | 原实现结果 | 实际断言 |
| --- | --- | --- |
| `rpc_close_timeout_joins_task_before_reporting_failure` | 失败：timeout 返回前 listener 未释放 | 原 3 秒虚拟 timer；失败返回前 task owner 已析构；端口可重新 bind；原 `Elapsed` 仍可观察 |
| `cancelling_rpc_close_keeps_task_abort_owned` | 失败：取消 close 后 task detached | 先 poll close 到 pending，再 drop waiter；要求真实 task 析构通知和 listener 释放 |
| `rpc_task_panic_is_reported_after_listener_release` | 通过 | task 主动 panic 后 listener 已释放，close 仍报告原 task panic 文本 |

三项均无数据库或硬件，只绑定 loopback listener。析构通知在 listener drop 后发送。前两项保留独立 AbortHandle 兜底：先保存旧实现未释放的事实，再 abort 并收回测试任务，最后报告原断言失败，因此 RED 本身不会留下挂住 listener。取消用例精确要求 `matches!(cancellation, Ok(Ok(())))`，不会把通知 sender 意外消失当作成功。

RPC RED 后 Root 授权修改实现，本分工完成定点 patch，没有整文件格式化或覆盖 Root 对其他区域的编辑。源码随后保持冻结；本分工已从 Root 最终完整 workspace 原日志确认三项全部通过，没有自行补跑。

## 最终验证方案与证据要求

已有 Tokio `test-util` 来自 Server dev-dependencies，workspace 已启用 `full`，没有新增 feature。虚拟时钟仅用于三项纯 RPC 控制流及无真实 DB 的清理顺序用例；这些测试能迅速触发原 3/45 秒 timer，但必须另外等待 entered/exit witness，不能把 `advance` 返回当作任务已结束。

**真实 PG/Companion 外层用例采用原 45 秒真实等待**，不 pause/advance 生产 Runtime、Tonic 或 SQL 的时钟。在已完成真实 owner 初始化、内层 25 秒尚未开始的固定 setup barrier 注入 pending；这样命中的是外层原 45 秒，不能被内层 25 秒抢先或被虚拟时间扭曲。失败必须仍是 `Delivery test exceeded 45s`。

最终代码还包含真实 heartbeat task panic 与原 body panic 同时存在的用例，要求其它 owner 仍被清理，且最后恢复原 primary failure。无 DB 控制流用例覆盖 body drop→context cleanup→database cleanup 的顺序、cleanup 跨过原截止仍失败但只 drain 一次，以及多处 cleanup panic 不掩盖 primary panic。这些由 Root 统一执行，最终原日志已记录通过；未实现的先前方案建议不算已运行结果。

真实资源证据核对点包括：

- 生产 ShutdownHandle finished/wait、runtime task 数为零、Delivery 不再 pending；PlaybackSession inactive。
- heartbeat 实际退出，RPC close/join 或 task 析构证据成立；listener 端口可重新 bind；临时目录已移除。
- 在 context cleanup 阶段执行 `SELECT 1` 仍成功，证明真实 owner 收尾时 pool 尚在；cleanup 完成后同一工作 pool 已关闭。
- 独立 cleanup 成功标记在资源断言之后写入，防止“预期 body 失败”掩盖 cleanup 断言自身失败。
- Root 在测试结束、任务数据库仍在线时采集 schema/连接快照，再精确移除本任务容器；不以事后停止状态补写即时快照。

固定 stage、无身份计数和结果足够；不加入 owner/source/device UUID、内容、private grants 或原始 SQL 错误。原成功场景的一次 TTS/Response/Admission、原输出身份和精确取消断言保持。

## 结果与剩余边界

当前已确认并修复 RPC fixture 的两个 handle 丢失边界，已查看 Root 的真实 RED 原件和最终完整 workspace 结果。复核使用文件读取与 Python 标准库哈希，没有运行测试二进制、Cargo 或数据库：

- `final-v2-workspace.log` 明确列出三项 `cleanup::` RPC 回归全部 `ok`；同一日志中的完整 duplex 为 **21 passed / 0 failed / 0 ignored / 0 filtered**，目标用时 **45.60 秒**，包括真实外层 45 秒及 heartbeat primary/cleanup 失败用例。
- `final-v2-workspace.json` 的完整命令为 `cargo test --workspace --offline --locked --no-fail-fast`，exit 0，**347.91 秒**，默认测试并发，`source_drift=[]`，900 秒进程看守未触发。
- `validation-summary.json` 的 **1256/0/5** 仅计最终完整运行；3 项 PTY 与 2 项真实 Easy-Turn 仍为既有 ignored，不把 RED、初始 GREEN、中止运行或子进程重入重复计入。
- 本分工实际重哈希保留文件 `run/validation-20261015-v01-outer/final-duplex-playback`：**110661120 字节**，SHA-256 **`bc2c7cb8e1291aa60547963d56e45071703ec8df68c9b4e04e2623d812865184`**，与 summary 一致；哈希前后 size/mtime 未变化。
- 最终 `final-v2-source-manifest.json` SHA-256 为 **`021b2fb3d46d64ddd98c2344cb078647b66bc143dc1f41d4b5d1326e19b6a6d2`**；逐项读取当前 **671/671** 输入，字节数与 SHA 全部匹配，无漂移。
- `final-resources.json` 记录任务容器 `f160b5cfa550d5345aa879ebcbc4d88686d339423624bcf43433d2a609603464` 在线时 **0 schema / 0 other clients**。按 command start＋elapsed 与 snapshot UTC 计算，快照在命令退出后 **56.106 秒**，不能称为退出同一瞬间。Root 确认期间未重启数据库；本分工没有额外连接或查询数据库，最终容器移除证据由 Root 管理。

上述结果足以完成本分工交接，停止源码和文档编辑。

历史 V3 failing artifact 仍缺失，**V01 历史超时根因保持 open**。本轮人工故障注入只验证新确认的外层取消与清理所有权，不能据此回推旧 25 秒超时原因或将它关闭。真实模型、录放音、执行器、Linux/Pi 和长期负载均不在本分工验证范围。
