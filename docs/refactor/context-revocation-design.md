# ContextLease 与派生上下文撤销契约

2026-10-17补充：普通Generation可按专用只读证据引用已提交CompanionInvitation，输出授权与历史事实分离，完整来源/TTL/隐私/政策继续约束派生。当前契约见[邀请历史设计](companion-invitation-history.md)；下方2026-09-14状态与验证数字为原包历史，不能代替最新进度。

状态：2026-09-14，`a33ec67` 已实现 Context、Cache、Pipeline、Execution 和输出接纳的来源校验。后续 Memory 向量身份包通过 macOS 默认 workspace 802 项整体回归，详细证据见[构建记录](progress.md)。本文替代 2026-09-08 的只读设计，区分已落地契约、定向证据和未完成主线。**派生 Memory 的递归父根、Appraisal/Relationship 和 Companion 尚未全部合并，不能据本包宣称全链清除已经完成。**这些工作不属于用户指定的延期范围。

## Context 的身份、用途与实际读集

`modules/memory/context/` 提供 PostgreSQL 持久化的 `ContextLease` 和只读物化结果 `GenerationContext`。迁移 `000007` 建立不可更新的 `context_leases`、部署级 `context_policy`，并给 Response、Execution 增加 `context_id`。Lease 保存原始来源、用途、政策版本、证据集合及摘要；模型 JSON、旧适配器快照或任意 UUID 不能生成有效权限。

原始 `ContextSource` 包含 owner、request、channel、conversation、source event、原始接纳时间和 `source_privacy_epoch`。Generation 必须来自真实已提交的用户消息，首次查询正文必须等于该行原文。SQL 校验 active owner、会话归属与 channel、消息 role、原 epoch 和时间戳；数据库以微秒精度比对原时间，不用 Server 与数据库时钟的先后关系授予权限。派生操作的 `ContextBinding` 必须与原 request/source 一致。

| 持久用途 | 可以授权的操作 |
| --- | --- |
| `generation` | 正常生成、Response、Execution、输出和派生写入；每项仍受当前政策、原始来源和规范证据约束 |
| `source_notice` | 仅固定错误提示的 `OutputAdmission`；不能提交 assistant 历史、执行工具、写 Cache/Memory 或发起 Body 动作 |
| `memory_capture` | 仅 `AutomaticMemoryWrite` |
| `memory_maintenance` | 仅 `MemoryMaintenance` |

证据来自规范 SQL 行：`Memory(id, revision)`、`UserMessage(id, session, epoch, content_digest)`、`DerivedMessage(id, response_id, context_id, content_digest)`、`Graph(edge_id, projection_digest)`、`Cache(id, revision, context_id, scope, query_digest, content_digest)` 和 `Execution(effect_id, intent_digest, completion_digest)`。图证据同时合并支撑投影的规范 Memory 根；显示三元组或模型声称的 evidence IDs 不能替代来源。

初始物化在短的 repeatable-read 事务中完成 Memory、图和模型可用历史的一致读取。查询向量请求之前先在有界事务中验证持久原文、账户、epoch；扩展已有 Context 时还验证全部原始根。向量在事务外计算，回来后仍核对原政策。没有可信匹配向量时可明确走规范关键词检索；规范表缺失、SQL 失败或来源变化不能被包装为“没有记忆”的成功结果。

硬上限为物化内容 128 KiB、证据 JSON 64 KiB、256 个证据引用、64 条历史、12 个 Memory 结果和 24 条图候选；实际政策可以进一步收紧。正文选择、裁剪与根集建立一起完成：不进入模型的完整历史不加入根集，已采用的 Memory 片段仍携带其完整规范 revision 依赖。超限时跳过完整候选或拒绝物化，不能只丢祖先凭据。Insights 使用捕获接口返回的完整有界内容，不再在登记来源后截掉提示尾部。

组合 assistant 历史、缓存和工具结果时，合并并去重已有的规范根，保留直接派生记录关联，避免每轮无界追溯历史链。`valid_until` 取依赖的最早失效时间。扩展产生新 lease，但继承原 source、epoch、policy 和旧根；不能取得当前快照给旧模型结果改签。提交 ACK 不确定返回 `CommitUnknown`，不表示可以换身份重做。

## 持久政策与发布顺序

`privacy_epoch` 继续使用现有 `memory_privacy_state`，没有另造隐私时钟。部署级 `context_policy.version` 是持久单调版本，覆盖 Memory 读写/暂停/捕获/敏感过滤/整理、历史窗口，以及 Cache 开关、TTL、阈值和 embedding profile 等实际政策。

