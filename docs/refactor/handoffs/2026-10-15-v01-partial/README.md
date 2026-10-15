# V01 部分初始化与 paired late-drain 交接

2026-10-15，分支 `codex/architecture-refactor`，基线 `af5b35b`，源码 `dc3ec17`；本包 **10/10 完成**。历史 V01 根因仍开放，R03 暂停，六轮仍2/6。

四份分工：

- [Root](root-HANDOFF.md)：范围、真实paired部分初始化、验证、资源与恢复。
- [Initialization](initialization-HANDOFF.md)：schema遗留RED和统一初始化失败清理。
- [Drain](drain-HANDOFF.md)：原native write guard、10秒关闭预算及迟到排空。
- [Review](review-HANDOFF.md)：独立所有权、断言与最终证据审查。

一次RED **0/3**确认缺口；定向 **6/0**；最终一次workspace **1262/0/5**（既有ignored、0filtered），duplex **27/0/0**。674提交输入匹配。结束后约0.584秒同库资源0/0，随后任务容器/卷已删除。默认shadow、政策、原期限及并发不变。

从[checklist](../../checklists/v01-partial-2026-10-15.md)和[恢复检查点](../../resume-checkpoint-v01-partial-2026-10-15.md)继续。查阅[最终摘要](evidence/validation-summary.json)、[24份原件索引](evidence-index.json)、[提交核对](evidence/commit-verification.json)、[资源时点](evidence/resource-timing.json)及[回收](evidence/cleanup.json)。大二进制仅保留在ignored `run/validation-20261015-v01-partial/`；归档脚本不是另建的测试框架，保留原run目录假设。

本轮不覆盖整个建库future被外部任意取消、全部startup阶段、native锁中毒或永不退出supervisor。不能由这些注入回归倒推历史25秒超时根因，也不能用watchdog终止证明清理成功。
