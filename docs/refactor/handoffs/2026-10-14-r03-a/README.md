# R03-A 交接与证据

基线 `2b30dd6`；分支 `codex/architecture-refactor`。本包为可信动作内容绑定及用户确认的提示音写入绕过修复；真实硬件、平台扩张与新依赖不在范围内。

- [Root](root-HANDOFF.md)：统筹、用户决定、验证、资源与提交。
- [Core / Server / transport](contract-HANDOFF.md)：内容身份、签名与原意图恢复。
- [Embedded](embedded-HANDOFF.md)：同次读取、准备所有权与本地配置。
- [独立审查](review-HANDOFF.md)：信任边界、RED 与同源 GREEN 复核。

实现提交`e7265f0d0e9be8d6e663bfcabd53739b32206e27`；本包10/10，R01—R06仍2/6。最终macOS默认回归1207/0/5；661个提交blob与冻结输入一致。详见[验证汇总](evidence/validation-summary.json)、[提交核对](evidence/commit-verification.json)、[资源收口](evidence/cleanup.json)及[证据索引](evidence-index.json)。

当前以 [checklist](../../checklists/r03-a-2026-10-14.md) 和 [恢复入口](../../resume-checkpoint-r03-a-2026-10-14.md) 的阶段状态为准。

证据保留最初3个业务RED、cue越界RED、数据库停止导致的候选中止和清理共享服务状态断言，不覆盖失败。候选patch基于2b30dd6；额外untracked assets.rs单独保存，最终版本由实现提交+green manifest定位。没有Linux应用/真实硬件验收。

原始 `.log` 与 `.patch` 按字节归档，不清理空行或 diff 上下文前缀；因此全量 Git whitespace check 会在这些证据文件提示空白。排除该原始证据目录后，两次提交的源码/文档 whitespace check通过。归档脚本保留原run目录的相对路径假设，作为执行记录保存，不应从归档目录直接执行容器启动/清理脚本。
