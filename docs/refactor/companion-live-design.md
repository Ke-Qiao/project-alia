# CP05 Companion 真实主动表达设计（实现中 / 待完整验收）

2026-09-28：本合同已进入当前工作树实现与定向验证；最新状态以 [恢复入口](resume-checkpoint-2026-09-28.md) 和 [checklist](checklists/companion.md) 为准。下文保留原设计依据，不代表全部故障、平台或真机验收已完成。

最初设计于 2026-09-26；本次实现状态更新于 2026-09-28。CP05 的持久准入、专用 Context、固定邀请 runner、原 voice actor 与设备 capture gate 已实现。定向 Companion 38/0/0、Runtime/HTTP 7/0/0、完整 duplex 配对回归 9/0/0 已分别通过，均为冻结前结果，不能相加作为最终 workspace 数。当前冻结 V1 为 630 inputs，manifest SHA-256 `58c76e82adf71fed96fa6b113f51185412dd3bd0619a3db747835f3d81accd4a`；Root 正在执行统一验收，CP07 尚未完成。本文不作为 Pi 5 真机或声学验收证据，也不表示已启动用户设备。

约束以 [Companion 契约](companion-contract.md) 为准。近端语音的临时避让和明确暂停沿用[自然全双工可恢复暂停契约](duplex-pause-design.md)及[控制协议](duplex-control-protocol.md)：临时疑似插话可经新鲜语义确认恢复，明确 Wait 必须等待明确继续。CP05 不得用每次近端语音都永久取消的实现替代这项已确认的产品要求。

## 1. 首轮目标与状态归属

首轮只发送规范 `FollowUpInvitation` 的固定文本：

> 如果你愿意，我们可以再聊一会儿。

不调用 LLM，不接受自由文本，不增加工具或身体动作，也不宣称固定邀请已经实现完整 Affect / Regulation 语义。来源仍是已有 canonical Appraisal 及完整 Context 依赖闭包；机会来自原存活 VoiceSession actor，预算和政策来自 SQL。调用方提交的 JSON、冻结决策结果、Presence 或模拟机会均不能自行成为权限。

| 所有者 | 新增职责 | 保留的边界 |
| --- | --- | --- |
| `modules/companion` | live pending 生命周期、政策及预算复核、持久 admission、固定邀请 runner | 不自行分配 voice generation，不拥有第二套媒体 owner |
| `memory/context` | 专用邀请 Context 的发行与逐次验证、完整来源闭包、撤权 | 不扩大普通 `Generation` 的授权范围，不改写原 Appraisal parent |
| `interaction` / VoiceSession actor | 原机会复核、唯一 generation 分配、返回原 `Arc<OutputProducer>` | snapshot 只提供事实；数据库 commit 不证明 actor 或设备已经接纳 |
| `delivery` | 规范 Response、精确内容约束、唯一派发和媒体 / Hold owner | 不因 Existing、未知 ACK、恢复或重启新建替代媒体 |
| Embedded capture / playback owner | 首次 Register / Start 的本地竞争屏障、临时阻断及原输出恢复 | Server 尚未看到的本地 speech 也必须阻止错误启动 |

默认仍为 Shadow。当前 ServerBuilder 已装配 live 依赖，具备 live readiness 的实例允许 active owner 显式将持久政策改为 Enabled；未装配 live 的实例仍拒绝该值。原 actor 与设备还必须协商精确的主动输出 capture gate 版本。源码装配和软件测试没有自动启用政策，也没有部署或打开用户设备；完整平台与冻结验收仍由 CP07 收口。

## 2. 候选生命周期与稳定身份

shadow 的一次取样及 opaque 去重墓碑不能作为 live 待办队列。live 需要独立、有界的状态：

```text
canonical source -> pending
pending -> pending       忙碌 / 临时缺少新鲜机会，原 TTL 不变
pending -> admitted      同事务占预算 + 专用 Context + Response prepare
pending -> expired       原 TTL 到期
pending -> retired       来源 / 政策失效或明确关闭
admitted -> retired      撤权、失效或原 owner 结束；不会回到 pending
```

