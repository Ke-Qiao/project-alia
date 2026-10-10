# Context / Companion 边界交接：L11

基线 `f15db66`，分支 `codex/architecture-refactor`。本分工 **8/8**，实现和静态审查完成；第 8 项由 Root 统一进行冻结源码下的 Cargo / PostgreSQL 验收。没有启动 Cargo、数据库、容器、服务，没有访问用户 5433 或凭证，没有提交。

## Checklist

- [x] C01 核对 Context authority、Companion source/grant、Delivery 控制绑定及现有真实 PostgreSQL 回归。
- [x] C02 Context 保留政策、账户、隐私和关系 authority 锁以及规范 Appraisal 证据读取。
- [x] C03 `CanonicalObservation` 和擦除根投影归 Companion。
- [x] C04 私有 grant、预算/身份完整性、Companion 政策与锁后重读归 Companion。
- [x] C05 Context 保留私有 lease 签发、证据合并、持久化和精确引用校验，字段权限不扩大。
- [x] C06 迁移 Context / Delivery 调用和原 invitation 测试；删除旧模块及转发路径。
- [x] C07 完成作用域 rustfmt、diff 检查、SQL/权限静态比较。
- [x] C08 Root 同源编译和 PostgreSQL 回归完成，证据索引更新。

## 实现和所有者

- `modules/memory/context/appraisal_source.rs` 提供 `AppraisalReadAuthority`、`lock_appraisal_authority` 和 `load_user_appraisal_source`，返回原私有 `ContextLease` 的只读对象与 Appraisal ID/revision/digest/effective_at/valid_until。这个模块不再引用 Companion 类型、表或业务政策。`private_readable` 保持原有范围供 Shadow / Live 的提前拒绝检查；ContextLease 的全部字段仍是 `pub(super)`。
- `modules/companion/source.rs` 从已校验的 Context/Appraisal 数据构造 `CanonicalObservation` 和擦除根；完整保留每种 Evidence 映射、根排序去重与禁止 invitation 再成为新 source 的规则。原函数已限制 `AppraisalFact::UserMessage`，后方不可达的 ResponseCommitted/Execution fact 分支删除；对应合法 Evidence 的 response/execution 擦除根映射保留。
- `modules/companion/invitation.rs` 拥有 `private_grant` 读取、64 KiB 界限、JSON 摘要与完整 grant 结构校验、所有持久绑定和预算/时间一致性，以及 Enabled/quiet/paused 政策与锁后 grant 精确一致性检查。摘要仍调用已有 canonical digest。
- `modules/memory/context/invitation.rs` 保留原 Context 证据适配：重建规范 source closure，多 Appraisal 同源检查及 evidence merge，匹配 observation 摘要，调用 Companion 锁后复验，再在私有字段内建立 dedicated purpose/request/Context/输出截止与唯一 admission evidence。签发仍在当前事务 `persist`，引用验证仍逐字段比较 expected lease。
- `modules/delivery/repository/control.rs` 直接向 Companion 读取已验证 grant 的原绑定事实，保持原 scope/request/initiative/through_sequence 约束。读取本身不能给出输出许可。
- 原 `context/companion.rs`、`context/companion/invitation.rs` 删除；原 5 个 PostgreSQL invitation 回归与 purpose 单测移到 `context/invitation`。Delivery 的事务写调用更新为关联函数。Shadow / Live 消费路径由 Companion agent 更新。

## 事务和锁顺序

Context/Companion authority 始终按 **Context policy FOR SHARE → active user FOR SHARE → privacy FOR UPDATE → relationship FOR UPDATE**。随后锁 canonical Appraisal / 原父 Context 的 source closure；重复 `lock_authority`、过期与完整 ancestry 验证保留。

Invitation 初次读取 grant **不锁 grant**，仅取得不可变选择事实；再锁 authority 和全部原始根，合并并验证规范 observation；随后锁 Companion policy，再 FOR SHARE 读取 grant 并逐项比较初次读取结果。保留这个顺序，避免 source 擦除触发器先锁根、再退休 grant 时反向等待。之后在同一事务检查输出截止，生成/验证 lease；只有新签发再检查短 `admit_before`，不把短准入期限变为旧输出恢复期限。

这里保留必要的 Context evidence adapter → Companion source/grant helper 依赖，因为 Context 负责证明该 lease 的完整依赖，而 Companion 负责自身政策和 grant 内容。没有新增 trait、registry、事务管理器或可修改/伪造 lease API，也没有把独立 `validate()` 的结果拿去授权稍后的写入。事务开始/commit 仍属于原 Live admission / Response / Delivery 等具体用例。

## 静态结果与待验收

`run/validation-20261010-l11/context-static-audit.json` 记录基线与新四模块 SQL 字符串多重集一致：**8/8**，无新增或删除查询；Context 私有字段权限保持，neutral Appraisal source 无 Companion 类型/SQL，Context invitation 不再解析私有 grant 或 Companion 政策。这个静态结果只证明所述结构，不替代编译、事务并发或 PostgreSQL 运行验证。

作用域 rustfmt 和 `git diff --check` 通过；旧 `context::companion`、`CompanionAuthority`、`lock_companion_authority` 源码引用清零。业务 error 类别保持，Appraisal source 的三处错误文案去掉 Companion 名称；没有政策/配置/依赖/feature/migration 变化。

Root 应至少涵盖迁移后 5 个 invitation PostgreSQL 测试及 purpose 单测、Context `validated_transaction_serializes_clear_until_effect_commit` / `rollback_or_missing_canonical_storage_never_returns_empty_authority`、Appraisal clear/late SQL rollback、Shadow replay 和 Live admission 故障矩阵。当前默认 Shadow、全部政策和 source 双 epoch 保持。

## Root 最终验收

源码 `60f665cf6fce4811b144dfc743b4814c5bdf19d5`，V4 manifest `520ec684c2619e775389330adbe837113edcb675e2f7d62fc7f5d65758fe274f`，640/640 Git blob 一致。macOS workspace **1060/0/5**、真实 PTY **3/0**、无默认 availability **2/0**、提前准入 **1/0**；Linux ARM64 Server 完整默认并行复跑 **778/0/5**，两端无默认全目标及 Linux release/native link 通过。所有最终结果同源且无漂移。

本轮 10/10，L09—L12 3/4，R01—R06 仍 2/6。四个任务容器均停止。停机前 schema/其他连接均为零，无需额外删除 schema。原失败、修正和最终证据见本目录 evidence-index.json。

未运行 Linux 全 workspace 或部署 smoke，未验证真实模型、Pi/声学/机械与其他平台。Linux V4 首次旧 panic-fence 子进程 20 秒超时仍保留为未定位项；定向和完整复跑通过不等于根因修复。
