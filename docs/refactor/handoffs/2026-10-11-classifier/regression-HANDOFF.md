# Classifier／live Skills 回归 HANDOFF

日期 2026-10-11，基线 `4e4d595`。本 subagent 先只读核对既有 Router、真实 HTTP Skills fixture、Interaction 内部 fixture 和 Execution 回归，再按 Root 统一契约实施。没有运行 Cargo、数据库、Docker 或 Git，没有修改生产文件。

## 实际链路与复用

新增 `tests/server_runtime/classifier.rs`，复用原 server runtime `Fixture`、`delivery::account`、Skills HTTP `client/response/definition` 三个 helper；三个 helper 只提升到 `pub(super)`。父测试模块入口由 Root 添加。

测试直接通过真实 HTTP 创建／修改技能，经 `ServerEvent::ChannelInput` 进入已经由 ServerBuilder 启动的 Interaction，再走实际 `process_text`、Router classifier、skill resolution、最终模型、Response／Conversation 持久化。等待 `ConversationTurnCompleted` 后核对真实数据库 history，不手工构造 `RouterInput` 代替 Generation 接线。

只新增一个 input-only 观察 Channel，其能力不声明物理文本／音频输出，默认 Delivery 方法仍拒绝输出。没有复制现有完整文本投递 adapter，也不宣称验证用户看到或听到内容。受控 `LlmAdapter` 记录 classifier 的结构化 JSON payload 和最终模型 system messages；minor／major 独立标记，分类只允许 minor。等待阶段使用 Semaphore，所有等待和 runtime stop／join 有界，断言失败仍执行停止。

## 两条新增测试

`http_skill_catalog_drives_real_classification_and_final_model_context` 连续运行五个真实 turn：

- 创建非 builtin 技能后，分类的 `available_skills` 立即包含确切 name、display_name、description、category、keywords 五字段，没有执行 prompt 或 preferred tools。只有选中后的最终模型收到秘密标记 prompt。
- HTTP 更新说明和关键词后，下一个 classifier 输入立即更新。
- 禁用、删除后，候选消失；即使模型仍返回原技能名，也必须正常 fallback，最终模型不接收原技能 prompt。
- 返回从未提供的未知名字同样正常完成，不把未知名送去失败的 resolve 后中断回答。
- 每个 turn 恰好一次分类和一次最终回答，且完成真实历史持久化。

`classification_in_flight_revalidates_the_selected_skill_and_current_fallback_mode` 对六种情况分别运行真实 turn。classifier 先记录已提供的候选并停在 Semaphore，HTTP 变更成功后才释放旧分类结果：

1. 仅改变执行 prompt，保持公开候选 metadata 不变，旧选择也必须失效；新旧 prompt 都不注入 fallback。
2. 禁用已选择技能。
3. 删除已选择技能。
4. 删除再创建完全相同内容的同名技能，显式确认持久化 id 不同；新对象不能继承旧选择。
5. 修改无关技能目录内容；原技能定义和 id 没变，原选择继续有效。这验证 per-skill revalidation 不误用全目录变化一概拒绝。
6. 等待期间更新技能并把 Router 当前模式改为 MinorOnly；失效选择必须使用当前 minor fallback，而非分类开始时的 Both 模式。

两条 fixture 的 minor/default role 都是原隔离配置的 `noop` 名称，因此正常 Skill 分支使用默认 minor adapter；Both 模式 fallback 必须调用 major。第六种情况通过 Router 既有 `apply_settings` 接口改模式，不声称覆盖 HTTP Settings 本身。所有失效选择保持单次分类，没有为等 HTTP 完成使用固定 sleep。

## 既有回归迁移与边界

`tests/server_runtime/skills.rs` 从旧 `resolve(&SkillMatch)` 迁移至先取 `catalog()` 再 `resolve(&catalog, name)`。禁用／删除后的预期是 `Ok(None)`；真实索引 unavailable 仍是 error，不能混为一谈。原 CRUD 原子性、snapshot 失败回滚、PostgreSQL 屏障、普通用户权限、精确错误状态、disabled preferred tools 和 `schedule_task` 批准回归保留。

工具可见性和执行批准继续由原 `interaction_tools`、`execution_service`、Skills HTTP 提案回归共同验证。本分工不新增工具执行器、不批准实际副作用，也没有修改 Shadow／执行政策。Cache 目录指纹、旧缓存失效、变更期间生成内容不写缓存的内部回归由 Root 实施，本文件不把这些工作列为已经验证。

## Checklist 与恢复

- [x] 只读审查可复用 Router／HTTP／Interaction／LLM fixtures 和调用者。
- [x] 用真实 EventBus→Interaction 链证明 HTTP CRUD 影响分类和最终模型。
- [x] 未知／禁用／删除技能名正常 fallback，不注入失效 prompt。
- [x] 分类中六种更改场景、durable id、private content、当前模式和单次分类约束。
- [x] 旧 Skills HTTP resolve 契约迁移，保留政策与原子性回归。
- [x] 单文件 rustfmt 完成，源码冻结供 Root 统一编译。
- [x] Root 在冻结输入上编译并运行两条新回归和相关既有回归，记录实际结果。
- [x] Root 完成同源完整验收、资源清理、checklist／恢复点和提交。

本分工最终 **8/8**。Subagent 只做静态阅读和格式化；Root 的实际运行结果见下方验收补充。

未验证真实模型准确率、物理投递、Pi／音频硬件、跨进程目录同步或穷尽并发调度。最终证据与提交以 Root HANDOFF 为准。

## Root 最终验收补充

源码 `744cae2a11f8192f0e8ccf3d13004402202c1411`，V2 manifest `adeeaa0d150e6c7a036beeb05aa409da1d811e130d60354fe46e290c2ec132b5`，644/644 Git blob 一致。

macOS workspace **1080/0/5**；Linux ARM64 Server 默认并行完整回归 **798/0/5**。两端无默认 feature 全目标检查、边界正反自测和变更文件格式检查全部通过。计数依次为 passed/failed/ignored，嵌套子进程摘要不重复累计；所有最终阶段同源、无源码漂移。

以上实施时的“待验证”描述保留时序语境，以本段为最终状态。新增回归随完整默认测试实际运行；没有单独把循环场景或嵌套子进程当作额外测试累计。四份 HANDOFF、checklist 和恢复点完成，资源关闭见 Root HANDOFF。