候选 ID / dedup 沿用原 owner、source event、固定 intent 和 generator version，不含政策版本、扫描时间或 operation ID。created_at 绑定原 canonical effective_at；当前 UserMessage 来源还需保持 effective_at 等于原 source_received_at，不能通过 rebuild 或扫描刷新 TTL。旧 Shadow 候选不可因启用新政策而获得 live capability；同源重建或换政策不能重新获得表达资格。

同一 live 候选只能有一次持久 admission。机会暂不可用时可以在候选原有效期内重新取得机会并重评；发生真实准入后，无论物理结果成功、失败或未知，都不得用新身份再次派发。重评必须有界并受退避、容量和停机规则约束，不能忙循环。

admission、initiative request、Context 的身份必须在第一次事务意图中固定；可按版本化规则从稳定候选身份派生，不能每次重试重新生成。现有 Delivery 可在第一次插入时产生 Response ID，后续通过固定 `owner + initiative_request_id + output_key` 查回同一记录。

## 3. 新请求与原 Appraisal parent 的关系

真实主动输出需要新的 `initiative_request_id`，以避免占用原用户请求的 Response key。新 Context 保持原 owner、source event、conversation、channel、source_received_at 和 source privacy epoch，只把当前输出的 request_id 设为新的 initiative request。

`conversation_id` 是 chat session，不能拿 VoiceScope 的实时 `session_id` 替代。VoiceScope 的 session、device boot 和 generation 另外冻结到 admission 与固定响应内容。

新 Context 的 dependencies 必须含原 parent 的完整 roots、所引用的 Appraisal revision / result digest、实际消费的 Relationship 证据，以及本次 admission 证据。原 parent、Appraisal row 的 request_id、source_fact 和时间全部保留。

无需修改 Appraisal parent header 的依据来自现有校验逻辑：

- [`validate_record_reference`](../../packages/alia-server/src/modules/memory/context/appraisal.rs) 验证的是 **原 parent.source.request_id 等于 Appraisal row.request_id**，并在原 parent 上验证 source_fact；它没有要求新派生 lease 的 request_id 等于原请求。
- [`validate_appraisal_parent_header` / `validate_parent_header_inner`](../../packages/alia-server/src/modules/memory/context/repository.rs) 验证同 owner、Context policy、privacy epoch、parent 用途、有效期与完整根包含关系，不要求新旧 request_id 相同。
- [`DeliveryRepository::validate_context`](../../packages/alia-server/src/modules/delivery/repository.rs) 则将新 Response key 的 request_id 与新 lease 绑定。这与原 Appraisal 的 parent 校验分别覆盖两个不同的身份关系。

因此要新增专用派生 Context，而不是把原 Appraisal 的 parent 改成新请求。普通历史 materialization 目前对 parent 有自己的 Generation 限制；CP05 不顺便将 Companion 历史开放给 Memory / Appraisal / Affect。将来如需消费这类历史，应另行明确它的来源规则。

## 4. 专用 Context 与不可变邀请内容

已在 [`context/types.rs`](../../packages/alia-server/src/modules/memory/context/types.rs) 实现：

```rust
ContextPurpose::CompanionInvitation

ContextEvidence::CompanionAdmission {
    id: Uuid,
    intent_digest: String,
}
```

`CompanionInvitation` 只授权已有的 `ContextUse::ResponseCommit` 和 `ContextUse::OutputAdmission`，不新增泛化的 ContextUse，不授权 Execution、AppraisalWrite、AffectRead、Memory、Cache 等用途。必须恰有一个与用途匹配的 CompanionAdmission 证据；普通 Generation 即使拷贝这一证据也不能取得主动表达身份。

[`context/companion.rs`](../../packages/alia-server/src/modules/memory/context/companion.rs) 已提供仅供服务内部调用的发行接口，具体实现在 `companion/invitation.rs`：

```rust
pub(crate) async fn issue_invitation_in_transaction(
    tx: &DatabaseTransaction,
    owner: Uuid,
    admission_id: Uuid,
) -> ContextResult<ContextLease>;
```

