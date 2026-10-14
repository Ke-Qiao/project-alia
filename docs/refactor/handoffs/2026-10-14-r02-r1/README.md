# R02 R1 硬件 AEC 路由与 consumer 前置核对

日期2026-10-14；基线 `9890bab`，分支 `codex/architecture-refactor`。前置包10/10完成，本轮仅文档和读取证据，音频实现仍为 `28920fb`；R01—R06仍2/6，R03暂停。

- [Root](root-HANDOFF.md)：范围、集中问题、文档整合与提交。
- [Hardware](hardware-HANDOFF.md)：官方公开资料、参考接口及缺失规格，6/6。
- [Host](host-HANDOFF.md)：采播/speech、实际声道语义与单 owner，6/6。
- [Review](review-HANDOFF.md)：consumer条件与独立证据复核，6/6。

先读 [前置记录](../../r02-r1-aec-preflight.md)、[checklist](../../checklists/r02-r1-2026-10-14.md)和[恢复检查点](../../resume-checkpoint-r02-r1-2026-10-14.md)。当前没有选定或创建主机 AEC，R1 实物、固件和参考电气仍待补；已提出硬件优先建议，等待用户决定。

证据为 [来源元数据](evidence/source-retrieval.json)和[静态检查](evidence/validation.json)。前者记录6次成功官方公开读取；完整厂商正文/图片在ignored `run/validation-20261014-r02-r1/`，不整篇复制入仓库。后者核对来源字节、源码/依赖未变、变更文档链接及diff格式。没有运行Cargo、实际CLI、设备、SSH或模型，上一轮335/260测试只保留为历史证据。
