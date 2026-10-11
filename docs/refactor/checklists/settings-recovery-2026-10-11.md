# 运行时设置持久化与恢复 checklist

基线 `1d6ef37`，分支 `codex/architecture-refactor`。本轮 **10/10 完成**；L09—L12 仍 4/4，R01—R06 仍 2/6，不换算为项目完成百分比。

- [x] **SR01 审计**：读取上一轮四份 HANDOFF，核对 Registry、所有 save 调用者、HTTP、启动加载与既有回归。
- [x] **SR02 契约**：明确 SQL、desired RAM、模块 live apply、文件保存的部分成功与显式重试边界。
- [x] **SR03 保存**：替换原地截断写入，完整快照、独占候选、原子发布、文件与目录同步、错误阶段和候选清理。
- [x] **SR04 权限与并发**：同 Registry 保存串行，保留已有目标权限约束与符号链接语义，普通加载只读。
- [x] **SR05 旧入口**：按用户确认将 Memory privacy PATCH 迁移到同一 SQL 政策与管理员修改权限，读取返回权威政策，删除 RAM/save 旁路。
- [x] **SR06 恢复回归**：发布前失败、发布后同步失败、重复 PATCH、模块部分失败、权限/CAS、重新加载与重启。
- [x] **SR07 macOS**：冻结最终源码，workspace 回归和 Server 无默认 feature 全目标检查。
- [x] **SR08 Linux**：相同源码 ARM64 Server 回归和无默认 feature 全目标检查。
- [x] **SR09 交接**：技术说明、四份 HANDOFF、可核对的命令与日志、延期台账及恢复检查点。
- [x] **SR10 收尾**：资源关闭、输入与提交身份核对、正常提交、工作区干净。

保留 HTTP 响应形状、现有 SQL CAS、shadow、工具可见性及执行批准；Memory privacy 的管理员修改权限已由用户明确确认。不增加依赖、平台、外部付费探测或硬件动作；验证仅使用本轮隔离数据库，不访问用户 5433。

源码 `2f5e2a2267c8499a116e652a4779f111ddbe5aff`；冻结 V1 manifest `c2fc158414333488da098f06050c4daf681477aef50fbdda193b82b111744312`，653/653 源码 Git blob 一致。

macOS ARM64 workspace **1120/0/5**；Linux ARM64 Server 默认并行完整回归 **838/0/5**；两端 Server 无默认 feature 全目标检查、架构边界正反自测、变更范围 rustfmt 和差异检查通过。数字为 passed/failed/ignored，嵌套子进程摘要不重复累加。全部最终阶段使用同一份源码，没有源码漂移。本轮新增 12 项（7 storage、2 Registry、3 HTTP/runtime）在两端通过；本轮测试阶段均首轮通过，测试期限保持原值。

全部 Cargo 阶段结束。本轮独立 PostgreSQL 与 Linux holder 已停止，关闭前其他 DB 连接为零，测试 schema 为零，遗留 schema 清理数 0；用户 5433 未访问。日志、源快照及缓存保留在 ignored `run/validation-20261011-settings-recovery/`，可核对副本见证据索引。空闲 holder 的停止码不作为 Cargo 结果。

下一软件候选：先审计纯非政策 `PATCH /api/v1/settings` 在等待更新锁／政策读取期间发生管理员降权时的准入重验证，保留现有 SQL actor 核验、API 与权限政策；先复现，再补缺口。

恢复入口：[当前检查点](../resume-checkpoint-settings-recovery-2026-10-11.md)。