接口自行读取 durable admission、候选、canonical Appraisal 与 parent；不接受调用方上传的 roots、epoch、scope、自由文本或权限声明。Context 使用原来源头和新的 request_id，materialized_digest 覆盖下述完整、类型化的固定响应内容。输出 valid_until 固定为原 candidate.expires_at（已取来源与原 TTL 的较早者）。短机会另用 admit_before 控制首次准入及 actor reserve，二者不混用；发行、重试、heartbeat 或恢复均不得延长任一期限。

固定响应 envelope 至少包含：

```text
Response.kind        = companion_invitation
Response.output_key  = companion_invitation
content.schema_version
content.invitation_kind = follow_up_invitation
content.generator_version
content.text         = CandidateKind::FollowUpInvitation.text()
content.voice_scope  = admission.expected_scope
content.candidate_id
content.admission_id
content.source_event_id
```

使用一个固定类型及 `deny_unknown_fields`，明确定义 UUID、版本、文本、JSON 字节数边界，统一用已有 canonical digest 实现比较。不要散落多个近似的 JSON 拼装或仅比较 `text`。first admission 的 `expected_scope` 和机会 identity 也属于不可变 intent。

不得复用 `SourceNotice`：当前 SourceNotice 仅授权 OutputAdmission，不能提交规范 Response history；Delivery 控制绑定还显式拒绝 `source_notice`。强行复用会混淆两种权限与可恢复输出语义。

## 5. Delivery 已实现的内容校验

Delivery 私有 `validate_context` 现返回已经验证的 ContextLease，再以 `validate_response_intent(lease, key, kind, content)` 同时保留原 notice 限制和专用 Companion 限制。固定内容检查位于持久入口，runner 自身的固定 literal 不能替代这些检查。

| 具体入口 | 必须验证的约束 |
| --- | --- |
| [`prepare_in_transaction`](../../packages/alia-server/src/modules/delivery/repository.rs) | 专用 purpose、固定 output key / kind、完整内容 digest、原来源头、admission / request / expected scope 一致 |
| 同文件 `validate_response_context` | 每次重新读取的已存 Response 仍符合专用 Context 的完整 intent；不能仅首次 prepare 验证 |
| 同文件 `mark_generated_in_transaction` | 除大小边界和 prepared content 外，**另验传入 final_content 恰为固定邀请的 `{"text": literal}`**；邀请的 committed 重试也重验当前 Context |
| 同文件 `prepare_media` | representation 的 text 必须为固定 literal，媒体 identity 的 scope 必须为同一 expected scope；限制固定邀请所需的输出数量 / output key，不能让通用 256 个 representation 上限变成重复邀请许可 |
| [`repository/text.rs`](../../packages/alia-server/src/modules/delivery/repository/text.rs) 的 Start / `offer_text_chunk` / `offer_text_end` | 本轮固定邀请只允许音频，以上文字入口逐一明确拒绝；不能借通用文字投递重放邀请 |
| 固定邀请 runner / TTS 调用 | 只把冻结 literal 交给受信 TTS；不能把候选私密描述、Appraisal 自由文本或模型生成内容作为实际媒体输入 |

普通已 committed Response 的相同 final_content 重试保留历史幂等读取；固定邀请还会重验当前专用 Context。两者都不会重建被擦除 assistant message，也不发行新的输出权限。SQL 对文本和 metadata 的限制不能证明任意 PCM 真正念了这段话；本轮配对测试使用明确受控的 TTS/native fixture，不作声学内容验证声明。

## 6. Durable admission 的最小完整持久契约

独立迁移 `m20260926_000015_companion_live.sql` 已加入 `companion_live_pending`、`companion_admissions`、跨模式 `companion_source_claims` 及来源擦除关联表。历史 shadow decision 不会变为可消费 grant。admission 冻结：

