# R02／R03 现场前置交接

日期：2026-10-14；基线 `283f38f`；分支 `codex/architecture-refactor`。承接已完成的 R03-0、R02-A、R03-A，仅补检查入口和前置操作材料。

- [Root HANDOFF](root-HANDOFF.md)：实现整合、验证、边界与提交。
- [Audio HANDOFF](audio-HANDOFF.md)：当前音频配置、选择语义与资料清理。
- [Motion HANDOFF](motion-HANDOFF.md)：纯文件检查、复用验证及信任边界。
- [Deployment HANDOFF](deployment-HANDOFF.md)：staging、启动副作用与独立审查。

状态见 [checklist](../../checklists/field-prep-2026-10-14.md)，继续工作从 [恢复检查点](../../resume-checkpoint-field-prep-2026-10-14.md) 开始。实际硬件和部署值集中记录在[现场资料](../../../operations/hardware-deployment-readiness.md)，不把旧盘点当作本轮实测。六轮仍 2/6。

本包 **10/10**；实现提交 `e409dc944a589104d0e2d2c2977f642ad8aaef28`。默认 Embedded **316/0/0**，无默认 CLI **5/0/0**，两示例离线检查通过；665 个提交 blob 与最终冻结输入一致。见[验证汇总](evidence/validation-summary.json)、[提交核对](evidence/commit-verification.json)及[证据索引](evidence-index.json)。真实硬件、Linux/Pi、模型、staging 普通服务启动和 R05 长稳均不在本轮执行范围。

首轮命名为 green 的候选运行实际 exit101，原因是新增测试夹具缺少既有必填 identity 字段；其原日志、夹具、补丁和输入清单完整保留，不算业务 RED 或最终通过。原始 `.log`/`.patch` 按字节归档，不清理证据空白；源码/文档 whitespace 核对排除该 evidence 目录。归档脚本保留原 run 目录相对路径，属于执行记录，不从本目录直接运行。

用户允许Mac模拟后，本轮另实际运行现有无默认features合成PCM示例，exit0、4.66s：7条记录、16个样本值、9个重置提示，保留2个丢失播放块、独立时钟未对齐和旧采集流结束状态unknown。文件标记SyntheticFixture，physical_stream_complete=false；只是离线证据演练，不是真实录放音/AEC/模型验收。原始fixture、输出与hash见本包evidence。
