# V01 外层 45 秒与失败清理边界 — Root HANDOFF

日期 2026-10-15，分支 `codex/architecture-refactor`，基线 `99c946d`，源码提交 `a73e6e8bc37e4103cb5fc0ffef2aa31d809ff3d6`。本包 **10/10 完成**，只修复测试夹具已确认缺口；历史 V01 25 秒超时根因仍开放。默认 shadow、当前政策、执行批准、精确取消、R03 暂停、无新依赖/硬件均保持。

## 范围与已确认缺口

调用范围为 `delivery_support::{with_db,with_db_context}`、`duplex_playback::{RpcServer,Runtime,with_scene}` 和 Companion 配对 fixture；新增两份测试模块。未改生产源码、配置、依赖、功能开关或导出。`with_db` 被多个集成目标共用，因此最终验证覆盖 workspace，而非仅新测试。

1. 原 RPC `close` 把 JoinHandle 从 Drop guard 移走再按值交给 3 秒 timeout；超时或取消 close 会 detach。现在 timeout 借用原 handle，显式超时先 abort/join 再报告原失败；close 被丢弃时 guard 仍可 abort。
2. 原外层 45 秒 drop 整个 callback，跳过显式清理。新增 context 变体将部分资源保留在 timed body 外，初始化 channel 前保存 paired owner，ServerBuilder 等待前保存现有 ShutdownHandle，无生产框架变更。
3. body 与 context cleanup 使用同一绝对 45 秒成功截止。截止后仍等待同一个 cleanup future 排空，最终保持失败，不重建或刷新截止。Fixture 构建和最终 DB teardown 仍在原计时范围外，45 秒从来不是进程总寿命保证。
4. 清理收集同步 revoke、heartbeat、ServerRuntime、voice RPC、paired owners 的错误，尝试后续阶段，保留原 body panic。heartbeat 队列发送观察取消，避免停在满队列发送。数据库关闭前等待已有 ServerRuntime cleanup owner 结束。
5. paired Delivery 若首次 shutdown 未完成，则保留 owner/DB，释放其它资源后等待其结束并收集原 join；首次失败仍保留。极端生产 supervisor panic 可能使现有 running 标志不复位，此时不能把 watchdog 终止算成已清理；本轮未新增生产 completion API。

## 有界验证过程

- 旧 RPC 实现定向 RED：1 passed / 2 failed，默认并发，原 3 秒虚拟计时。两项失败为 listener 未退出/close 取消丢失任务；fallback 负责回收 RED 故意暴露的原任务。
- 首版完整 duplex：18 passed / 1 failed，13.55 秒。新真实 PG 外层用例推进 Tokio 时钟时伴随运行时清理失败，无法把混合时钟的结果解释为生产缺陷；保留断言，将该用例改成原 45 秒真实等待。纯控制流/RPC 用例继续使用已有 test-util，真实 I/O 不使用虚拟时间。
- 独立审查修复了初版把 context cleanup 移出成功期限的问题，并新增 44 秒 body + 2 秒 cleanup 回归：即使完成排空也必须报告 45 秒失败；原 panic 优先，cleanup 只构造一次。
- 第一份 workspace 验证因发现测试假阳性而主动停止：73.33 秒，exit -15；部分已运行目标不累计通过数。pre-DB 断言可能被原失败优先规则遮蔽，现在只有全部断言成功才写独立标志，最外层必须验证该标志。中断留下 2 个测试 schema、0 个连接，已记录后仅重建任务隔离数据库，重建后 0/0。
- 修订版最终 workspace **1256 passed / 0 failed / 5 既有 ignored / 0 filtered**，347.91 秒；67 Running + 5 Doc-tests。duplex **21/0/0，45.60 秒**，含本轮新增 11 项（RPC 3、纯控制流 6、真实 owner 2）。5 项既有 ignored 为 PTY 3 和 Easy-Turn 2，本轮没有新增忽略。源码清单 671 个输入，SHA-256 `021b2fb3d46d64ddd98c2344cb078647b66bc143dc1f41d4b5d1326e19b6a6d2`。所有追加运行均因具体修订，不重复历史随机场景求绿。

## 证据边界

真实 owner 回归检查：原 45 秒失败/原 panic 保留；heartbeat 已退出；ShutdownHandle finished；runtime tasks=0；Delivery 无 pending；playback inactive；目录已删除；voice listener 可重新绑定；这些检查成功时工作数据库仍开放，随后关闭。纯控制流验证 context 排空在 DB teardown 前，RPC 回归验证 listener 所有者真实退出。

外层不新增 SQL 诊断预算；保留固定阶段信息。原内层失败的 1 秒脱敏诊断、25 秒业务期限、32 秒其它配对期限、3 秒 RPC 期限、CASES=3 与默认 libtest 并发不变。进程 watchdog 是失败上限，不是延长任何测试成功期限。

未动态注入每一种部分初始化失败、native mutex poisoning 或永不退出 supervisor；paired.close 的 late-drain 分支未强制制造，既有 Delivery 取消后保留 owner 回归只证明底层机制；源码保证与动态验证分开陈述。不验证 Linux/Pi、真实模型、录放音、执行器、长期负载。R02 硬件资料仍待，R03 暂停，R05 现场门槛不变。

## 分工与恢复

Root 负责 Companion/paired context 接入、统一验证、资源证据与提交；[Flow](flow-HANDOFF.md) 负责公共夹具和期限；[Fixture](fixture-HANDOFF.md) 负责 RPC 复现与修复；[Review](review-HANDOFF.md) 独立检查。三个 subagent 均完成：Flow 7/7、Fixture 6/6、Review 5/5；独立复核无阻断，已停止。

## 最终资源与交付身份

完整验证结束后 56.106 秒采集真实 SQL 快照：全部非系统/非 public 测试 schema 为 0，其他 client 为 0；期间未重启 PostgreSQL。不是退出瞬间的快照。随后按完整 ID/label 移除任务容器及卷并核对不存在，未找到相关测试进程；共享 OrbStack/astrbot 没有变更。

671 个当前输入与源码提交 blob 全部匹配，限定 Rust 格式和源码 diff 检查通过。最终配对 binary 110661120 字节、SHA-256 `bc2c7cb8e1291aa60547963d56e45071703ec8df68c9b4e04e2623d812865184`，保留于 ignored run 目录；[证据索引](evidence-index.json)归档 30 份原始日志/JSON/脚本。查看[最终汇总](evidence/validation-summary.json)、[提交身份](evidence/commit-verification.json)、[资源快照](evidence/final-resources.json)、[资源回收](evidence/cleanup.json)与[过程台账](evidence/run-ledger.json)。

原始日志末尾空行按字节保留；归档不是新源码，不能改写后还声称原件哈希一致。恢复从[本轮检查点](../../resume-checkpoint-v01-outer-2026-10-15.md)继续，不重复本轮已完成验证。R01—R06 仍 2/6。
