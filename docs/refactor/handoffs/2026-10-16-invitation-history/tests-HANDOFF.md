# CompanionInvitation 历史只读引用 — Tests

2026-10-16（本地 2026-10-17 凌晨回填最终结果）；基线 `95cd077`，源码提交 `4e2968f18d765f326e76d24a3bcdf5bba9034efc`。负责新 `packages/alia-server/src/modules/memory/context/invitation/history_tests.rs`；Root 负责接线、执行及提交。本分工 **6/6** 完成，不是项目完成比例。七项新增回归已在统一定向 GREEN 和最终一次 workspace 中通过，证据、资源与提交身份已核对。

## Checklist

- [x] 只读核对 Context/Companion 真实 PostgreSQL fixture 与 Generation pipeline 阶段屏障。
- [x] 最小 RED 先于生产变更执行并保留；独立核对预期遗漏断言与同库清理结果。
- [x] 完成七项回归候选，复用原 fixture、45 秒正文期限与默认并发。
- [x] 限定 rustfmt、diff check 和静态 API 核对；修正原 instance 退休、删除来源的 FK 前置及防再源拒绝见证，交 Root/review。
- [x] 核对 Root 一次定向 `context::invitation::` GREEN 原始证据与资源状态。
- [x] 核对最终冻结源码的 workspace、资源清理与提交身份并收口。

## 已确认 RED

复用 `companion::live::tests::{Fixture, with_fixture}`，实际定义位于 `companion/repository/tests.rs`。通过原 `fresh()` 获得真实 admission、Context 和 pending Response，在真实事务中插入 assistant message 并调用 `DeliveryRepository::mark_generated_in_transaction` 后提交。随后插入新的真实用户 source，使用新的 request materialize 普通 Generation。

`run/validation-20261016-invitation-history/invitation-red.log/json`：0 passed / 1 failed / 0 ignored / 377 filtered，目标 0.47 秒、命令 66.77 秒、exit 101，`source_drift=[]`。唯一失败为 `committed same-session invitation was omitted from ordinary Generation history`；此前身份与七张副作用表计数断言通过。`after-red-resources.json` 为 0 schema / 0 other clients。旧源码片段与 manifest 单独保留，不用修复后文件覆盖 RED。

## 七项候选及实际断言

1. **已提交 live 邀请进入新 Generation。** 新用户 source/request/context 身份，原邀请文本和新 `CompanionHistory` 引用可见，没有 `CompanionAdmission` 进入普通 Generation；admission、source claim、Response、Delivery、Execution、Memory、Appraisal 表计数不因历史读取增加。
2. **提交与会话边界。** 故意持久化有 assistant 指针但仍 pending 的 Response，不能贡献历史；真实提交后可读。其它 owner、同 owner 其它 session 均不读该历史，错误 channel 的新 source 被拒绝。
3. **输出期限、退休与 shadow。** 使用现有合法 4 秒 candidate TTL，以 PostgreSQL `clock_timestamp()` 观察原 deadline，轮询上限沿用既有邀请测试的 6 秒；不修改原期限、不推进 Tokio/PG 时钟、不靠固定 sleep 推测过期。过期后原邀请 ResponseCommit/OutputAdmission 拒绝，新 Generation 仍可读；其 `valid_until` 必须等于原邀请 Appraisal 依赖的最早真实期限且晚于输出 deadline，不能丢成 `None`。使用 `permit.server_instance_id()` 的原 owner 退休，再检查 private grant/digest 为 NULL；新建 ContextService、切回 shadow 后仍只读且不产生新输出权限。这里证明服务对象重建，不冒称整机进程重启。
4. **已发行租约的再次校验。** 独立 fixture 覆盖八种原因：先删对应 Appraisal 再删除原 user source、单独修改 source 正文、修改 assistant 正文、修改 final_content、修改 Response channel、替换当前 Context 政策、真实 Memory privacy clear、真实 relationship clear。旧 Generation 在 ResponseCommit/OutputAdmission 均拒绝，新 materialize 省略邀请。删除 source 的一支明确是 Appraisal/source 复合撤销，先满足非级联 FK；正文修改支独立验证 source 校验，不以 FK 异常充当撤回。
5. **派生闭包及权限不可扩大。** 普通 Generation 真正提交后，再由后继 Generation 读取；移除继承的 marker、篡改四个 marker 字段，以及混入原 live admission 均拒绝。缺失 marker 断言依赖二级 DerivedMessage 的真实 parent 闭包，不声称可识别任意直接重写数据库并重算摘要的伪造。
6. **Memory/cache 派生。** 实际 `MemoryService.bind_context/store_guarded` 保存根，检查 persisted direct roots 中 marker 与 synthetic Memory validator 成功；按照既有 cache fixture 的 SQL 建立普通 Generation cache，执行真实 `merge_cache` 并验证。删除邀请 assistant 后，既有 Memory 根消费、合并后的 Generation 和再次 cache merge 均拒绝。此项不声称管理 `get/list` 自动过滤或物理擦除全部后代。
7. **阻止再次成为 Companion 来源。** 原 fixture 创建真正的新用户 Appraisal，确认其 Context 包含历史 marker；`load_companion_source` 必须由专门的 `an invitation cannot become a new Companion source` guard 拒绝，不能将更早的无关 Appraisal 错误当通过。admission 数仍为 1、Delivery 数仍为 0。

