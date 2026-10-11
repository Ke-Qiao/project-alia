# 当前检查点：2026-10-11 运行时 Settings 恢复完成

后续身份重验证轮已完成，最新恢复从 [Settings 身份检查点](resume-checkpoint-settings-authorization-2026-10-12.md) 开始；下文保留上轮历史状态。
分支 `codex/architecture-refactor`，本轮 **10/10**，L09—L12 **4/4**，R01—R06 仍 **2/6**，不是项目完成百分比。三个 subagent 已完成交接，无待恢复 Cargo／PG 作业。

依次读取 [技术说明](settings-recovery-2026-10-11.md)、[checklist](checklists/settings-recovery-2026-10-11.md)、[四份 HANDOFF](handoffs/2026-10-11-settings-recovery/README.md) 与 [证据索引](handoffs/2026-10-11-settings-recovery/evidence-index.json)，再核对 branch/status/log。

源码 `2f5e2a2267c8499a116e652a4779f111ddbe5aff`；冻结 V1 manifest `c2fc158414333488da098f06050c4daf681477aef50fbdda193b82b111744312`，653/653 源码 Git blob 一致。

macOS ARM64 workspace **1120/0/5**；Linux ARM64 Server 默认并行完整回归 **838/0/5**；两端 Server 无默认 feature 全目标检查、架构边界正反自测、变更范围 rustfmt 和差异检查通过。数字为 passed/failed/ignored，嵌套子进程摘要不重复累加。全部最终阶段使用同一份源码，没有源码漂移。本轮新增 12 项（7 storage、2 Registry、3 HTTP/runtime）在两端通过；本轮测试阶段均首轮通过，测试期限保持原值。

全部 Cargo 阶段结束。本轮独立 PostgreSQL 与 Linux holder 已停止，关闭前其他 DB 连接为零，测试 schema 为零，遗留 schema 清理数 0；用户 5433 未访问。日志、源快照及缓存保留在 ignored `run/validation-20261011-settings-recovery/`，可核对副本见证据索引。空闲 holder 的停止码不作为 Cargo 结果。

Settings 域独占保存 gate，准入后取快照，私有候选→file sync→原子 rename→完整 canonical 父链 sync；发布前失败保旧，发布后失败明确持久性未确认。保留 desired／module／SQL 部分成功和同值显式重试。运行时设置没有 setup 式 journal；失败后重启加载完整 old/new 文件，SQL 是政策权威。首次 setup 原受约束恢复不变。普通加载只读；硬链接／无法保持 owner/group 的替换拒绝，不提权。

用户明确确认本轮 privacy PATCH 统一 SQL 政策和管理员修改权限；GET 登录权限、请求字段和原 8 字段响应保留。两个 PATCH 共享 SQL current／If-Match／CAS，privacy GET/audit 不再读取陈旧 RAM。SQL 失败不更新内存投影，privacy 不再保存被过滤的政策文件。

**下一软件候选：先审计纯非政策 `PATCH /api/v1/settings` 在等待更新锁／政策读取期间发生管理员降权时的准入重验证，保留现有 SQL actor 核验、API 与权限政策；先复现，再补缺口。**

V01、R02 AEC、R03 反馈、Pi/24h/R05、R06、前端和可信文件系统部署等暂缓项保持开放，见 [台账](deferred.md)。不要由本轮软件通过推断真机完成。

未推送、未建 PR，未使用 reset 卡。

未运行真实断电、独立进程 kill/restart、真实 PTY／Easy-Turn、Linux 全 workspace、release／部署 smoke、外部付费服务、Pi／声学／执行器或长稳。存储回归使用软件故障注入，runtime 重启使用实际 ServerBuilder；不等于操作系统进程重启或物理断电。ACL／扩展属性迁移、异 owner 拒绝的提权实测、FIFO 故障和其他本机进程并发改写没有验证。
