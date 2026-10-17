# 插件契约与信任边界交接

2026-10-17；起点`89d04a2`，应用源码仍`6c62c84`。本轮只做源码／证据审查、契约和资料清理，没有新的运行时验收。

| 交接 | 责任与阅读重点 |
| --- | --- |
| [Root](root-HANDOFF.md) | 范围、集中决策、统一契约、静态验证和接续顺序 |
| [Server](server-HANDOFF.md) | 管理鉴权、存储scope、现有Execution身份／批准及测试复用 |
| [Embedded](embedded-HANDOFF.md) | 包／实例／命令身份、发布和进程清理、两端协议影响 |
| [独立审查](review-HANDOFF.md) | 表达ABI、私有输入、行业资料、隔离与效果信任边界 |

规范入口为[插件契约](../../../architecture/plugin-contract.md)，进度见[checklist](../../checklists/plugin-contract-2026-10-17.md)和[恢复检查点](../../resume-checkpoint-plugin-contract-2026-10-17.md)。D01—D03均已由用户确认，设计包10/10；运行时实现仍为后续0/4，不能把目标契约当作已生效机制。草案`826dbf0`及旧静态记录保留原时点，定稿检查另行归档。

[证据索引](evidence-index.json)只记录本轮静态检查及已有证据的来源边界，不表示运行插件或验证隔离有效。PA01—PA11均待后续实际实施／验收；未审核第三方代码安装和隔离执行是明确保留的目标。
