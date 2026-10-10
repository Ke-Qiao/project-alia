# Skills HTTP 索引回归 HANDOFF

日期 2026-10-11，基线 `9c9d1b8`，分支 `codex/architecture-refactor`。本分工负责真实 HTTP／PostgreSQL 集成回归，不修改生产代码，不运行 Cargo、数据库、容器或 Git 提交。运行验收由 Root 统一执行。

## 先检查再补缺口

`tests/skills.rs` 已覆盖 seed、preferred tools 和 Markdown 解析，但没有运行中的 HTTP CRUD／索引一致性验证。`tests/interaction_tools.rs` 已证明技能偏好不会隐藏全局相关工具；`tests/execution_service.rs` 已证明模型 `confirmed` 参数不能批准、跨 owner 确认失败、executor 热替换、停止与取消所有权。保留这些测试，不复制其内部实现。

新增 `tests/server_runtime/skills.rs`，复用原 `Fixture` 独立 schema、`delivery::account` JWT、真实 `runtime.run()` HTTP 监听、`catch_unwind` 后停止并 join 的清理方式；只在 `tests/server_runtime.rs` 增加模块入口。没有生产 test hook、mock 数据库、新依赖或真实模型／外设动作。

## 三组新增回归

1. `http_crud_publishes_skill_context_without_granting_tool_authority`：创建成功立即经过 `/skills/match`、registry `resolve/list_skills` 验证 prompt、preferred tools、cacheability；更新关键词后旧关键词立即不匹配；禁用后 match/list/resolve 清理，重新启用后恢复；删除后 SQL、artifact、bindings、依赖图和索引均清理，重复删除／更新返回 404。未登录创建为 401，普通当前用户的既有写权限保持。
   - 未知 tool 的创建和更新返回 400，完整 SQL row／binding／artifact／依赖图与原索引保持；外部 dependent 仍阻止删除。
   - 重复 preferred tool 去重，禁用数据库工具不进入偏好结果，技能偏好不会安装其 executor。
   - 现成 builtin `schedule_task` 在技能创建前后定义不变；即使请求参数写 `confirmed=true`，执行入口仍返回 `Proposed` 且 `requires_confirmation=true`。其他 owner 的确认返回 404，`scheduled_tasks` 无副作用；原 owner 随后取消提案。本测试没有批准或执行任务。
2. `failed_skill_snapshot_keeps_the_complete_index_and_rolls_back_http_changes`：fixture 暂时改名 junction 表，让 direct reload 关联读取失败，原完整列表、match 和 resolution 保持；HTTP response 构造失败也不提交。随后恢复表、成功 reload。再暂时改名 tools 表，并更新技能为无 preferred bindings，使 response 可以完整构造，但 replacement snapshot 批量读取 tools 必须失败；要求 HTTP 500，所有数据库事实与旧索引一起保持。恢复后正常更新立即可见。表名恢复动作在断言失败后仍执行。
3. `concurrent_skill_reload_retains_readers_and_publishes_the_latest_snapshot`：真实 PostgreSQL 表锁阻塞 junction 读取，`pg_locks` 确认旧 reload 已到达该阶段，索引读者仍能获得原完整 prompt。并行 SQL writer 必须在 `skills` 的 `RowExclusiveLock` 等待，证明不能穿透 snapshot 的数据库屏障；排入下一个 reload，解除 junction 锁，join 旧 reload 和 writer，再让下一个 reload 读取已提交的新 prompt。最后两个并发 HTTP 更新各改一个字段，两者 SQL 和运行索引结果都保留。

并发测试用 PostgreSQL 实际未授予锁作为阶段见证，没有固定 sleep 猜测竞态。它与 Registry 内部 gate／owned commit／stop 测试一起提供顺序保证；不能把一次最终值断言单独表述为穷尽全部调度顺序。`futures::poll!` 只把下一次 reload 排入 gate，真正新 snapshot 读取在 writer 提交后继续。

## Root 候选运行证据与最终只读复核

本 subagent 已只读核对 Root 保存的原始日志和结果文件，未自行启动任何运行：