Settings 的策略字段只修改请求明确给出的部分，其余从当前 DB 政策保留。当前 active admin 才能提交政策，HTTP `ETag`/`If-Match` 支持版本比较；未给 `If-Match` 也以实际读到的版本做 SQL CAS。CAS 确认之前不发布运行时设置。模块应用失败会明确返回错误，不能撤回已经提交的新政策；相同 PATCH 的重试仍进入模块应用和设置文件保存，不能被本地 registry 的“值未变化”过滤掉。

Embedding 设置先用实际 provider 的纯预览计算完整描述，不调用网络，再把 profile 身份与政策一起 CAS。预览确定的 model、dimensions、endpoint 一并发布和持久化，避免仅修改模型时重启混用旧维度。合法配置省略维度时登记推断值；API 的错误类型仍被拒绝。API key 轮换不改变向量身份。重启实际配置与既有 DB pin 不一致时，语义缓存保持不可用，必须显式确认新的模型、维度或 endpoint，不能自动改签旧政策。

启动仅在政策尚不存在时使用配置作为 seed；既有 DB 政策覆盖旧 TOML 投影后，生产者和 Pipeline 才启动。Memory 始终构造真实后端，启用/暂停控制模型访问，不能使存储和清除退化为 Noop。

暂停不删除数据。到期恢复由 `ContextPolicyStore::current()` 在 DB 事务中根据数据库时间清除暂停状态并推进版本；不是旧任务依据本地时钟自行恢复权限。旧 lease 在暂停、恢复或其他相关政策变更后继续失效。

## 锁顺序与在途事实

新接纳先锁政策行，再验证 active user、锁现有 privacy 行、核对规范证据，之后才锁对应 Response/Execution/Delivery/Cache 行。读集采用稳定的证据顺序。Response 保留 `FOR NO KEY UPDATE`，使审计外键的 `KEY SHARE` 不反向形成行锁死锁。先读账本定位来源的入口，会在取得写锁后再次比较不可变意图和 context。

LLM、embedding、TTS 和网络投递都不持有数据库事务。Response 提交、Cache 写入/审核、Memory 写入、Execution propose/approve/claim 和新输出接纳，在自己的短事务内重复校验原 lease。不同模块不共享一场跨网络长事务；一个提交失败不能被解释为其他模块已经发生的事实也回滚。

Insights 的排队、账户复核和整个 phase 共用 30 秒截止时间；超时释放并发名额并报告“完成未确认”，结果日志另有 2 秒截止时间，不继续占用该名额。OpenAI 兼容 embedding 请求也有 30 秒总截止时间，覆盖响应头和完整正文。取消本地等待不证明远端没有执行，也不回滚已提交的 Cache 批准或其他写入；后续任务依据当前持久状态继续，不能把超时统计当作零事实。

清除在原事务中推进 epoch、物理删除该 owner 的 Cache 正文及级联向量，并标记未完成 Response 取消。撤权后的新接纳由 DB gate 拒绝；活动输出通过既有检查/heartbeat 发起原 identity 的精确取消。通知只帮助及时停止，不承担权限正确性。

已交给设备的片段或已领取的外部执行是事实。Delivery/Text/Body 回执和 Execution completion 可以在原来源撤销后记录原执行结果；它们不重新授予发送或执行许可。Unknown 不能重新入队。输出账本支持的迟到 checkpoint 单独审计，不覆盖已经冻结的 Unknown。`Submitted` 只证明后端提交，不证明听到、屏幕呈现或身体位置。

数据库提交不能与物理发送组成原子事务。清除提交后不再批准新的接纳，但此前已批准且在途的片段仍可能产生事实；精确取消及未知结果必须如实记录。当前 clear API 不能被解释为“返回时所有设备已经物理停止”。若需要这种更强产品语义，仍需等待所有已登记 owner 的 fence/join，并报告未确认的设备。

## Pipeline、Execution 与固定错误提示

Pipeline 先持久化原始用户输入，再建立该 turn 共用的 `GenerationContext`。转录纠正、路由、主模型及 Memory 工具使用这份实际来源；Cache 命中或规范 Execution 结果进入模型前先合并证据。需要新 Memory 查询时扩展原读集，不重新取得 epoch。Response 准备会冻结 turn 的 context，冻结后不能再添加来源；Pending 持久化和后续派生提交使用冻结的 binding。

