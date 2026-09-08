# ContextLease 与派生上下文撤销设计（待实施）

状态：2026-09-08 基于当前源码的只读设计，尚未实施或运行本设计的故障测试。本文不是已通过验收的能力清单。ContextPack、缓存、历史重用、响应撤销和 Companion 都属于重构主线；没有将它们加入用户指定的延期范围。

## 已有边界与实际缺口

规范 Memory 已有 owner、revision、来源依赖、privacy_epoch、原子批次和递归遗忘。Memory 向量晚返回也会在事务内重新核对规范 revision/epoch。当前 Delivery 会在准备、提交和投递许可边界验证原始 source_privacy_epoch，并保留独立输出事实。下一包应扩展这些实际保证，而不是重新建立另一套来源时钟。

| 代码位置 | 静态发现及触发场景 |
| --- | --- |
| `providers/adapter.rs::MemoryContextPack/MemorySnippet/GraphSnippet`；`providers/memory/simple.rs::retrieve_context_pack` | Pack 没有 owner、privacy/policy 版本和完整实际 revision 读集。节点检索与图检索分开执行；两次读取间发生遗忘或更新时，可以得到不属于同一有效快照的内容。图的显示三元组没有携带足够的规范 evidence 身份。 |
| `channels/pipeline.rs::process_text/build_runtime_prompt_context/retrieve_runtime_memory_context_pack` | 入口核对 source epoch 后，ContextPack、关系摘要和历史跨多个 await 取得，没有传递共同 lease。检索失败返回 None，与无记忆状态混合；后续 MemoryContextPack 再检索也可能与主 LLM 实际使用读集不同。 |
| `providers/conversation/database.rs::get_history` | 接口只给 session_id，直接读取所有 role，返回时丢实际 message ID、source stamp 与 Response 来源。当前 Pipeline 先取得所属会话降低了直接跨用户风险，但共享 provider 接口自身不能证明 owner，且无法排除已经失效的 assistant/tool 派生历史。读取全会话后才在 RAM 截断也不适合形成有界读集。 |
| `providers/cache/db.rs::put/update_embedding/get_semantic_candidates` | Cache 无行 revision/ContextLease。覆盖 response/metadata 时不清旧 embedding；异步 embedding 仅按 query_hash 回填，旧任务可写进同键重建的新行。语义候选还混入 `user_id IS NULL`，且未按请求 scope/embedding profile 过滤。当前 Pipeline 写入的是私人条目，未验证存在跨用户泄漏；但全局候选路径不具备可信公共来源证明。 |
| `providers/insights/scheduled.rs::activate_cache_entry/delete_cache_entry` | 审核 LLM 使用旧快照，回来只凭 ID 更新/删除；同 ID 内容已被替换时，旧审核可以激活或删除新版本。`memory_independent` 等布尔值是规则/模型判断，不是规范来源证明。 |
| `channels/pipeline.rs::cache_write_for_response/persist_text_interaction` | 当前通过 memory_context_used/tools 等启发式避免缓存明显依赖记忆的回答，但 history 仍参与生成。旧 assistant 历史可以进入新回答，而该回答被标为 memory_independent；缓存写入与 Response commit 分属事务，也没有提交时的 lease 核验。 |
| `expression/runtime.rs::observe_memory_recall`、`expression/affect.rs`、`expression/affect/persistence.rs` | Recall 生成 appraisal，关系/情感又回到提示词。当前有界持久化 owner 解决退出时任务归属，但 AppraisalWrite 没有规范 context lease；显示理由中包含 memory ID 不等于真实 revision 依赖。清除后排队 appraisal 和已派生关系投影尚无统一失效链。 |

当前树未发现已实现的独立 Companion/InitiativeCandidate 模块；实际可接的前身是 Expression 的 appraisal、关系摘要及相关后台路径。不能将目标中的 Companion 闭环写成现成功能，也不能因为尚无模块就把它从主线排除。

## 最小公共契约

建议在 Server 内设置一份持久化、不可由模型构造或修改的 ContextLease，并给调用者只读 handle：