旧 invitation purpose、Delivery 内容/身份、短 admission deadline 与来源/政策撤回测试保持原样，已由 Root 同一次定向执行。本轮没有新 purpose、测试专用生产 hook、数据库框架或依赖。

## 已核对定向 GREEN

只读核对 `run/validation-20261016-invitation-history/invitation-green.log/json`：一次 `cargo test -p alia-server --lib --offline --locked -- modules::memory::context::invitation::`，**13 passed / 0 failed / 0 ignored / 371 filtered**，目标 5.63 秒、命令 49.54 秒、exit 0，`source_drift=[]`。七项新增回归均在上述 13 项之内，不与 13 相加。默认测试并发未改，构建 jobs 为 2。

`after-green-resources.json` 在同一任务 PostgreSQL 记录 **0 schema / 0 other clients**。旧 RED 和本次 GREEN 的命令、日志及源码身份独立保留。

## 最终 workspace、资源和源码身份

已只读核对归档 [final-workspace.log](evidence/final-workspace.log)、[final-workspace.json](evidence/final-workspace.json) 和 [validation-summary.json](evidence/validation-summary.json)：一次 `cargo test --workspace --offline --locked`，**1277 passed / 0 failed / 5 既有 ignored / 0 filtered**，555.32 秒，exit 0，`source_drift=[]`；原默认测试并发不变。Server library 为 381/0/3，`server_runtime` 为 85/0/0，`duplex_playback` 为 31/0/0。本文件七项新增回归在最终原始日志均为 `ok`，已包含于 Server library 和全量 1277，不与它们、定向 GREEN 或 RED 相加。五项既有 ignored 是三项真实 PTY 和两项真实 Easy-Turn 服务测试，本轮没有新增或启用忽略项。

[final-resources.json](evidence/final-resources.json) 在同库最终命令退出后记录 **0 schema / 0 other clients**；[cleanup.json](evidence/cleanup.json) 记录核对 exact container ID 与任务 label 后停止并删除本任务容器及卷，任务已不存在，其它容器未修改，共享运行环境保留。

[commit-verification.json](evidence/commit-verification.json) 对源码提交 `4e2968f18d765f326e76d24a3bcdf5bba9034efc` 核对 **677 项输入逐 blob 一致**，无 mismatch/extra/missing；最终 manifest SHA-256 为 `4713f1a8f475c8524ca8c8dec270fbec441a984eea913bc8d49f98482b785ac2`。本 agent 另对 [evidence-index.json](evidence-index.json) 的 **34 份归档原件**逐一重新计算字节数和 SHA-256，全部吻合。源码提交之后仅本交接记录随 Root 文档收口，不修改已测源码。

## 范围和未冒称的验证

- `interaction/integration_tests/context.rs` 已有 `DelayedLlm` entered/release，以及模型等待后清除／政策变更阻止 Response/历史提交的真实 pipeline 回归。本轮复用相同 Context materialization/validation 入口，没有为邀请再建一套 pipeline fixture，也未新增模型实际看到邀请的专项进程测试；最终 workspace 保留原 pipeline 回归。
- 新历史 validator 的来源时间比较采用既有 PostgreSQL 微秒约定。静态追踪确认该路径的 parent 由逐列 SQL `load()` 读取，Response 时间也来自 SQL，两侧已微秒化；未复现所谓纳秒生产故障，也没有为不可达输入增加大型夹具。
- 根 Appraisal 的最早期限有本轮显式断言；各类 Memory/Graph/Cache 等来源期限沿用现有校验。不能把本轮七项概称为所有根类型的穷尽故障注入。
- 保留默认 shadow、当前政策、执行批准和资源保证，R03 暂停；无硬件、真实录放音、外部模型或平台扩张。
- 本 agent 没有运行 Cargo、数据库命令或 Git 提交。所有动态结果由 Root 统一执行后再只读核对；RED、统一定向 GREEN、最终 workspace、清理与提交身份均已核对。没有新增 Linux/Pi/硬件/模型实跑，历史 V01 超时根因继续独立开放。