- V1 manifest `33cd145d05cb6145a0cf7dd672a313debd0644fd7b76847a70c51665a930c492`，macOS `skills-http-v1.log` 的新增 HTTP 三组为 **3 passed／0 failed／0 ignored**，测试执行 2.49 秒；`skills-http-v1-result.json` exit code 0，source drift 为空，默认测试并行度。证据位于 `run/validation-20261011-skills/`。
- V1 macOS `skills-lib-v1.log` 与 Linux `linux-arm64-v1/evidence/skills-tests.log` 均为 **7 passed／1 failed**。两端失败都在 Registry 测试对关闭连接池 future 必须首次 `Pending` 的假设，尚未进入该用例后续 fail-closed 断言。V1 失败记录必须保留，不能用 HTTP 3/3 掩盖或合并成整个版本通过。
- V2 manifest `0ecb239bfe6ec3d4a9a12032e518376695e2b5b1b78f8fb3a79f3ce5a9639552` 对比 V1 的 **643 个输入**，唯一变化为 `modules/interaction/skills/registry_tests.rs`。夹具改为接受 close 的 Ready／Pending 两种调度，并显式验证 pool 已关闭；恢复失败 pool 前等待实际 owner 结束。生产源码和本分工 HTTP 测试均未改变。
- 已核对 V2 macOS `skills-lib-v2.log` 与 Linux `linux-arm64-v2/evidence/skills-tests.log`，均为 **8 passed／0 failed／0 ignored**，执行分别 0.65／0.49 秒，结果文件均 exit code 0 且无 source drift。该定向结果不代替 Root 尚在执行的最终完整同源回归。

Shadow、Interaction 工具选择、Execution 批准模块和现行配置相对基线均无改动。`CompanionPolicy::default()` 仍为 `Shadow`，其已有默认／模拟机会回归保留；此事实属于源码核对，本轮新增三个 HTTP 测试没有独立模拟全部 Companion 场景。`select_tools_for_skill` 仍使用全局工具目录及原 plugin 政策，偏好只是排序输入；`ExecutionPolicy::requires_confirmation` 仍由工具定义或配置列表决定，不采信模型参数。

可见性实跑证据限于一个现有 builtin 定义和一个禁用、无 executor 的数据库 fixture，加上已有 ranker／Execution 回归。不要把这三组测试表述为“所有工具和所有政策组合全部验证”。删除 bindings 使用原 PostgreSQL 外键级联，HTTP 测试的数据库快照按 skill 关联读取；它不是在移除外键后检验孤儿行的测试。没有发现需要解冻生产源码的阻断项。

## 检查清单与续接

- [x] 检查 parser／ranker／Execution 已有回归与真实 HTTP fixture。
- [x] CRUD 成功立即发布、禁用／删除移除、401／404 和依赖限制。
- [x] 未知 tool 及 snapshot 准备失败的数据库／artifact／index 原子回滚。
- [x] preferred tools 不安装 executor，不改变 confirmation 或 owner 审批要求。
- [x] 真实 PostgreSQL 等锁见证、准备阶段读者可用、后续新快照和并发字段更新。
- [x] 单文件 rustfmt 与限定路径 `git diff --check` 静态检查。
- [x] Root 在冻结源码上运行新增三组与既有 Skills／Interaction／Execution 回归，记录实际结果。
- [x] Root 完成同源验证、交接摘要及提交证据。

本分工提交时进度为 **6/8**；Root 已完成最后两项同源完整回归和源码提交，最终 **8/8**，见文末补记。V1 定向结果保留为候选证据，不混入 V2 最终计数。Registry 内部 commit wait 被取消、COMMIT 被拒绝、重建失败后关闭索引及 stop 等待 owner 的精确验证由 Registry 分工实现；不能用丢弃 HTTP 客户端请求来替代服务内部 owner 取消证明，也不能把 deferred trigger 拒绝 COMMIT 的测试称为真实网络丢失 COMMIT ACK 的复现。

未验证真实 Easy-Turn、Pi、硬件执行或多进程热加载。最终运行结果、资源归还、源码冻结身份和提交以 Root HANDOFF 为准。

## Root 最终验收补记

上文“待 Root 验证”的状态为分工提交时刻；现已完成。源码 `7aca2a6`，V2 manifest `0ecb239bfe6ec3d4a9a12032e518376695e2b5b1b78f8fb3a79f3ce5a9639552`。macOS workspace **1068/0/5**；Linux ARM64 Server 默认并行完整回归 **786/0/5**。两端 Skills lib 定向各 **8/0/0**，无默认全目标检查、边界正反自测与变更文件格式检查通过。计数依次为 passed/failed/ignored，定向测试及嵌套子进程摘要不额外加到完整结果。所有最终结果同源、无源码漂移。 V1 原失败记录保留；详见 [Root HANDOFF](root-HANDOFF.md) 与 [证据索引](evidence-index.json)。任务资源已全部停止；本分工所有最终验收项已关闭。