```text
ContextLease {
  context_id, owner_id,
  privacy_epoch, policy_version,
  source_request_id / original_source_event_id,
  valid_until,
  dependencies_digest,
  bounded evidence_refs
}

EvidenceRef = Memory(id, revision)
            | UserMessage(id, owner, source_epoch, content_revision)
            | DerivedMessage(id, response_id, originating_context_id)
            | Appraisal/Relationship(id, revision, originating_context_id)
            | ToolResult(effect_id, immutable_result_digest, originating_context_id)
```

实际返回的 ContextPack 与模型历史共用 lease；图必须包含支持三元组的规范 memory/revision 或明确的版本化图证据。不能只附 query、memory_count、模型给出的 evidence IDs 或一个“使用过记忆”布尔值。只记录实际进入 LLM、表达或派生投影的读集；截断/过滤也应在建立该读集前完成。有大小上限，超限必须明确截断内容并同步证据，或拒绝，不能截掉凭据却保留正文。

privacy_epoch 继续由现有持久行提供。policy_version 是持久化单调版本，覆盖实际生效的读取/写入/暂停/敏感信息政策；HTTP 配置、运行时 reload、启动配置同步都必须走同一版本更新入口。暂停不删除数据，恢复推进新版本，不能使旧任务重新有效。若政策当前是部署级，使用单个部署政策行即可，不建立虚构的复杂租户策略系统。

持久结构可以是一张有界 dependency JSON 的 context_leases 表，header 字段独立约束 owner/epoch/version/expiry；不先建通用事件图或无限递归解释平台。Response、cache、appraisal 等通过 context_id 引用不可变来源。来源发生变化时 fail closed；不得重新获取 snapshot 给旧模型结果改签。

组合旧派生历史时合并、去重其规范根证据与有效期，避免每一轮再递归走遍所有历史 ContextLease。保留直接消息/结果关联用于解释，但权限验证使用有上限的完整根读集。若根读集超过限制，应减少进入模型的旧派生内容，不能只丢祖先证据；摘要也不能凭“已压缩”切断来源。valid_until 取实际依赖失效时间的最早值，提交时仍查询规范 revision/删除状态。

## 一致性与锁顺序

查询 embedding、模型调用、网络投递都在数据库事务外。完成查询向量后开启短事务：读取当前政策和 active 账户，锁现有 privacy 行，然后在同一事务中读取并物化 memory、图证据、模型可用历史及实际关系投影，写入 lease。返回前核对入口的原始 source epoch。

建议新的共享顺序为：政策行 → users → memory_privacy_state → 规范 evidence 行（按稳定 ID 顺序）→ Response → Delivery/cache/appraisal。用户和政策只读行使用与更新互斥的共享锁；privacy 读锁要与现有 Memory 写入/clear 的 FOR UPDATE 互斥。这样短事务内不会混入一半更新前、一半删除后的内容。现有 Delivery 的 Response FOR NO KEY UPDATE 要保留，避免审计 FK 的 KEY SHARE 死锁回归。所有参与写入的入口需要统一核对锁顺序，不能在持有 Delivery 行锁后反向获取 privacy 锁。

原始 chat source 的 owner/session 归属必须在 SQL 验证，不靠 handle 外层声称。历史删除或编辑也应参与规范版本/来源失效；当前 append-only 内容可将“行存在、owner、不可变内容摘要”作为初始 revision 契约，不造一个只在内存递增的版本。

提交 Response、启用 cache、写入 appraisal 与领取投递之前，都在各自短事务重新验证同一 lease。整条 LLM 工作不需要跨模块长事务，但任何派生提交都不能只有事务前的检查。失败只说明该提交未获确认，不虚称其他模块已经发生的事实也回滚了。

## 贯穿生成、工具和投递

