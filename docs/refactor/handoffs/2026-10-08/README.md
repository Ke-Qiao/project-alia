# 2026-10-08 分层重构交接

本轮目标：Memory 迁出 provider 层，Interaction 接管旧 Pipeline 的用例，Bootstrap / Platform 分离装配与生命周期。基线 `adb28c1`，分支 `codex/architecture-refactor`。

入口：[本轮技术说明](../../layering-2026-10-08.md)、[checklist](../../checklists/layering-2026-10-08.md)。后续执行读取四份交接：

1. [Root：整合、验证、提交与续接](HANDOFF-root.md)
2. [Memory：具体服务、纯域类型和采集](HANDOFF-memory.md)
3. [Interaction：轮次阶段、去重与缓存取消修复](HANDOFF-interaction.md)
4. [Boundary：Bootstrap / Platform 与架构守卫](HANDOFF-boundary.md)

最终验证针对 `source-v4-manifest.json` 的 **630 个输入**，SHA-256 为
`ccfaa7981fc7062dfeb43e568e52af038915064a2187352e0891980afc6fcbd3`。
v1 是初始定向通过版本，v2 是新增取消回归的修复前失败版本，不能与 v3/v4 混为同一源码证据。V3 Linux 的 Appraisal 期限竞争失败和同源复核仍保留；V4 只修正该测试夹具，629 项其它输入不变。

默认 Shadow、持久政策与真实硬件边界保持；本轮没有录音、播放、执行器动作、真实外部模型调用或用户 5433 数据库访问。正式结果、资源关闭与源码提交见 Root 交接；历史 2026-10-06 证据保持原样。

本轮 **8/8**，源码 `978af9c942da42aa3cd98182cde3dafa6a6e33be`；[当前恢复入口](../../resume-checkpoint-2026-10-08.md)、[数字状态](../../resume-state-2026-10-08.json)、[证据索引](evidence-index.json)。

证据目录保留各阶段实际命令、原始日志、环境和源码清单；`tools/` 中的运行包装脚本是原工作目录快照，路径与任务资源按该次执行记录，不是重新创建服务的入口。测试计数器可直接针对归档日志和 result JSON 复核。

原始日志按字节保存，包括 PTY 的 CR/ANSI 和 Cargo 末尾空行，因此归档日志不作空白规范化；最终文档差异检查排除这些 `.log` 文件。源码的 fmt / diff check 已在冻结验证中通过。
