# 当前检查点：2026-10-06 清理已完成

本轮完成 **10/10**，源码 `4434be4992e4c23381ff911e8bc50e24a69dc881`，分支 `codex/architecture-refactor`。没有待接回的 Cargo/PG/容器任务；三名 subagent 已交回文档并停止修改。R01—R06 仍为 **2/6**，本轮清理不代表整体项目验收完成。

恢复时先读：

1. [本轮记录与最终验证表](cleanup-2026-10-06.md)、[10 项 checklist](checklists/cleanup-2026-10-06.md)。
2. [四份 HANDOFF](handoffs/2026-10-06/README.md) 与 [证据索引](handoffs/2026-10-06/evidence-index.json)。
3. [六轮后续 checklist](checklists/next-round.md)、[延期台账](deferred.md) 和实际 Git 状态。

最终输入共 619 项，manifest `a5264bb79689db17b84934389094fea9a26945c3a5be5e56e81e067f876f03a0`；逐项 Git blob 匹配。macOS 默认 1051/0/5、PTY 3、Embedded 无默认 240/0/0；Linux ARM64 定向 669/0/3、两端默认 release/link/run、无默认全部目标通过。真实模型两项本轮未重跑。v1/v2 失败记录仍保留，不能混合计数。

当前契约是 VoiceSession v2 / Playback v3 / Body v1，文本/音频必须有输出身份。Embedded 的 `speech.channel_id` 必须匹配 Server 实例 ID；旧 mode、edge_stt、mixer、TranscriptCorrection、无身份输出与 type alias 已移除。使用新示例配置；不恢复旧兼容层。

下一步按用户选择推进 R02 AEC/实机声学、R03 执行器/资产/反馈、R04 剩余管理增强或 V01 有界定位；清理本身没有未完成源码项。明确输入会话关闭契约、前端、通用 Embedded 适配与端上模型产品化仍开放。保持默认 Shadow 和当前政策，不从软件通过推断硬件授权或物理效果。

本轮独立数据库与 Linux 容器均已停止，任务库无剩余测试 schema/连接；用户 5433 未访问。没有推送或创建 PR。