AI Execution 的 propose、approve、claim 必须携带并复核原 context、owner、request、conversation/channel、source event、epoch 与接纳时间。直接 API/调度操作保留各自显式权限边界，不能把派生工具结果冒充用户原文。规范工具结果从持久 Execution 意图及 completion 读取并合并根；原 tool JSON 不是新权限。

Delivery 的 Response 准备、历史提交、媒介准备、dispatch、Start、音频 offer、文本 chunk/End 及续租都验证适用来源。Body claim/authorize 同样检查原 Response 和 scope。已有执行的晚事实不走新的接纳 gate。

处理失败时可以创建独立 `SourceNotice` lease，只包含代码枚举选定的中文或英文固定错误提示摘要，仍保留并验证原 owner/source/epoch。Response 必须使用 `source_notice` 类型和完全相同的文本；文本 chunk 必须是该固定字面量的连续片段，End 必须覆盖完整字面量。它不能升级为普通生成 context，不能写 assistant 历史或授权工具、Cache、Memory、Body；原 source 已被清除时也不能重签提示权限。

## Cache 的版本、向量与清理

迁移 `000008` 删除没有可信 context 的旧缓存，建立明确的 `user_id + scope + query_hash` 私人唯一键。owner 不可为空；exact/semantic 查询均显式提供 owner 和 scope，没有 NULL-owner 公共回退。

`CachedResponse` 携带 `cache_id/revision/context_id`、owner、scope、原 Response、query/content digest、状态与到期时间。`put` 不仅验证 Context，还检查原 Response 属于同一 request/source、已经 committed、未取消，且正文与原 `final_content.text` 相同。数据库原子替换推进 revision、清除旧向量并回到 `pending_insights_review`；重复同一已提交 Response 的回调保留已审核版本。会使自身生成来源失效的 Cache 覆盖被拒绝。

审核前再次授权，模型调用在事务外。`apply_review` 使用原 `cache_id/revision/context_id` 和不可变内容条件 CAS；过期、清除、替换或政策变化后的旧审核不能激活或删除新条目。统计只计实际提交的 Applied 结果。`safe_direct_return`、`memory_independent` 等标记是审核条件，不能消除规范来源依赖。

只有批准后的 active revision 才能请求 embedding。`embed_profiled` 将实际请求配置快照和结果一起返回，验证请求摘要、结果数量/index、有限非零值及确切维度；配置锁在网络等待之前释放。`fill_embedding` 再检查原版本、Context、query digest 和当前政策的 profile pin。向量存入独立 `llm_cache_embeddings`，不再固定为旧 1024 维列；语义查询只在同一个不可变 profile/维度分区计算距离。

Profile 覆盖 provider、endpoint、model、dimensions、distance、vector type 和描述格式版本。**`request-config.v1` 是请求配置格式版本，不是供应商权重版本或权重未变化的证明。**没有可验证 profile 的适配器不参与语义缓存，不能调用普通 `embed()` 后猜测身份。

有效性以 DB 时间、TTL 和依赖最早失效时间为准。后台清理复用 `OwnedBackground`，每 5 分钟处理一批，初始化和审核也执行清理；每次调用硬上限为 256 条，stop 等待 worker 退出。清理以 `FOR UPDATE SKIP LOCKED` 选取到期候选，删除时再次核对同一 `revision/context_id` 和到期状态，因此并发刷新持锁的条目被跳过，已续期的新版本不会被旧候选删除。TTL 清理不能替代 clear：清除必须在同一撤权事务物理删除私人正文及向量，晚审核/晚回填不能写入同键重建的新行。

## Memory 捕获与历史处理假设

Insights 的提取、反思、习惯检测使用 `Automatic`，要求持久 `capture_mode=LlmSelected`；已有 Memory 的整理使用 `Maintenance`，另要求 `consolidation_enabled`。两者都服从启用和暂停政策，不能借 Maintenance 绕过自动捕获限制。`materialize_memory_capture` 在短事务中重读真实用户行和规范 Memory revision，返回专用用途 lease 与规范正文。模型提示只能用这些返回行，不能使用事务外旧副本或没有 lease 的 assistant 文本。

