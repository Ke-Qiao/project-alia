# 运行时设置持久化与失败恢复

本轮承接 `1d6ef37`。先检查已有 Registry、Settings HTTP、Memory privacy、启动加载及相关测试，再补实际缺口。当前实现和验证进度见 [checklist](checklists/settings-recovery-2026-10-11.md)。

## 保留的事务边界

Settings PATCH 已有管理员限制、SQL Context policy CAS、更新锁及同值显式重试，本轮保留。处理顺序仍为 SQL 政策提交 → desired Registry → 模块及 Interaction live apply → 非政策文件保存。模块可能先产生效果再返回错误，文件也可能在发布后同步失败，因此不能宣称 SQL、内存、各模块和文件构成同一个原子事务。

`changed` 表示期望值变化，`applied` 与 `module_reports` 表示各组件报告，`requires_restart` 和 `ignored_runtime` 保留原含义。`success=false` 不会撤销已完成动作。显式请求键即使已等于期望值，也继续应用并保存。携带 `If-Match` 的失败请求应先读取当前版本；旧 ETag 不能绕过政策 CAS。

运行时保存采用推荐默认的既有部分成功与显式重试语义。向用户说明过跨重启恢复的替代方案，本轮没有把首次 setup 的持久恢复记录复制到运行时设置。首次 setup 的受约束重试规则保持不变。

## 文件发布

旧保存直接截断 `settings.toml`。新保存由 Settings 域拥有：同 Registry 的保存串行，取得准入后才读取快照，沿用过滤 SQL 政策键和 null 值的规则。序列化完成后使用相邻独占临时文件，写完、同步后原子替换，再同步完整 canonical 父链。之前失败留下的目录不能被误认为已可靠同步。

发布前失败保留旧目标并清理本次候选；发布后错误明确报告“已经发布，但持久性未确认”。API 使用固定诊断，不回传底层文件内容、路径或凭据。没有后台保存任务、自动重试或跨域回滚。进程内显式重试重新保存当前期望快照；失败后直接重启只能读取磁盘上完整的旧版或新版，SQL 仍是政策权威。普通加载只读，先完整读取和解析，再一次合并并保留 schema。

Unix 新文件以 0600 创建；既有文件先检查写入许可，保留 mode，继续跟随目标符号链接而不替换链接本身。原子替换需要新的 inode，不能悄悄改变硬链接共享或文件 owner/group，因此检测到这些不兼容情况会拒绝保存。文件系统路径由可信服务账户管理；不增加权限、不提权、不添加跨进程文件锁。

## Memory privacy 旧入口

旧 `/memories/privacy` PATCH 只写内存投影，保存时这些键又被排除；实际生成和重启读取 SQL。因此旧成功响应并不能证明政策生效。用户明确确认本轮统一政策和管理员修改权限。

两个 PATCH 复用 API 内窄范围的政策读取、版本校验、类型验证及提交 helper，保留 2 秒 SQL 期限、400／403／409／503 分类和事务内 actor 核验。Privacy PATCH 成功后才更新投影，不再写 `settings.toml`，不调用无关 provider。请求字段、8 个成功响应字段、capture_mode 优先、clear_pause 优先及 pause_days 范围均保持。

Privacy GET 仍允许登录用户访问；它与 audit.privacy 均读取 SQL，能观察另一服务实例提交的变化。暂停到期继续由 SQL current 使用数据库时钟完成政策版本转换，删除旧的本机时钟判断。

## 验证与后续

三个 subagent 分别负责 Registry 存储、HTTP 政策迁移、真实 HTTP 与公共 API 回归，Root 负责交叉审查、冻结源码、统一双平台验证和提交。实际结果在收口后填写四份 HANDOFF 与恢复检查点，未运行的项目不记为通过。

后续候选：纯非政策 Settings PATCH 在等待更新锁期间发生管理员降权时的准入重验证。SQL 政策事务已有 active admin 核验，不能把该保障误称覆盖所有非政策修改。本轮不扩展所有 Provider 的应用原子性、超时或并发模型。

未承诺真实断电、硬件 flush、任意 ACL／扩展属性迁移、同账户其他进程并发改写或跨实例非政策文件事务。其他平台、Pi、声学、执行器、长稳和前端继续按 [延期台账](deferred.md) 执行。

## 最终验收

源码 `2f5e2a2267c8499a116e652a4779f111ddbe5aff`；冻结 V1 manifest `c2fc158414333488da098f06050c4daf681477aef50fbdda193b82b111744312`，653/653 源码 Git blob 一致。

macOS ARM64 workspace **1120/0/5**；Linux ARM64 Server 默认并行完整回归 **838/0/5**；两端 Server 无默认 feature 全目标检查、架构边界正反自测、变更范围 rustfmt 和差异检查通过。数字为 passed/failed/ignored，嵌套子进程摘要不重复累加。全部最终阶段使用同一份源码，没有源码漂移。本轮新增 12 项（7 storage、2 Registry、3 HTTP/runtime）在两端通过；本轮测试阶段均首轮通过，测试期限保持原值。

全部 Cargo 阶段结束。本轮独立 PostgreSQL 与 Linux holder 已停止，关闭前其他 DB 连接为零，测试 schema 为零，遗留 schema 清理数 0；用户 5433 未访问。日志、源快照及缓存保留在 ignored `run/validation-20261011-settings-recovery/`，可核对副本见证据索引。空闲 holder 的停止码不作为 Cargo 结果。

未运行真实断电、独立进程 kill/restart、真实 PTY／Easy-Turn、Linux 全 workspace、release／部署 smoke、外部付费服务、Pi／声学／执行器或长稳。存储回归使用软件故障注入，runtime 重启使用实际 ServerBuilder；不等于操作系统进程重启或物理断电。ACL／扩展属性迁移、异 owner 拒绝的提权实测、FIFO 故障和其他本机进程并发改写没有验证。