| 字段组 | 内容与约束 |
| --- | --- |
| 身份 | admission_id、candidate_id、owner_id、稳定 dedup、initiative_request_id、output_key、context_id |
| 原来源 | source_context_id、原 source event、candidate digest；来源细节使用既有有界规范结构，不复制 raw text |
| 决策 | Companion policy revision / digest、generator / decision version、decision digest、预算版本 |
| 机会 | opportunity identity、server instance、原 snapshot scope、speech revision、through_sequence、观察时间与截止时间 |
| 唯一输出 | expected_scope、完整固定响应内容及 digest |
| 计费和期限 | charged_at、UTC budget_date、不可延长的 valid_until |
| 状态 | active / retired、枚举 retirement reason；仅允许单向退休 |

唯一约束覆盖 candidate_id、owner + dedup、owner + initiative_request_id + output_key，以及同一原机会的消费身份。所有可重试意图均不可变；只允许更新有界运行事实与退休状态，不能修改内容、来源、政策版本、scope 或到期时间。

在同一事务内先插入 admission、再写 Context、再 prepare Response 时，可以为 context_id 使用 `DEFERRABLE INITIALLY DEFERRED` 外键，Context ID 事先固定，commit 时完整校验。不要为了循环引用先提交半成品 grant。初版无需在 admission 强制保存 response_id：固定 Response key 已足够找回唯一 Response，避免再造一个循环 FK。

计预算的是已承诺的准入尝试，包括后续失败、撤销和未知结果；撤销与换政策不退款、不清空每日计数、不移除去重。否则并发或丢 ACK 可重复联系用户。等待回应只能由新的 canonical 用户 source 解除，Presence、静音或“没有回答”不能代替回应。初版按原 conversation / channel 的新用户来源判定回应，仍复核 source 权限。

源撤销、privacy / relationship clear、Appraisal 修订以及政策撤销必须同事务使 admission 失效并擦除私密派生快照。仅保留预算 / 去重所需 opaque tombstone，擦除后的 replay 不再返回原 inputs、内容 hash 或私密原因；不能从日志补回。

live pending 私密记录上限为每 owner 4 / 全局 128，私密 grant 为每 owner 128 / 全局 1024；容量检查在同 schema 的事务容量锁内完成，跨 schema 测试不会共享全局锁。runner 最多 4 个并发尝试，且同 voice channel 同时只有一个尝试，避免后续 snapshot 覆盖已冻结机会。过期清理每批有界，保留拒绝迟到请求所需的去重与 charge 身份。

## 7. 同事务占预算与准备 Response

当前内部接口将一次冻结与原事务准入分开：

```rust
impl LiveRepository {
    async fn prepare_intent(
        &self, candidate_id: Uuid, opportunity: VoiceOpportunity,
    ) -> LiveResult<LiveAdmissionIntent>;

    async fn admit(&self, intent: &LiveAdmissionIntent) -> LiveResult<AdmissionResult>;
}
```

`VoiceOpportunity` 必须来自已绑定的原 actor，不提供反序列化的外部调用入口。repository 根据 candidate 固定 owner，验证 opportunity 的 owner / channel / server instance / session / boot；不允许 caller 选择另一位 owner 或任意 scope。离线 Simulation 永远不会走这一接口。

事务阶段按以下顺序完成；外部网络、TTS 和 actor reserve 均在 commit 之后：

1. 无锁读有界候选头，以定位 owner / source；按现有顺序锁 Context policy、active account、privacy、relationship。
2. 读取并锁完整 canonical source roots、原 parent 和 Appraisal，确认候选原时间、版本、有效期、完整依赖与当前权限。
3. 锁该 owner 的 Companion policy 及预算序列化点，复核当前持久 Enabled、quiet / paused 关闭，以及冻结政策 revision / digest 完全匹配。
4. 在同一 owner 锁下重读 candidate / admission 去重、UTC 当日已准入数、最后准入时间和 canonical 回应事实，以本事务的显式 DB 时间构造 BudgetSnapshot。纯 decide 必须得到 Allowed；shadow_would_allow 不能提交。
5. 固定 admission / initiative request / Context 身份、expected_scope、完整内容与 deadline，记录本次预算消费。
6. 插入 admission；调用 `issue_invitation_in_transaction`；调用现有 `DeliveryRepository::prepare_in_transaction(&tx, NewResponse)`。
7. commit 后才返回 Fresh permit，随后才能请求原 actor reserve、绑定原 OutputProducer、生成固定 TTS 和进入已有 Delivery 流程。

