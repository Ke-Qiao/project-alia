# R03-0 — 停止保持、校准启动与 PCA 初始化恢复

日期：2026-10-14。源码提交 `b5100d778a53dda9f3adc55fd0309a5eb5c853e9`，起点 `125ea58`，分支 `codex/architecture-refactor`。本包软件 checklist **10/10**；R01—R06 仍 **2/6**，不将软件回归记作实机通过。

四份 HANDOFF：

1. [Root](root-HANDOFF.md)：范围、集成结果、证据和下一任务。
2. [Lifecycle](lifecycle-HANDOFF.md)：停止保持、校准门槛、旧配置清理及所有权。
3. [PCA](pca-HANDOFF.md)：AI、SLEEP/失败恢复、多板构造及寄存器模型。
4. [Review](review-HANDOFF.md)：独立源码/测试/证据核对和平台限制。

[Checklist](../../checklists/r03-0-2026-10-14.md) · [恢复检查点](../../resume-checkpoint-r03-0-2026-10-14.md) · [验证摘要](evidence/validation-summary.json) · [证据哈希](evidence/evidence-index.json)

先在旧实现上复现 5 项预期失败，再验证 macOS ARM64 默认 features 的 Embedded 全 targets：**263 passed / 0 failed / 0 ignored**（lib 199，integration 64，含 PCA 14 项）。657 个测试输入与源码提交逐项一致。Embedded 格式通过；workspace 格式检查的失败为基线未改 Server import 排序。Linux daemon 停止，本轮未运行 Linux；不复用历史通过结论。

原始日志、RED patch 与失败元数据按字节保留。差异检查仅对原始 `.log` 的空 EOF、以及 patch 数据中的上下文空格作路径限定豁免，不修改这些证据。脚本保留当时 `run/validation-20261014-r03-0` 布局，不应直接从本 evidence 目录运行。UTC 时间戳对应上海 2026-10-14。

未连接 Pi、未打开真实 PCM/I²C/UART、未部署或新增依赖。校准确认仍为操作者声明；中心/恢复事务提交不证明机械位置或保持。
