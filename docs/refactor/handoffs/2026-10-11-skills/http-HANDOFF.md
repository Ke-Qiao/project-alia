# Skills HTTP CRUD HANDOFF

基线 `9c9d1b8`，分支 `codex/architecture-refactor`。本分工修改范围为 `packages/alia-server/src/api/skills.rs`；源码与文档已交 Root，统一运行验收已完成，见文末 Root 补记。subagent 未运行 Cargo、数据库、容器、服务或硬件，未提交。

## 审计结论与实现

原有三个 HTTP 写入口已经把 skill、首选工具绑定以及 artifact/dependency 变更放在同一个数据库事务中。缺口是提交后留下 reload TODO，运行中的 match/list/resolve 索引继续使用旧数据。直接在提交后补 `reload().await?` 还会出现“数据库已写入，但加载或响应查询失败”的结果；原 loader 也会先清空共享索引再逐项查询，失败可能暴露不完整索引。因此本轮复用 Registry 实现的变更所有者，没有在 HTTP 中增加独立缓存或后台刷新任务。

创建、更新、删除统一调用 `state.modules.skills.begin_change()`，使用 `change.transaction()` 执行原有 SQL、artifact 和 dependency 操作，最后 `change.commit()`。索引准备、提交、发布以及提交阶段的生命周期均由 Registry 所有者管理，详见本轮 Registry HANDOFF。

更新操作原先在事务外读取旧记录；现在在同一个变更所有者及其事务内读取，避免同一个 Registry 的并发更新先读到相同旧值。创建和更新使用泛型 `ConnectionTrait` 的原 `skill_response`，在提交前从同一事务构造响应；提交后直接返回已构造的 JSON，没有额外数据库查询、第二次 reload 或未等待的任务。

`skill_change_error` 将 Registry owner 返回的 `anyhow::Error` 中的 `DbErr` 恢复为既有 `AppError::Database`，其余保持 `AppError::Internal`。新增一项小回归，验证带 context 的数据库错误仍属于 `INTERNAL_DB_ERROR`，普通内部错误仍属于 `INTERNAL_ERROR`。artifact 与 dependency 操作原有错误映射保留。

## 保留的契约与政策

- routes、Create/Update 请求、响应字段、source、时间戳及未找到对象的 404 保持。
- 所有入口继续使用既有 `AuthUser` 和当前活跃账号验证；没有增减 admin 限制，没有调用工具执行或批准入口。
- list/get 仍查询数据库并包含 disabled skills；match/list runtime/resolve 使用 Registry 当前快照，只有 enabled skills 出现在运行索引。
- 首选工具仍按工具名称去重；未知工具返回原 Validation 错误并回滚；disabled tool 的绑定与依赖仍保存，但从响应与运行首选名称列表中过滤。
- 删除前仍检查 external dependents；skill、binding 和 graph 的事务关系保持。
- `intent_patterns` 继续保留原“必须为数组”的接受语义；数组中非字符串由原 loader 忽略，没有借本轮重构改变输入规则。`prompt_template` / `description` 既有更新及 null 行为未扩张。
- Skill 首选工具继续只是排序提示；全局工具可见性和 Execution 批准链没有改动。

`interaction/prompt.rs` 读取运行 skill list，`generation.rs` 使用运行 resolve；两者随索引发布看到新数据。Router 分类提示中的内置技能表仍为既有设计，本轮未改变分类政策，不能将 HTTP reload 完成表述为新技能自动被分类器选择。

独立待办的具体入口：`packages/alia-server/src/modules/interaction/routing/service.rs` 的 `CLASSIFICATION_PROMPT_BOTH_TEMPLATE`（77 行）与 `CLASSIFICATION_PROMPT_MINOR_ONLY_TEMPLATE`（131 行）列出静态技能；`classification_prompt_for_mode`（234 行）和 `classify`（321 行）使用这些模板；`classification_user_prompt`（248 行）只传 user text / recent context。`routing/mod.rs::RouterInput`（7 行）没有 live skill 列表，`generation.rs::route_response`（558 行构造 RouterInput）也不传 Registry 快照。`RouterService::to_decision`（408 行）仅检查技能名非空，最终 `generation.rs`（622 行）调用 Registry resolve。后续应独立审计动态候选发现、禁用／删除后的路由退化与允许技能名校验，继续保持 tools 全局可见性和 Execution 批准；本轮未改动以上入口。行号指本轮基线源码。

## Checklist

- [x] 读取三个 CRUD、list/get/match、AuthUser、ArtifactRegistry、Registry loader 与实际调用方。
- [x] 三个写入口统一变更所有者，移除全部 reload TODO。
- [x] 更新读取与响应查询移入同事务，提交后不再读取数据库。
- [x] 保留请求/响应/auth/可见性/批准/依赖检查及错误分类，新增错误 mapper 回归。
- [x] 本文件 scoped rustfmt 与 `git diff --check` 通过；未引入依赖、feature 或旧路径兼容层。
- [x] 提交分工和契约给 Registry / Regression agents 与 Root，准备本 HANDOFF。
- [x] Root 在冻结源码上完成编译、真实 HTTP/SQL 回归、取消/失败/关闭验证和证据归档。