现有 [`DeliveryRepository::prepare_in_transaction`](../../packages/alia-server/src/modules/delivery/repository.rs) 已提供事务内 prepare、Response key advisory lock、不可变 intent digest 与同 key 幂等检查，可以直接复用。不能在这条路径调用会另开事务的 prepare service，否则“最后一次预算 + Response”不再原子。

[`DatabaseConversationProvider::commit_response`](../../packages/alia-server/src/providers/conversation/database.rs) 已把 `mark_generated_in_transaction` 和 assistant history 写入合为一个事务。CP05 可以在预算 + prepare 原子完成后，使用这个既有提交入口；不必为了预算原子性把全部 history 提交也提前塞进准入事务。固定内容限制必须先进入 `mark_generated_in_transaction`。

锁序总约束是 **Context authority → 完整 source roots → Companion policy / budget → candidate / admission → Response → Delivery / control**。正常 Context 验证不能先锁 admission 再等待 source；来源擦除触发器会按 source → admission 更新，反向会形成死锁。可无锁读取 admission 头定位来源，在原 roots 验证后再锁 admission 并重查 digest。CompanionAdmission 的 dependency 排序应位于其原来源证据之后。

政策变更、清除与准入必须沿一致锁序；如果事务后段调用 Delivery 再次验证 roots，该事务必须已经持有完整相同 roots，不能在拿到 admission / Response 锁后首次发现新依赖。迁移触发器也要按相同图审阅，避免普通统计字段更新触发全 owner 清除。

## 8. Fresh / Existing、提交未知与单次能力

实施决定已补充于 [存储契约](companion-live-storage-design.md)：READ COMMITTED 下持有同一 pending 行锁至事务完成，重试获得锁后再查询原 admission；存在则 Existing，无记录且原意图/来源仍有效才证明可按原意图重试。Existing 仅返回 opaque receipt，不在擦除后重返 private Response。设备本地可恢复门与迟到 Resume 竞争详见 [设备契约](companion-live-device-design.md)。

当前返回类型明确区分新权限与不带私密输出的回执：

```rust
pub(crate) enum AdmissionResult {
    Fresh {
        permit: LiveAdmissionPermit,
        context: ContextBinding,
        response: ResponseRecord,
    },
    Existing(AdmissionReceipt),
    Deferred(Vec<DecisionReason>),
    Retired,
}
```

`LiveAdmissionPermit` 不实现 Deserialize，不可从查询结果、冻结 JSON 或 Existing 构造；字段仅模块内部可写，消费一次后不能 Clone 再用。可沿用 Delivery 的 DispatchPermit / ControlPermit 模式，无需新增通用 capability 框架。只有确认本次新事务成功提交的路径能返回 Fresh。

数据库 commit ACK 未知时，返回带原 admission identity 的 `CommitUnknown`，查询 / 重试只观察原记录与结果。如果实际已提交，则返回 Existing；不能因为原调用者没收到 Fresh 而补发一个可消费 permit。数据库明确回滚才允许按同一不可变 intent 再尝试事务，不能更换 candidate、request 或 output key。

只有 Fresh 可以进入 actor reserve。actor reserve ACK 未知、reply waiter 被丢弃、Fence / InitiativeAdmitted / Register / Start ACK 未知时，保持原 token、identity、预算和 Delivery 的 Unknown / 清理事实；不能重新分配 scope、Response、producer、媒体或 permit。原 owner 仍负责收尾，查询是恢复事实而非重放权限。

## 9. 与现有 VoiceSession actor 的接线

当前 [`VoiceInitiativeRequest`](../../packages/alia-server/src/modules/interaction/initiative.rs) 消费不可 Clone/Deserialize 的 `LiveAdmissionPermit`，其中固定 ContextBinding、原机会、scope 和两段期限。actor 先验证 OutputAdmission，再 invalidate 分配新 generation、fence 并创建输出。顺序为“durable admission / Context / Response prepare commit 在前，原 actor reserve 在后”，无需发行未授权临时 Context。

