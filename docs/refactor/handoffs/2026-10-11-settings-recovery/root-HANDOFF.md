# Settings recovery Root HANDOFF

基线 `1d6ef37`，分支 `codex/architecture-refactor`。三个 subagent 分别负责 storage、HTTP flow、regression；Root 负责恢复上下文、范围决策、交叉审阅、统一 Cargo／数据库窗口、证据和提交。

先审计发现：原 Settings 已有 SQL CAS 与同值显式重试，缺口为原地截断写文件；旧 Memory privacy 则只改内存，未提交实际 SQL 政策。用户明确确认本轮把 privacy 修改统一为管理员并接入同一 SQL 流程。运行时保存沿用已说明的推荐默认部分成功与显式重试，和首次 setup 的同次发布恢复记录分别处理。

存储归 Settings 域；HTTP 只处理既有应用流程和安全错误报告；policy helpers 留在 API 内，未创建跨 SQL／模块／文件的事务框架。实际细节见 [技术说明](../../settings-recovery-2026-10-11.md)。

Root 审查补上非 regular 文件在 write-open 前拒绝，以及 dangling symlink 指向缺失父树时保持原失败边界。三个分工完成并停止源码修改后冻结 V1；最终测试、资源状态与提交信息在下方收口时补充。

下一候选是纯非政策 PATCH 等待更新锁／政策查询期间 actor 降权后的重新核验。现有 SQL policy 事务已检查当前 active admin，但不能据此宣称整个 settings 修改流程都有相同保障。

恢复时先读本轮 checklist 和三份分工 HANDOFF，核对 Git 状态与证据版本。新一轮修改必须建立自己的输入清单，不能覆盖本轮日志或只读快照。R01—R06 的真机出口保持原状态，不能由本轮软件验证代替。

## Root 最终验收补记

源码 `2f5e2a2267c8499a116e652a4779f111ddbe5aff`；冻结 V1 manifest `c2fc158414333488da098f06050c4daf681477aef50fbdda193b82b111744312`，653/653 源码 Git blob 一致。

macOS ARM64 workspace **1120/0/5**；Linux ARM64 Server 默认并行完整回归 **838/0/5**；两端 Server 无默认 feature 全目标检查、架构边界正反自测、变更范围 rustfmt 和差异检查通过。数字为 passed/failed/ignored，嵌套子进程摘要不重复累加。全部最终阶段使用同一份源码，没有源码漂移。本轮新增 12 项（7 storage、2 Registry、3 HTTP/runtime）在两端通过；本轮测试阶段均首轮通过，测试期限保持原值。

全部 Cargo 阶段结束。本轮独立 PostgreSQL 与 Linux holder 已停止，关闭前其他 DB 连接为零，测试 schema 为零，遗留 schema 清理数 0；用户 5433 未访问。日志、源快照及缓存保留在 ignored `run/validation-20261011-settings-recovery/`，可核对副本见证据索引。空闲 holder 的停止码不作为 Cargo 结果。

本轮总 checklist **10/10**，L09—L12 仍 **4/4**，R01—R06 仍 **2/6**，不是项目完成百分比。前文实施时点的未运行说明保留，实际整轮结果以本节为准。

未运行真实断电、独立进程 kill/restart、真实 PTY／Easy-Turn、Linux 全 workspace、release／部署 smoke、外部付费服务、Pi／声学／执行器或长稳。存储回归使用软件故障注入，runtime 重启使用实际 ServerBuilder；不等于操作系统进程重启或物理断电。ACL／扩展属性迁移、异 owner 拒绝的提权实测、FIFO 故障和其他本机进程并发改写没有验证。