后续恢复先读取 Root checklist、Registry 和 Regression HANDOFF。此处列出的运行行为仍须以 Root 最终同源测试结果为准，不将静态审查记为已运行验证。

## Registry 交叉审查

初版提交错误恢复使用新连接直接重建索引。审查本地 SeaORM 1.1.20 / SQLx 0.8 源码确认，提交返回传输错误后原事务的 rollback 只会排队，不能仅凭新连接查询证明原数据库 COMMIT 已结束。作者据此在统一 `begin_snapshot` 中加入 `LOCK TABLE skills IN SHARE ROW EXCLUSIVE MODE`，位于 Repeatable Read 事务的任何查询之前；变更、reload、提交结果不确定后的重建均经过此屏障。Root 负责实证等待屏障后才取得 RR 快照，以及相关故障回归。

最新实现的锁顺序为 changes gate → 数据库事务／表屏障 → 准备完整 snapshot → index 写锁。owned commit worker 在 COMMIT 前把索引设为不可用，确认提交成功后无 await 发布准备好的完整快照；提交错误则在仍持有 changes 与 index 的情况下经过新屏障重建，重建失败维持不可用。worker panic 不会重新暴露可能过期的原快照。HTTP 丢弃等待不会丢弃 worker；stop 先等待 changes gate 并关闭 admission，再 close/wait 既有 RuntimeTasks，新的变更及 reload 检查关闭标志。

HTTP 响应查询或 snapshot 准备失败仍发生在提交 worker 启动前，原数据库事务回滚且原索引保持；提交结果错误经原 DbErr 分类返回。只读审查未见新的阻断项，以上为代码路径结论，实际取消／故障／并发／stop 结果以 Root 同源验收为准。

## V2 冻结后最终边界复核

已核对 Root 的 V2 manifest 文件摘要为 `0ecb239bfe6ec3d4a9a12032e518376695e2b5b1b78f8fb3a79f3ce5a9639552`。重新读取最终 HTTP / Registry / 回归测试，并扫描 `packages` 与 `crates`：旧 Registry create/update/delete/get/list 与 plugin skill mutation 没有遗漏调用方；生产 HTTP 仍恰好三个变更 owner 入口，未出现提交后 reload 或异步裸写旁路。启动 seeds / markdown 同步仍只处于独占 init，运行中的变更与 reload 走统一 gate。

对基线 `9c9d1b8` 的实际 Git 比较确认 `auth`、HTTP tools/effects、整个 Execution 模块、Interaction tools/generation/prompt/routing 源码均未改变。进一步读取调用链确认：Skill preferences 进入全局可见工具的排序；`ExecutionToolRegistry::execute` 继续进入 ExecutionService，submit 重新解析真实 executor 并使用当前政策产生 effect，显式批准只从已认证 confirmation 入口进入；worker 在 dispatch 前再次验证工具与政策摘要。更新首选工具既不会增加真实 executor，也不会跳过 require_confirmation、账号归属或 claim 验证。

已再次核对事务内 response 的错误回滚、DbErr 分类、提交所有权在 HTTP 取消后的保留、stop 对准备阶段与已启动 worker 的等待，以及 panic 后不可用索引语义，未发现新的阻断项。Root 已报告 V1 真实 HTTP 三项通过；V1 双平台相同的库测试失败属于连接池 close 首次 poll 状态假设，V2 改为断言 pool 已关闭并兼容 Ready/Pending。此处仅记录 Root 的验收进度，V2 完整实测结果留给 Root 最终补记，不将 V1 结果充作 V2 同源验收。

未运行 Cargo、PostgreSQL、Linux 或硬件；本轮没有实现动态分类器技能发现，也没有改变既有数组元素或 nullable 字段语义。

## Root 最终验收补记

上文“待 Root 验证”的状态为分工提交时刻；现已完成。源码 `7aca2a6`，V2 manifest `0ecb239bfe6ec3d4a9a12032e518376695e2b5b1b78f8fb3a79f3ce5a9639552`。macOS workspace **1068/0/5**；Linux ARM64 Server 默认并行完整回归 **786/0/5**。两端 Skills lib 定向各 **8/0/0**，无默认全目标检查、边界正反自测与变更文件格式检查通过。计数依次为 passed/failed/ignored，定向测试及嵌套子进程摘要不额外加到完整结果。所有最终结果同源、无源码漂移。 V1 原失败记录保留；详见 [Root HANDOFF](root-HANDOFF.md) 与 [证据索引](evidence-index.json)。任务资源已全部停止；本分工所有最终验收项已关闭。