expected_scope 从真实 opportunity 的 scope 计算：session 和 device boot 相同，generation 为原值 `checked_add(1)`。这只是冻结预期，分配权仍属于 actor；[`TurnController::invalidate`](../../crates/alia-core/src/turn.rs) 恰好增加一次 generation。actor 已在分配后显式比较实际 scope 与 admission.expected_scope，溢出或不一致均拒绝。

actor 继续执行原 owner / channel / session / boot、机会 identity、generation、speech revision、近期 capture、无 utterance / detection / retry / 活输出的检查，并在每次 await 后 drain 已到采集和重查。当前 snapshot TTL 为 5 秒、actor capture freshness 为 2 秒；纯决策更宽的上限不放松 actor 的真实门槛。speech revision 为 0 可以表示尚无 speech 的有效空闲 capture 窗口，不能把 0 一概判为缺证据。

`LiveAdmissionPermit` 是 reserve 请求的实际能力输入。返回的原 `Arc<OutputProducer>` 通过 [`constrain_initiative(context_id, capture, deadline)`](../../packages/alia-server/src/modules/delivery/service/output.rs) 绑定专用 Context、immutable capture N，并缩短 deadline；后续绑定 Response/control 时再次检查一致。

实施契约明确区分两段期限：`admit_before = min(原 decision.valid_until, opportunity.expires_at, last_capture_at + 2s)`，只允许在严格早于此值时完成持久准入与原 actor 的最终预约核验；`grant.valid_until = candidate.expires_at`，已经受原 source 与原候选 TTL 约束，最多为既有策略允许的 300 秒。首次预约成功后，原 OutputProducer 的媒体与可恢复 Hold 只受这份固定输出期限及当前来源/政策控制，不再把旧机会当作持续空闲事实。后续 fresh capture 通过原 Hold/Continue 处理。Context 验证保留历史 Allowed 决策、历史准入时间 `< admit_before`，当前时间必须 `< valid_until`；纯 replay 仍使用原冻结 as_of。机会过期后不可补做首次 reserve，也不可换新机会重试未知提交；输出截止时间也不会因恢复、重新扫描或 heartbeat 刷新。应测试短机会已过但既有输出可在原期限内恢复，以及两个边界相等时均拒绝新权限。

在 actor 尚未建立输出 owner 前发现新 speech / epoch 变化，属于失效的首次准入，拒绝这次预约且保留已消费预算。输出 owner 建立后发生近端 speech，则进入原输出的可恢复避让 / Hold 流程；不能把同一条件无差别实现成 permanent cancel。

## 10. 每次 Delivery 验证都必须看到政策撤销

新增 CompanionAdmission evidence validator 必须查询 **当前** 持久 Companion policy，而非只验证 admission 内的冻结副本或 decision digest。至少检查：

- admission active、owner / 新 request / source / Context / expected scope / 固定内容完全一致，且仍在原期限内。
- 当前政策 revision / digest 与 admission 一致，mode 为 Enabled，quiet / paused 未启用。
- 原 Context policy、privacy / relationship epoch、每条 canonical root 和 Appraisal revision / digest 仍有效。

经现有 `ContextService::validate_in_transaction` 串入后，Delivery 的 prepare、ResponseCommit、media prepare、dispatch、Start、后续新媒体 admission、非终态 heartbeat，以及控制 Register / Pause / Resume 中取得新权限的分支，都必须覆盖这一检查。实施时按实际调用图逐个确认，不能仅凭“统一入口”推定没有绕路。

政策 / 来源撤销事务应退休依赖 admission，并把绑定 Response 标为 cancel_requested，驱动原 producer / token 停止。迟到的设备事实回执、Unknown 对账、取消与 native 收尾仍可记录；撤权不能阻止 cleanup，也不能因为记录迟到事实而重新授权输出。

数据库授权与物理播放不是分布式原子操作；已提交的 native 前缀仍记在原身份下。进程重启、原 instance / VoiceSession / boot 消失时退休旧机会与 grant，不从 durable admission 重建音频或恢复旧 media owner。

