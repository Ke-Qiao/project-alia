# 配置目录持久性与发布恢复 checklist

基线 `7165ad6`，分支 `codex/architecture-refactor`。本轮 **10/10 完成**；L09—L12 仍 4/4，R01—R06 仍 2/6，不是项目完成百分比。

- [x] **CD01 审计**：恢复上一轮四份 HANDOFF，核对 base/fragments writer、loader、HTTP/CLI 完成状态、启动发现和既有回归。
- [x] **CD02 契约**：区分尚未发布与提交已开始但持久性未确认；同一发布可核对后恢复，新内容/路径不能绕过 pending。
- [x] **CD03 配置所有权**：发布与恢复归配置域，OOBE 只保留模板和交互，删除旧无条件失败回滚。
- [x] **CD04 文件与目录**：候选、fragments、目录树和完整 canonical 祖先目录链按顺序同步；原子发布后同步父目录，权限与 operator 文件保持。
- [x] **CD05 恢复记录**：固定相邻 pending 描述文件约束候选与快照；同 intent 重试与重启恢复，不接受外部改动后的盲覆盖。
- [x] **CD06 交互回归**：HTTP 原状态码/JSON、CLI 单赢家、pending 拒绝新写、Ready/取消/drain 语义和失败清理。
- [x] **CD07 macOS**：统一冻结源码，完整 workspace 与无默认 feature 全目标检查。
- [x] **CD08 Linux**：同源 ARM64 Server 完整默认回归与无默认 feature 全目标检查。
- [x] **CD09 交接**：技术说明、四份 HANDOFF、原始证据、延期台账与恢复检查点。
- [x] **CD10 提交收尾**：正常源码/记录提交，输入与证据身份核对，任务资源关闭，工作区干净。

保持既有 API、Shadow、工具可见性和 Execution 批准；正常无恢复记录的配置继续只读加载，不增加目录同步权限要求。无新依赖、平台、硬件或外部服务；不访问用户 5433。只在隔离目录模拟同步失败，不能把进程重启或 kill 等同真实断电。

预记录边界：marker 建立前的崩溃可能留下无法确认所有权的临时文件/CLI fragment tree，不自动删除 operator 数据；同 UID 其他进程改变文件树的部署信任问题继续单列。运行时 `settings.toml` 的写入不是本轮 base/fragments 发布事务，另行审计。

源码 `095499676496fba79853532b2ad4f570b368d37c`，V3 manifest `11373f598d31148c27b15bfa1fc5bb015e3b2726f877347560b23bbcc1b9343f`，650/650 Git blob 一致。

macOS workspace **1108/0/5**；Linux ARM64 Server 默认并行完整回归 **826/0/5**。两端无默认 feature 全目标检查、架构边界正反自测、变更文件格式检查、计数器 8 例自测通过。数字为 passed/failed/ignored；嵌套子进程摘要不重复累计，最终阶段同源、无源码漂移。

全部 Cargo 阶段结束；关闭前其他数据库连接为零，最终测试 schema 为零，遗留 schema 清理数 0。本轮 V1/V2/V3 Linux holder 与独立 PostgreSQL 已停止；用户 5433 未访问。空闲 holder 的停止码不作为 Cargo 测试结果。缓存、只读快照保留在 ignored `run/validation-20261011-config-durability/`。

下一软件候选：先审计 `SettingsRegistry::save` 与 `PATCH /api/v1/settings` 的 live apply、内存配置及 `settings.toml` 持久化顺序，定义保存失败与重试的恢复语义，再在授权范围补缺口。保持 API、当前政策和权限；先检查既有实现，不直接扩展本轮发布协议为通用事务框架。

恢复入口：[本轮检查点](../resume-checkpoint-config-durability-2026-10-11.md)。
