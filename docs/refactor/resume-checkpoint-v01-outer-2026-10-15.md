# 恢复检查点：V01 外层 45 秒与失败清理

2026-10-15；分支 `codex/architecture-refactor`；基线 `99c946d`。源码 `a73e6e8`，本轮 **10/10 完成**，三个分工 7/7、6/6、5/5；历史 V01 根因仍开放，R03 暂停，R01—R06 仍 2/6。

先读 [checklist](checklists/v01-outer-2026-10-15.md)、[四份 HANDOFF](handoffs/2026-10-15-v01-outer/README.md) 和 [Root](handoffs/2026-10-15-v01-outer/root-HANDOFF.md)，核对 Git 状态；不要重复已完成批次求绿。

## 已确定的契约

- 配对/Companion owners 保留在外层 body timeout 之外；保存部分初始化和现有 startup/runtime ShutdownHandle，清理结束后才回收数据库。
- body + context cleanup 共用原 45 秒绝对成功截止；超时后排空原 cleanup future，最终仍为失败。原 25 秒业务、32 秒其它 paired、3 秒 RPC、1 秒失败 SQL、CASES=3 和默认 libtest 并发不变。
- RPC 等待期间不移走 JoinHandle，超时 abort/join 后恢复原失败；清理各阶段错误不会覆盖原 body panic。不存在生产源码/配置/依赖或权限政策变动。
- 原始 RED、失败及中断运行单独保存；最终只计完整修订版，不累计重复目标。真实 PG 回归使用真实时间，纯控制流才使用现有 test-util。

## 验证与资源

最终源码清单为 671 输入，SHA-256 `021b2fb3d46d64ddd98c2344cb078647b66bc143dc1f41d4b5d1326e19b6a6d2`。完整 workspace **1256/0/5**（5 项既有 ignored、0 filtered），347.91 秒；duplex **21/0/0**，45.60 秒；671 个提交 blob 与冻结版本一致，独立审查无阻断。原 RED 1/2、初版18/1、中断-15与修订结果分别留证，不累计。退出后56.106秒快照为0测试schema/0其他client，未重启PG；任务容器/卷已移除，共享运行时/服务未改。见[最终证据](handoffs/2026-10-15-v01-outer/evidence/validation-summary.json)及[资源回收](handoffs/2026-10-15-v01-outer/evidence/cleanup.json)。

## 后续边界

1. 本轮是已确认 fixture 缺口修复，不是历史 25 秒超时根因结论。只有新的真实失败、原 V3 artifact 或可区分条件才重启有界定位，保留原机会与并发。
2. 未穷举部分初始化、native poisoning、paired late-drain 或永不退出 supervisor。生产 running 标志在极端 panic 后可能无法复位；保留 owner/DB 并报告未完成，watchdog kill 不算清理成功。Fixture 初始建库/迁移和最终 SQL teardown 仍不属于原 45 秒窗口，不预建生产通用期限框架。
3. R02 硬件 AEC 优先路线保持；R1 板/固件、厂商附件、功放/扬声器与参考电气资料仍待。R03 用户暂停，部署账户/目录/模型端点和 R05 现场出口继续见 [延期台账](deferred.md)。本轮没有 Linux/Pi、真实模型、录放音、执行器或长稳验证。