## 11. 设备已捕获、尚未上传 speech 的竞争

Server drain 只能处理已经到达的帧。设备可能已在本地捕获新 speech 并更新 watermark，但帧仍在网络中；此时 Server 的机会与 reserve 检查仍可能认为空闲。Fence、InitiativeAdmitted SessionEvent、Register 和 Start 又可能经不同 RPC 以不同次序到达。仅有 watermark 而没有 capture gate 的旧路径不能建立 Hold；当前实现已加入精确 v1 capability、同步 capture barrier 和可恢复本地门，避免只依赖 Server revision 或 SessionEvent。

首次主动输出必须在设备 capture 所有者的同步屏障内完成下列行为：

1. 验证原 opportunity 的 session / boot / 原 scope、speech watermark 与本地最新 speech 事实，不能仅依据收到的最后一次 Server 状态。
2. 将该 admission、规范 Response 和 expected scope 原子登记到本地输出门；Register 成功 ACK 之前门已经生效，后续 Start / 首个 native write 都必须经过同一个门。
3. 保留登记后新 speech 的本地阻断，不因迟到 Fence、SessionEvent、Register ACK 或 Server 的旧“允许”结果清除。通过 scope、hold identity、单调控制序号和 covered capture watermark 拒绝旧释放。
4. 在已有输出 owner 上建立 `AcousticCandidate` 临时避让，保留原媒体、源前缀、游标、Context 和 generation；由 Server 对新鲜 capture / detection 的独立语义决定恢复或升级明确等待。

登记前已经出现不在原机会覆盖范围内的新 speech，可拒绝尚未成立的首次准入；这时没有原媒体可恢复，不能假称已创建 Held 输出。登记完成后新 speech 则必须得到可恢复临时门，即使 Start 尚在途、TTS 还未产生第一段，也不能丢失阻断事实。门的生命期、有界未匹配控制与确认 / 退休规则沿用原 DU11 控制 owner，不能再建一套独立暂停服务。

临时 AcousticCandidate 经匹配本次 hold 原因及新鲜语义确认只是附和后可以恢复；ExplicitWait 只允许明确继续。恢复原 producer 的未提交部分，不再生成固定邀请、不从头重播、不换 generation。静音、VAD 结束、旧 Backchannel、收到旧 ACK 或仅有时钟经过都不授权恢复。真实撤权、来源失效、明确停止、期限耗尽或原 owner 无法保留媒体，才沿既有永久退休规则终结。

设备临界区只安装 / 检查轻量状态、取得原 owner，不在 capture 锁下做网络、SQL、模型或等待 native 收尾。必须审阅现有 capture → PlaybackSession → backend gate 的锁关系，保持控制与收尾在原 owned task 上进行。此软件门不证明所有真实近端语音都被 VAD 检出，也不证明硬件排队尾音已经消失。

## 12. 当前实现文件与验收归属

| 文件范围 | 已实现职责 |
| --- | --- |
| `modules/companion/repository.rs` 与 `live/` | live pending、原子预算 / grant / prepare、Fresh / Existing、查询与有界清理 |
| `m20260926_000015_companion_live.sql` | admission / pending schema、不可变约束、必要 FK / 唯一性、来源和政策撤销闭包；未回写已完成 shadow migration |
| `modules/memory/context/types.rs`、`repository.rs`、`companion.rs` | 专用 purpose / evidence、issuer、source-first 验证与排序；不扩大 Generation |
| `modules/delivery/repository.rs`、`repository/text.rs`、相关 control 校验 | 固定 intent、final_content、media / text 内容约束与逐次撤权 |
| `modules/interaction/initiative.rs`、`grpc/voice_session/initiative.rs` | 消费 Fresh permit、actual / expected scope 比较、沿原 actor 的预约与取消 |
| `modules/companion` runner / service 与 ServerBuilder | 固定文本 TTS、原 OutputProducer / Response 接线、有界任务与关闭；默认 Shadow，具备 live readiness 后才接纳显式 Enabled |
| `proto` 与 Embedded capture / duplex / playback registration 路径 | 明确 admission 和机会屏障、不同 RPC 次序处理、本地临时门及原 Hold 恢复 |
| 现有 PG / VoiceSession / Delivery / Embedded / loopback 测试位置 | 下面的定向用例；随后由 Root 统一组合、workspace 与目标平台验证 |