1. 可信入口保留原 source epoch/消息 ID；retrieve_for_generation 返回 ContextPack + 有身份的历史 + ContextLease。
2. LLM、表达评估和工具上下文使用这份不可变 lease；不在每个 phase 重新检索并默默替换来源。必要的新检索形成新的有界读集扩展，并重新验证原读集，不能刷新原 epoch。
3. Response 创建、完整回复提交、缓存写入和 Memory 捕获携带 context_id。MemoryWriteContext 继续承担规范写回校验，并合并真实生成来源；不能只把“当前用户消息 ID”当作整个模型输出的来源。
4. clear/forget 在推进 privacy epoch 的同一事务中使旧 lease 不再可用，并标记相关未完成 Response/Delivery/candidate 撤销。更细的 memory revision、到期和政策变化也在每次 gate 查询中验证。
5. 事务后的本地通知用于立即取消 LLM、停止 token/TTS admission、调用原精确 playback/body fence。通知不承担可靠性；多实例 worker 在短轮询/现有 heartbeat 和每个持久化 admission 再查数据库。
6. 当前已提交给设备的音频或动作是现实事实，不能抹去。输出 gate 要区分新内容 admission 与已开始 submission 的晚回执：前者撤权后禁止，后者仍记最后事实或 Unknown。

数据库不能与网络的物理发送组成原子事务。需要明确线性化点：clear 事务提交后不再批准新输出 admission；提交前已经批准但在途的片段立即请求精确取消，并保留已发生/未知事实。如果产品要求 clear 返回即确认所有在途身体停止，API 必须等待全部已登记 owner 的 fence/join，并能返回未确认设备；不能仅凭 DB epoch 已更新宣称物理停止。

## 历史展示与模型重用是两个视图

clear_memory 不等于删除整个会话。用户原始消息和既有会话的展示保留；删除会话/原始消息是另一项明确操作。用户之后主动重新表达内容，形成新 source，仍可再次记忆。

模型上下文读取必须区分：

- 用户原始消息：保留真实 message ID、owner、原 source epoch，不把旧行改签为当前输入。是否允许作为短期会话上下文，按明确 history policy 决定。
- assistant/tool 派生消息：必须取得生成它的有效 ContextLease；旧记忆或关系来源失效后不能自动重注入。没有来源的历史派生行不可当作可信的新来源。
- 已执行工具事实：独立从 Execution 账本取得事实；带私密内容的结果仍需对应 owner 和使用政策。取消上下文不能把现实动作改成未发生，也不能直接把原 tool JSON 当作新的用户表达。

一个必要的实施决策是旧用户原文的“可聊”与“可重新写入长期记忆”边界。建议保留其会话展示和有意查询，但不让自动 Memory 抽取使用 clear 前的来源。若旧用户原文仍进入主 LLM，后续 AI MemoryStore 必须保守继承全部实际上下文来源；不能接受模型声称它只依据新消息。独立 Memory 抽取可只接纳当前 epoch 的真实用户来源，不以受旧上下文影响的助手回复作为替代证据。这比简单删除所有历史更准确，也避免新请求给旧私人内容改签。具体模型历史窗口政策需在实施接口冻结时由 root 明确，而不是在 provider 中暗自清空。

## Cache 是有版本的派生内容

CacheEntry 返回不可变 cache_id + revision + context_id；查询、审核、embedding 回填和删除均使用它们，不能只按 query_hash 操作。

- put 使用数据库原子 upsert；替换内容推进 revision 并清空旧 embedding。有效 owner、scope、TTL、policy 和 context 都由 SQL gate 限制。
- embedding 在事务外计算，回填需匹配 cache_id/revision/context_id、当前有效 lease、query digest、embedding profile/model revision；旧行删除重建同 hash 或同 ID 改版都不能接受旧结果。记录实际 profile，不能仅凭维度混用。
- Insights 审核携带原 cache revision/context，activate/delete 都用 CAS；旧审核不能激活新版本，更不能以 LLM 的 memory_independent 布尔值取消来源依赖。
- 私人语义缓存严格 owner + scope 隔离。下一包优先删除私人查询自动回退 `user_id IS NULL` 的路径。若确有公共缓存需求，只允许明确的可信静态公共来源独立 namespace；不能将匿名/缺 owner 视为公共内容。
- clear 后旧缓存必须立即逻辑不可读，并在删除事务或可靠擦除任务中移除应遗忘的私人正文/向量；不能仅等待 TTL。有效性查询不能依赖通知已及时清理物理行。

