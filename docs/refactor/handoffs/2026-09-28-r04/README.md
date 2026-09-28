# R04 · 2026-09-28 验收与交接入口

本包已完成并正常本地提交 **`8f5bb83`**：HTTP新装配置、Companion REST规范、在线健康期限，以及设备授权/实际状态4组修复；MG01—MG05全部关闭。633个已提交输入逐项等于最终V4，无额外/缺失输入。

- [Root / Runtime](runtime.md)：本机setup边界、成功响应排空、整包验证、提交及资源。
- [API](api.md)：Companion OpenAPI、终端CAS与Linux编译。
- [Setup](setup.md)：完整模板、实际配置语义、原子保存与CLI分片布局。
- [Device / Health](device-health.md)：当前owner/admin门、设备status、真实Provider有限期限与证据边界。

最终结果：[验收摘要](validation-summary.json)、[源码核对](committed-source-verification.json)、[逐目标测试计数](workspace-v4-counts.json)。workspace **1053/0/5**，真实PTY3、实际Easy-Turn2，合计 **1058 unique macOS tests**；macOS/Linux ARM64两端no-default all-targets、fmt通过，Linux仅编译。任务PG清理核对为空，Linux V4容器已停止，原缓存/快照保留。

V1测试编译失败、V2 refused夹具超时、V3旧配对3项超时及其后同binary9项通过均在 [前期记录](preliminary-failures.json) 保留；V4只增加配对失败诊断并完成全量通过，没有证明修复了首次超时根因，后续按台账V01跟踪。

执行清单见 [R04 checklist](../../checklists/management.md)，恢复总入口为 [2026-09-28检查点](../../resume-checkpoint-2026-09-28.md)。R01的630输入与1041个macOS测试独立保留在 [R01交接](../2026-09-28/README.md)，不混合累计。