上述职责已进入 V1 冻结输入；实际文件与哈希以 [630 输入 manifest](../../run/validation-20260928/final-source-manifest-v1.json) 为准。当前已通过的组件/配对定向证据分别保存在 [policy handoff](handoffs/2026-09-28/policy.md)、[repository handoff](handoffs/2026-09-28/repository.md) 和 [voice handoff](handoffs/2026-09-28/voice.md)；统一 workspace、平台检查及正常提交已完成；630 输入逐项与已提交 `a79cb1f` 一致。

## 13. 实际验收与证据边界

R01 CP01—CP07 已收口。源码提交 `a79cb1f`，630 输入 V1；完整 workspace 1036/0/5，另行真实 PTY 3、实际 Easy-Turn 2，共 1041 个不同 macOS 测试；macOS/Linux ARM64 两端 no-default all-targets 通过。统一结果见 [验收摘要](handoffs/2026-09-28/validation-summary.json)。以下以实际故障路径为单位，不声称穷举全部排列。

- [x] 独立 pending、shared source claim、原 TTL、不升级旧 shadow、重启退休与私有容量：7 live 和9 shadow PG 用例覆盖；同 schema 不同 owner 容量锁用 `pg_blocking_pids` 验证。
- [x] 原事务对账与原子性：真实 Response INSERT 故障、production TX staged rollback/commit、COMMIT deferred deadline 失败；只在已确认新提交后产生 Fresh，Existing 无 permit。没有逐一注入 grant/Context INSERT 故障或物理 PostgreSQL commit ACK 丢失。
- [x] 不可退款预算：两个真实事务并发争抢最后额度仅一个 Fresh；政策改版、退休和擦除不退款。此用例使用 `tokio::join!`，没有强制最后额度的 SQL 等待 barrier；UTC 换日与 awaiting_response 的证据为纯策略测试，未做实际跨午夜 PG 实验。
- [x] 专用 purpose 与完整 parent roots：5 Context PG 用例覆盖新 request、用途限制、固定 envelope/final/media/text 禁止、存储篡改重验、两段期限及撤权；只授予原固定邀请输出。
- [x] live 动态擦除覆盖 policy / message / privacy / relationship 和禁用账户后的清理；Graph/Cache 等共用来源闭包在 shadow/Context 既有回归中覆盖。本轮没有逐一重复全部 live 触发族，不把静态共享路径说成每族单独实跑。
- [x] Runner reserve 拒绝、原 reply 未知、取消、Existing 清理均经过真实 PG + 受控 mailbox；这是 mailbox 故障注入，不是实际网络断连。
- [x] 真实 actor 和软件设备完整正向投递、TTS 期间删源零首媒体、H1 Resume Applied/ACK 丢失后新本地 speech → H2 → fresh ExplicitContinue，保持原 producer/scope/cursor，只有一次 TTS/Response/admission。
- [x] Register/Start 的原 capture N 和本地 Hold 竞争在真实 Session/native fixture 验证，协议重排与正式/临时 Hold 共用门的对应路径见 [设备矩阵](companion-live-device-design.md#7-已运行的软件矩阵与后续-checklist)。没有为相同状态机穷举所有 RPC/队列排列。
- [x] 权限丢失后的清理、同 channel 单尝试、扫描轮转和生命周期排空已有回归；默认 shadow，真实装配支持明确 Enabled，没有自动打开用户设备。
- [ ] Pi 真机双讲、尾音、物理静音/恢复、AEC 与 24 小时运行归 R02/R05；软件提交与模型标签不作为声学通过证据。

上述未单独注入的排列和跨午夜实验是本轮证据的限制；没有据此声称发现生产故障，也没有增加未要求的通用故障代理框架。若后续真实运行暴露相应问题，应保存原输入与故障边界，先补针对性回归再修复。
