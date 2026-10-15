# V01 ServerStartup 初始化取消 — Root HANDOFF

2026-10-15；本轮 **10/10 完成**。分支 `codex/architecture-refactor`，基线 `4ab48fe`，源码提交 `9e3a93401aea481f729b21512431b980dfc3b02c`。三个 subagent 分别负责所有权与生产修复、确定性回归、独立复审；Root 统一执行验证、资源回收和提交。

## 已确认问题与修复

原启动 task 成功后，`Ok(ServerRuntime)` 已缓存在 JoinHandle；调用方尚未消费时 parent 取消，`ServerStartup::wait` 仍返回 Ready 并设置 handed_off。这个交付检查缺口经真实 PG 的一次 RED 复现；它不等于已证明永久泄漏，更不是历史 Companion 25 秒超时的根因结论。

现在只在 `Ok(runtime)` 交付前检查取消。若取消已可见，则记录 startup 失败并复用原 RuntimeCleanup 回滚；保留 runtime 跨回滚等待。私有 `rollback_startup` 原样统一旧失败出口，不改公共 API、原 grace、typed `ShutdownIncomplete` 或原完成句柄。原初始化 Err 不被后到取消覆盖。正常成功路径不变，没有新框架或依赖。

已有保证继续复用：module.init 首次 poll 前登记 rollback Slot；按值 `wait(self)` 未 poll 就 drop 仍请求 child 取消；native stop 超过 grace 时原 ShutdownHandle 保留完成见证；真实 Companion fixture 先保存 handle，清理 join 后才关闭 DB。

## 回归与资源

新 `server_runtime/startup.rs` 复用原 Fixture、Probe 和 `delivery_support::run_context`，四项覆盖取消已缓冲成功、原 3000ms grace 后仍保 owner、未 poll waiter 丢弃及已缓冲 init/stop 失败保留。context 持原 waiter、handle、runtime、init/native gates；失败先释放 gate，再 join 原 owner，最后清数据库。没有调整全局 helper 或原 Probe。

| 阶段 | 结果 | 证据含义 |
| --- | --- | --- |
| 旧实现 RED | 0/1/0，81 filtered；目标0.61秒，命令16.09秒 | ready 已通知且取消后 returned_ready=true；先完成 worker/pool/HTTP/gRPC 回收，再触发唯一目标断言失败 |
| 一次定向 GREEN | 8/0/0，77 filtered；目标4.34秒，命令40.38秒 | 4新+4旧；取消拒交付、typed incomplete、重等与双失败保留全部通过 |
| 一次最终 workspace | **1270/0/5，0 filtered**；505.0秒 | 67 Running + 5 Doc-tests；server_runtime85/0/0（80.42秒），duplex31/0/0（45.61秒） |

5项 ignored 为既有 PTY 3项、真实 Easy-Turn 2项，没有增加或忽略失败。统计只采用最终完整运行，各 target 采用最后汇总，子进程内部结果、RED 和 GREEN 不重复计数。全过程源码无漂移，没有同版本重复求绿。

RED、GREEN 和最终退出后同任务 PG 均为0 schema/0其他client；最终快照距命令退出约0.248秒。随后按完整容器 ID 与 label 核验，只 stop/rm -v 本轮任务容器，确认不存在；共享 astrbot / OrbStack 保持。任务资源和测试进程已结束。

676项冻结输入逐 blob 与源码提交一致，清单 SHA-256 `34595bfb7a942b17a3941474ae8c2c7392caa8c95f4f2e3847fdbf997011f205`；最终测试二进制身份见 validation-summary。原始失败、成功、输入、资源、提交核对与脚本均归档至 evidence，详见[索引](evidence-index.json)。原始日志空行保留，不人为美化证据。

## 保持与边界

原25/32/45秒、RPC3秒、诊断1秒、Delivery shutdown10秒、各配置 grace、DATABASE_CASES=2、Delivery CASES=3及默认 libtest 并发均不变。默认 shadow、当前政策、执行批准和精确取消保证保持；R03 用户暂停，不新增依赖、模型/真实音频/硬件或平台范围。R01—R06 仍2/6。

**历史 V01 根因仍 open**，只在有新失败、原 artifact 或可区分条件时继续有限定位。此次没有动态证明 from_config connect 返回 pool 前的取消、SQL 未知确认、discovery 注册失败后的局部 shutdown、旧 server_runtime 全局 with_fixture 的 body 失败 owner 顺序，不能称所有初始化阶段完备。Runtime 强杀、全部完成句柄丢弃、native poisoning/supervisor panic 或永久不退出仍不作完成承诺；watchdog 不是清理保证。这些是证据边界，不冒称用户已延期。

后续软件候选：先有界核对 `Database::connect` 返回前的取消与 SQLx 已有关闭机制，证明具体缺口后才改动；不预建全局 supervisor。R02 硬件 AEC 资料门槛、R03 暂停和 R05 现场出口继续保持。见[恢复检查点](../../resume-checkpoint-v01-server-startup-2026-10-15.md)。