## Companion 与关系投影

先把真实存在的 memory recall → appraisal → affect/relationship → prompt 路径接入 context_id。AppraisalWrite 入队、落库及读取投影都核验来源；旧排队写入不可重新产生已撤销证据。持久投影记录规范 revision 与支持它的 lease，失效后从仍有效证据重建，不继续沿用累加后的旧数值。内容与审计理由只保留最小必要信息，不能把删除的记忆正文复制到长期日志。

后续 Observation/Interpretation/InitiativeCandidate 复用同一 ContextLease 与来源规则，机会和政策 gate 在实际投递前再验证。当前不新增通用 Companion 插件平台，也不将该主线误记为完成或延期。

## 文件所有权与实施顺序建议

| 包 | 明确范围 |
| --- | --- |
| Context/Memory 数据 | 新 `modules/memory/context.rs` 或同模块小范围拆分，`providers/adapter.rs` Context/Memory 类型行域，`providers/memory/{simple,privacy}.rs` 与真实 PG 测试 |
| Cache | `providers/cache/*`、`entity/llm_cache.rs`、Insights 仅 cache review 段、cache 测试 |
| History/生成来源 | `providers/conversation/database.rs`、adapter Conversation 行域、`channels/pipeline.rs` 和 `pipeline/{streaming,voice,tools,memory_capture}.rs` 的 lease 调用段 |
| 输出/配置整合（root） | `modules/delivery/{types,repository,service}.rs`、`api/settings.rs`/settings 持久化政策版本、必要 Builder 注册、共享新迁移与 manifest |
| Appraisal/关系 | `expression/{runtime,affect}.rs`、`affect/persistence.rs` 与关系投影测试；在 root 当前 Affect 包冻结后独占接手 |

先冻结 ContextLease、policy/history 语义和新迁移；再并行完成一致性读取与 Cache CAS；随后接 Pipeline/Delivery、Appraisal/关系撤销，最后做跨入口故障测试。已提交迁移不可改，具体新迁移编号由 root 在实施时分配。无历史用户数据迁移负担，但全新安装和未来迁移完整性必须保留。

## 关键真实失败测试

1. PG 两个连接：节点读取后并发 clear/修改，图和 Pack 不得形成混合可用 lease；错误不被当作 empty-memory cacheable 成功。
2. LLM barrier：读 Pack/历史后 clear、forget、暂停、恢复或源 revision 改变；旧结果不能提交 Response/cache/Memory/appraisal，新输入和新 lease 可以正常工作。
3. 缓存 embedding 慢返回：同键覆盖、删除重建、清除后新建、profile 变化；旧回填均失败，新版本回填成功。审核模型慢返回不能 activate/delete 新 revision。
4. 两 owner、私人/公共/scope 混合候选；私人查询不能选到别人的记录或没有公共来源证明的 NULL owner 条目。
5. 上轮助手回复含旧记忆，clear 后新一轮不得从历史再引入；用户原文和会话展示仍存在，用户重新主动输入后可以重新记忆。旧 tool JSON 不能冒充当前用户来源。
6. 两个 Server 实例：一个 clear，另一个 LLM/Delivery/appraisal 队列等待；丢通知也由 DB gate 拒绝旧 admission，精确取消不影响清除后新输出。
7. 输出正在提交时撤权：保留提交前已发生的事实、最终回执和 Unknown；不能伪报零输出或物理停止，也不能继续批准新片段。
8. PG trigger/断连：lease、派生提交及撤销记录没有半事务状态；commit ACK 不确定时不刷新来源重试。租约与来源行缺失、NULL 版本一律 fail closed。
9. Memory recall 已进入 appraisal 队列但尚未写库，或已形成关系摘要后再遗忘；旧证据不回写、旧投影不再进入 prompt，有效无关来源仍可重建。

这些测试尚未运行。当前 Memory 和 Delivery 既有测试的通过记录只能支持它们已经实现的边界，不能作为 ContextLease 全链撤销验收。
