# Skills registry HANDOFF — 2026-10-11

负责：`/root/skills_registry`。基线：`9c9d1b8`，分支 `codex/architecture-refactor`。

## 范围与实际缺口

本 agent 仅编辑 `packages/alia-server/src/modules/interaction/skills/**` 和本文件。HTTP owner 接入事务，regression owner 负责真实 HTTP 回归；Root 统一编译、数据库、容器、证据冻结和提交。

原 `load_skills` 在查询 skill 行后才取索引写锁并清空 map，再逐行查询 preferred tools。错误会遗留空或部分索引；两个 reload 可用不同时间的查询结果反序覆盖。HTTP 三处 TODO 没有调用 reload。原 Registry 另有全仓无调用者的 CRUD、查询和 plugin 注册入口，且写入缺少 HTTP 已具备的完整事务。

## 已实现

- [x] `SkillRegistry::begin_change` 取得统一变更 gate，创建 Repeatable Read 事务，返回 crate 内 `SkillChange`。HTTP 使用 `transaction()` 写入并在 `commit()` 前构造响应。
- [x] 每个变更、reload、提交错误后的重建事务都先取得 `skills` 表的 `SHARE ROW EXCLUSIVE` 锁，再进行任何快照查询。该锁等待未明的旧 COMMIT 实际结束，避免另一连接先读取旧状态又将其当作权威结果发布。
- [x] 三批查询构造完整候选索引：enabled skills、preferred bindings、enabled tools。保留关键词匹配/priority/cacheable/prompt 和工具偏好语义，去除逐 skill/tool 的 N+1 查询。
- [x] 正常准备或 reload 查询失败保留原完整索引；事务未提交时继续依赖原 DatabaseTransaction rollback 机制。
- [x] 提交前取得索引 owned 写锁；复用 `RuntimeTasks` + oneshot 拥有不可中断的 COMMIT/发布阶段。外层 HTTP 等待取消后，worker 仍持变更 gate、DB 事务和索引锁，完成一次提交及发布。
- [x] COMMIT 报错时保留原 DbErr，经 DB 表屏障重新读取权威快照。重建成功发布实际值，仍向调用者返回原提交错误；重建也失败则索引为 None，match/resolve/list 均报 unavailable，健康状态 Degraded。成功 reload/change 可以恢复。
- [x] `stop()` 等现有 prepare/commit 的变更 gate，关闭后续变更/reload admission，并等待 RuntimeTasks。预期 DB 操作错误经 oneshot 返回，不污染 shutdown errors；panic 仍由 RuntimeTasks 记录。
- [x] 删除无调用者的 get_by_name/list_all_db/create/update/delete/register_plugin_skill/unregister_plugin_skills 和 PluginSkillDefinition。实际 builtin/file 初始化路径、matching、resolve、列表保留。
- [x] 添加 4 个真实 PG 单元回归，使用 deferred trigger/advisory 锁与 pg_stat_activity 确认 COMMIT 阶段，覆盖取消、stop、错误重建、双失败 unavailable、reload 恢复。
- [x] Root 最终编译、真实 PostgreSQL 回归、完整验证和源码提交核对。

## 测试与所有权边界

`registry_tests.rs` 只在 cfg(test) 下复用现有 Companion repository 的 `Fixture/with_fixture`，仅使用其隔离数据库与清理设施；没有新增生产跨域依赖或通用测试框架。

取消测试直接 abort `SkillChange::commit` 的等待任务，并确认后台 COMMIT 仍在 PG deferred trigger 等待，再验证 stop 未提前返回。没有把 HTTP 客户端断开误当作 Axum handler 已取消。

双失败测试为 Registry 建立同一 fixture schema 的独立连接池。在 COMMIT 已等待时关闭该池，触发提交拒绝后重建无法获取连接；验证三种索引读取与健康状态，再在测试内部恢复 DB 依赖并调用正常 reload。未添加生产故障注入钩子。

V1 macOS 与 Linux 定向验证均真实出现 7 通过、1 失败：双失败测试错误地要求 `pool.close()` 首次 poll 必须 Pending。V1 HTTP 回归为 3 通过。该测试需要证明的是池已关闭、后续 acquire 失败；首次关闭调用立即完成也满足故障注入条件。已对照本机锁定 SeaORM 1.1.20 与 SQLx 0.8.6 源码，Root 已应用并格式化仅修测试的 `run/validation-20261011-skills/registry-pool-close-v2.patch`：接受 Ready/Pending、显式断言 `pool.is_closed()`、不重复 poll 已完成 future；修复测试依赖前有界等待 commit worker 的 active 计数归零。保留 deferred COMMIT 实际等待、外层 DbErr、三类索引 unavailable、Degraded 和成功 reload 恢复断言。V2 源码 manifest SHA-256 为 `0ecb239bfe6ec3d4a9a12032e518376695e2b5b1b78f8fb3a79f3ce5a9639552`，相较 V1 仅 `registry_tests.rs` 变化。V1 失败日志保留；V2 已应用并经 Root 最终同源验证通过，两端定向均 8/8；完整结果见文末补记。

最终只读复核确认锁与资源顺序：变更 gate → DB 表锁 → 完整快照 → 索引写锁 → RuntimeTasks 拥有 COMMIT/发布；错误重建持有原 gate 与索引锁，重新取得 DB 表屏障后才读取。stop 先等同一 gate 并标记关闭，再 close/wait tracker，因此同时覆盖准备事务及已交接 worker；普通 DB 错误通过 oneshot 返回，panic 仍被 tracker 记录。Root 的 `lock-snapshot-probe.json` 在隔离 PostgreSQL 中实测 RR 事务先实际等待表锁，随后首个 SELECT 读到先前 writer 已提交的新值；没有将等待前的旧快照用于恢复。

## 保留事项

工具偏好仍只是排序提示：没有安装 executor、改变工具可见性、绕过批准或改变 Shadow/执行政策。文件/builtin 初始化仍在服务 admission 前执行，现有导入失败政策保持。

本轮没有建立多服务实例间的索引通知/复制，外部直接 SQL 写入仍需要显式 reload；表锁解决当前实例的提交边界与快照一致性，不声称实现分布式缓存同步。COMMIT 的连接错误可能意味着持久化结果不确定；恢复流程按数据库实际值发布并返回原错误，不声称回滚已发生。

本 agent 未执行 Cargo、DB、Docker 或 Git 提交；实际结果与剩余问题由 Root 验证后填入恢复检查点。

## Root 最终验收补记

上文“待 Root 验证”的状态为分工提交时刻；现已完成。源码 `7aca2a6`，V2 manifest `0ecb239bfe6ec3d4a9a12032e518376695e2b5b1b78f8fb3a79f3ce5a9639552`。macOS workspace **1068/0/5**；Linux ARM64 Server 默认并行完整回归 **786/0/5**。两端 Skills lib 定向各 **8/0/0**，无默认全目标检查、边界正反自测与变更文件格式检查通过。计数依次为 passed/failed/ignored，定向测试及嵌套子进程摘要不额外加到完整结果。所有最终结果同源、无源码漂移。 V1 原失败记录保留；详见 [Root HANDOFF](root-HANDOFF.md) 与 [证据索引](evidence-index.json)。任务资源已全部停止；本分工所有最终验收项已关闭。