派生 Memory 写入保留完整实际根，通过 `memory_write_context`/`bind_context` 绑定来源，并在 PostgreSQL 写入事务内再次验证政策、敏感过滤、Automatic 阈值与原证据。模型等待期间的 clear、暂停或 revision 变化会拒绝旧结果。显式 AI Memory 工具仍需原生成 lease，不能声称只依赖当前一句话而丢掉模型实际读过的历史。

**当前实现采用的推荐假设，尚未得到用户明确确认：清除 Memory 保留会话展示，但 clear 前的用户原文和已失效派生历史不再进入新的模型上下文。**这是模型重用视图的限制，不是删除会话。新用户主动重新表达内容形成新 source，可按当前政策再次记忆。

模型历史只纳入当前 epoch 的规范用户行，以及正文匹配 committed Response、原 Generation lease 及完整根仍有效的 assistant 行；无来源的 assistant/tool 旧行被排除。展示历史与模型历史分开读取。是否需要允许用户明确查询旧原文、以及这种查询如何限制长期记忆再写入，仍需产品确认，不能在 provider 中暗自放宽。

## 仍未完成的主线

| 主线 | 当前边界与下一步 |
| --- | --- |
| Memory 派生父根 | 实际请求描述符、动态维度、双向版本校验和有界后台重建已实现，见[Memory 向量设计](memory-embedding-design.md)。已提交子 Memory 对父 revision/private/expiry 和关系域根的完整闭包，仍在 Appraisal 包接入 Context、独立检索和投影；自身 revision 检查不能替代递归来源校验。 |
| Appraisal/Relationship | 主提示当前未注入没有 lease 的关系摘要，但真实 `memory recall → appraisal → affect/relationship` 路径仍存在。入队、落库、投影和表达使用尚未统一接入 context/revision 撤销；需要从有效证据重建投影，防止旧队列和累加状态重新引入遗忘内容，审计理由也不能复制已清除正文。 |
| Companion | 尚无完整 Observation/Interpretation/InitiativeCandidate 闭环。后续复用 Context 根集、用途、政策和实际投递前 gate，不新建另一套时钟，也不把该主线列为延期或完成。 |
| 跨路径完整撤销验收 | Context/Cache/Execution/输出的定向证明不能覆盖上述未迁移路径。完成它们后仍需多 Server、丢通知、模型/向量/投影队列等待、clear/forget/暂停/恢复与物理取消组合回归。 |
| 历史产品语义 | 保留展示、排除 clear 前模型历史是当前推荐实施假设；有意访问旧原文及其再捕获边界仍待用户明确确认。 |

本包迁移编号为 `000007`、`000008`；正式提交后的迁移不得重写。用户当前没有历史数据迁移负担，但全新安装、重启和后续迁移的一致性仍必须验收。前端及 Embedded 通用适配的延期范围见[延期台账](deferred.md)，不包含上表主线。

## 当前验证记录

以下是本包已获得的独立定向证据，不相加为最终 workspace 数量，也不代表所有平台验收完成：

| 范围 | 已知定向结果与限制 |
| --- | --- |
| Context | 23 项通过，覆盖规范来源/根集、政策撤权、历史/Cache/Execution 合并与专用用途等边界 |
| Cache 与 Insights | 16 项真实 PostgreSQL 测试通过，覆盖 owner/scope、替换与同键重建、审核及向量晚返回、clear、缺表无 public 回退、Automatic/Maintenance 与长正文实际读集，以及清理跳过并发刷新、保留新 revision 和 256 条硬上限 |
| Insights / Embedding 截止时间 | Insights 单元组 12 项通过，其中虚拟时间测试验证挂起 embedding 后释放 phase 名额、保留已确认的 Cache 批准并允许后续 phase 完成；Embedding 单元组 5 项通过，其中真实 HTTP 测试验证响应正文停滞会触发总截止时间，随后请求恢复 |
| Execution / Memory | Execution service 30、Memory 13、Memory embedding 13、Memory 隐私 24 项通过；请求身份已迁移，递归父根的待合并范围见上表 |
| 输出 | 新增 SourceNotice 固定文本、禁止 history/Body 升权，以及政策撤权后禁止新 offer、保留音频晚事实等定向测试已通过 |
| HTTP Settings | 4 项相关回归已随 Runtime 组通过，包含持久 CAS、重复 PATCH 恢复、profile 发布/重启、关闭 Memory 后的真实后端及清除 |

完整 workspace、特性组合和平台最终记录由主验收更新到[平台验证记录](platform-validation.md)与[进度记录](progress.md)。未完成主线的测试不能用本包上述通过项替代。
