# CP05 Companion 真实主动表达设计（待实施 / 未验收）

更新：2026-09-26。本文是 CP05 的实施草案，不代表真实主动表达已经启用，不作为软件、Pi 5 真机或声学验收证据。编写时 R01 shadow 源码处于统一验证冻结期；本文仅新增设计文档，不改变被冻结的源码或现有勾选状态。

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

默认仍为 Shadow。只有完整 live 链及组合验证完成后，才解除 `enabled` 的入口限制；不因存在本文、已持久旧配置或单个测试通过而打开任何用户设备。

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

建议在 [`context/types.rs`](../../packages/alia-server/src/modules/memory/context/types.rs) 新增：

```rust
ContextPurpose::CompanionInvitation

ContextEvidence::CompanionAdmission {
    id: Uuid,
    intent_digest: String,
}
```

`CompanionInvitation` 只授权已有的 `ContextUse::ResponseCommit` 和 `ContextUse::OutputAdmission`，不新增泛化的 ContextUse，不授权 Execution、AppraisalWrite、AffectRead、Memory、Cache 等用途。必须恰有一个与用途匹配的 CompanionAdmission 证据；普通 Generation 即使拷贝这一证据也不能取得主动表达身份。

建议在 [`context/companion.rs`](../../packages/alia-server/src/modules/memory/context/companion.rs) 增加仅供服务内部调用的发行接口：

```rust
pub(crate) async fn issue_invitation_in_transaction(
    tx: &DatabaseTransaction,
    owner: Uuid,
    admission_id: Uuid,
) -> ContextResult<ContextLease>;
```

接口自行读取 durable admission、候选、canonical Appraisal 与 parent；不接受调用方上传的 roots、epoch、scope、自由文本或权限声明。Context 使用原来源头和新的 request_id，materialized_digest 覆盖下述完整、类型化的固定响应内容。valid_until 只能取来源、候选和本次决策 deadline 的较早者，发行、重试、heartbeat 或 actor reserve 都不能延长。

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

## 5. Delivery 内容校验必须补齐的位置

当前 Delivery 的 Context 校验主要返回 notice literal；Generation 没有固定邀请的精确内容限制。CP05 不能只在 Companion runner 检查一次。

建议让 Delivery 私有 `validate_context` 返回已经验证的 ContextLease，再以小型 `validate_response_intent(lease, key, kind, content)` 同时保留原 notice 限制和新增 Companion 限制。不要借此设计通用策略框架。

| 具体入口 | 必须验证的约束 |
| --- | --- |
| [`prepare_in_transaction`](../../packages/alia-server/src/modules/delivery/repository.rs) | 专用 purpose、固定 output key / kind、完整内容 digest、原来源头、admission / request / expected scope 一致 |
| 同文件 `validate_response_context` | 每次重新读取的已存 Response 仍符合专用 Context 的完整 intent；不能仅首次 prepare 验证 |
| 同文件 `mark_generated_in_transaction` | 除大小边界和 prepared content 外，**另验传入 final_content 恰为固定邀请的 `{"text": literal}`**；目前验证 prepared content 不会自动限制 final_content |
| 同文件 `prepare_media` | representation 的 text 必须为固定 literal，媒体 identity 的 scope 必须为同一 expected scope；限制固定邀请所需的输出数量 / output key，不能让通用 256 个 representation 上限变成重复邀请许可 |
| [`repository/text.rs`](../../packages/alia-server/src/modules/delivery/repository/text.rs) 的 `offer_text_chunk` / `offer_text_end` | 如果这类 Response 能进入文字出口，按规范 UTF-8 字节偏移校验每个 chunk 和 End 的完整 literal；否则入口显式拒绝该 medium，不能留下任意文本出口 |
| 固定邀请 runner / TTS 调用 | 只把冻结 literal 交给受信 TTS；不能把候选私密描述、Appraisal 自由文本或模型生成内容作为实际媒体输入 |

已 committed Response 的相同 final_content 重试是历史幂等读取，可以保留现有“不重建被擦除 assistant message”的行为；它不发行任何新的输出权限。SQL 对文本和 metadata 的限制不能证明任意 PCM 真正念了这段话，实际 TTS 输入及唯一 producer 路径仍须验证。

## 6. Durable admission 的最小完整持久契约

建议新增 `companion_admissions`，而非把历史 shadow decision 改造成可消费 grant。至少冻结：

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

live pending、active admissions、私密 snapshots 和运行任务均须有每 owner / 全局上限；容量检查必须在同一容量锁内完成，不能用并发 COUNT 后无锁插入。沿用当前 shadow 的有界清理原则；live 的具体数值须在 migration / repository 常量中统一确定并测试，不能借 shadow 已有上限声称新表已受限。过期清理只处理有界批次，不能删除仍用于拒绝迟到请求的去重身份。

## 7. 同事务占预算与准备 Response

建议内部接口：

```rust
pub(crate) async fn admit_invitation(
    &self,
    candidate_id: Uuid,
    opportunity: VoiceOpportunity,
) -> CompanionResult<AdmissionResult>;
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

建议返回类型明确区分：

```rust
pub(crate) enum AdmissionResult {
    Fresh {
        permit: LiveAdmissionPermit,
        context: ContextBinding,
        response: ResponseRecord,
    },
    Existing {
        admission: AdmissionReceipt,
        response: ResponseRecord,
    },
}
```

`LiveAdmissionPermit` 不实现 Deserialize，不可从查询结果、冻结 JSON 或 Existing 构造；字段仅模块内部可写，消费一次后不能 Clone 再用。可沿用 Delivery 的 DispatchPermit / ControlPermit 模式，无需新增通用 capability 框架。只有确认本次新事务成功提交的路径能返回 Fresh。

数据库 commit ACK 未知时，返回带原 admission identity 的 `CommitUnknown`，查询 / 重试只观察原记录与结果。如果实际已提交，则返回 Existing；不能因为原调用者没收到 Fresh 而补发一个可消费 permit。数据库明确回滚才允许按同一不可变 intent 再尝试事务，不能更换 candidate、request 或 output key。

只有 Fresh 可以进入 actor reserve。actor reserve ACK 未知、reply waiter 被丢弃、Fence / InitiativeAdmitted / Register / Start ACK 未知时，保持原 token、identity、预算和 Delivery 的 Unknown / 清理事实；不能重新分配 scope、Response、producer、媒体或 permit。原 owner 仍负责收尾，查询是恢复事实而非重放权限。

## 9. 与现有 VoiceSession actor 的接线

当前 [`VoiceInitiativeRequest`](../../packages/alia-server/src/modules/interaction/initiative.rs) 已要求 ContextBinding，actor 会先验证 OutputAdmission，再 invalidate 分配新 generation、fence 并创建输出。因此采用“durable admission / Context / Response commit 在前，原 actor reserve 在后”的顺序，无需发行未授权临时 Context。

expected_scope 从真实 opportunity 的 scope 计算：session 和 device boot 相同，generation 为原值 `checked_add(1)`。这只是冻结预期，分配权仍属于 actor；[`TurnController::invalidate`](../../crates/alia-core/src/turn.rs) 当前恰好增加一次 generation。CP05 需让 actor 分配后显式比较实际 scope 与 admission.expected_scope，溢出或不一致都拒绝。

actor 继续执行原 owner / channel / session / boot、机会 identity、generation、speech revision、近期 capture、无 utterance / detection / retry / 活输出的检查，并在每次 await 后 drain 已到采集和重查。当前 snapshot TTL 为 5 秒、actor capture freshness 为 2 秒；纯决策更宽的上限不放松 actor 的真实门槛。speech revision 为 0 可以表示尚无 speech 的有效空闲 capture 窗口，不能把 0 一概判为缺证据。

`LiveAdmissionPermit` 应作为 reserve 请求的实际能力输入，或先由模块内接口消费后构造不可外部伪造的请求；不能只靠注释约定“调用方记得仅 Fresh 调用”。返回的原 `Arc<OutputProducer>` 通过已有 [`constrain_initiative(context_id, deadline)`](../../packages/alia-server/src/modules/delivery/service/output.rs) 绑定专用 Context 并缩短 deadline，后续绑定 Response 时检查一致。

候选 / source / decision / opportunity 截止时间均只能收紧。当前纯决策 deadline 包含短机会期限；初版应按原合同严格执行，接受慢 TTS 可能到期无输出。若产品需要将“最后准入时间”与“已准入输出剩余寿命”分开，应先增加明确且可验证的两段期限契约，不能在 runner 中隐式刷新五秒机会或 ContextLease。

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

Server drain 只能处理已经到达的帧。设备可能已在本地捕获新 speech 并更新 watermark，但帧仍在网络中；此时 Server 的机会与 reserve 检查仍可能认为空闲。Fence、InitiativeAdmitted SessionEvent、Register 和 Start 又可能经不同 RPC 以不同次序到达。现有 fence 会替换旧 control，watermark 本身不会建立 Hold，故只检查 SessionEvent 或 Server capture revision 不足以封住这个窗口。

首次主动输出必须在设备 capture 所有者的同步屏障内完成下列行为：

1. 验证原 opportunity 的 session / boot / 原 scope、speech watermark 与本地最新 speech 事实，不能仅依据收到的最后一次 Server 状态。
2. 将该 admission、规范 Response 和 expected scope 原子登记到本地输出门；Register 成功 ACK 之前门已经生效，后续 Start / 首个 native write 都必须经过同一个门。
3. 保留登记后新 speech 的本地阻断，不因迟到 Fence、SessionEvent、Register ACK 或 Server 的旧“允许”结果清除。通过 scope、hold identity、单调控制序号和 covered capture watermark 拒绝旧释放。
4. 在已有输出 owner 上建立 `AcousticCandidate` 临时避让，保留原媒体、源前缀、游标、Context 和 generation；由 Server 对新鲜 capture / detection 的独立语义决定恢复或升级明确等待。

登记前已经出现不在原机会覆盖范围内的新 speech，可拒绝尚未成立的首次准入；这时没有原媒体可恢复，不能假称已创建 Held 输出。登记完成后新 speech 则必须得到可恢复临时门，即使 Start 尚在途、TTS 还未产生第一段，也不能丢失阻断事实。门的生命期、有界未匹配控制与确认 / 退休规则沿用原 DU11 控制 owner，不能再建一套独立暂停服务。

临时 AcousticCandidate 经匹配本次 hold 原因及新鲜语义确认只是附和后可以恢复；ExplicitWait 只允许明确继续。恢复原 producer 的未提交部分，不再生成固定邀请、不从头重播、不换 generation。静音、VAD 结束、旧 Backchannel、收到旧 ACK 或仅有时钟经过都不授权恢复。真实撤权、来源失效、明确停止、期限耗尽或原 owner 无法保留媒体，才沿既有永久退休规则终结。

设备临界区只安装 / 检查轻量状态、取得原 owner，不在 capture 锁下做网络、SQL、模型或等待 native 收尾。必须审阅现有 capture → PlaybackSession → backend gate 的锁关系，保持控制与收尾在原 owned task 上进行。此软件门不证明所有真实近端语音都被 VAD 检出，也不证明硬件排队尾音已经消失。

## 12. 建议实施文件与验收顺序

| 文件范围 | 计划改动 |
| --- | --- |
| `modules/companion/repository.rs` 与局部 admission 子模块 | live pending、原子预算 / grant / prepare、Fresh / Existing、查询与有界清理 |
| 下一条独立 migration | admission / pending schema、不可变约束、必要 FK / 唯一性、来源和政策撤销闭包；不回写已完成 shadow migration 冒充先前已验收 |
| `modules/memory/context/types.rs`、`repository.rs`、`companion.rs` | 专用 purpose / evidence、issuer、source-first 验证与排序；不扩大 Generation |
| `modules/delivery/repository.rs`、`repository/text.rs`、相关 control 校验 | 固定 intent、final_content、media / text 内容约束与逐次撤权 |
| `modules/interaction/initiative.rs`、`grpc/voice_session/initiative.rs` | 消费 Fresh permit、actual / expected scope 比较、沿原 actor 的预约与取消 |
| `modules/companion` runner / service 与最少装配 | 固定文本 TTS、原 OutputProducer / Response 接线、有界任务与关闭；完成前仍拒绝 live enabled |
| `proto` 与 Embedded capture / duplex / playback registration 路径 | 明确 admission 和机会屏障、不同 RPC 次序处理、本地临时门及原 Hold 恢复 |
| 现有 PG / VoiceSession / Delivery / Embedded / loopback 测试位置 | 下面的定向用例；随后由 Root 统一组合、workspace 与目标平台验证 |

上述是建议范围，不是已修改文件清单。具体 migration 序号、局部类型名与容量常量由实施包确定后冻结；不得为了避免接口协调改成外部 JSON authority 或泛化 Generation。

## 13. 待实施与待验收清单

以下全部未勾选；每项必须有实际代码与验证证据后，再由主 checklist 记录完成。本文不自动同步既有 [Companion checklist](checklists/companion.md)。

- [ ] live pending 与 shadow tombstone 分离；busy / unknown 可有界重评，TTL 永不刷新，旧 Shadow 不升级。
- [ ] 稳定 candidate / admission / request / Context / Response key；同源、换政策、rebuild 和重复 operation 不重复表达。
- [ ] 专用 CompanionInvitation 只授权 ResponseCommit / OutputAdmission；其他 ContextUse 全拒绝。
- [ ] 新 request 成功发行派生 Context，原 Appraisal parent 与 request 不变；缺任一 canonical root 均拒绝。
- [ ] 假 owner、外部 roots / epoch / scope / opportunity、Simulation 和普通 Generation 伪造 invitation 均拒绝。
- [ ] 最后一次每日预算的两事务屏障测试只有一个 Fresh；policy CAS、UTC 换日、cooldown 和 awaiting_response 并发均保持不变量。
- [ ] 同事务 grant + Context + Response：任一写入失败全部回滚，无半成品权限或免费输出。
- [ ] Fresh permit 不可查询重建；Existing / commit ACK 未知 / reserve ACK 未知不重派、不新建 scope、不退款。
- [ ] 固定 content 在 prepare、stored Response 再验证、final_content、media prepare、text chunk / End 全覆盖；自由文本、额外字段、scope 篡改和重复 representation 拒绝。
- [ ] 每次输出或恢复验证都观察当前 Companion policy；关闭、quiet、paused、revision 变化与 dispatch / Start / Resume 竞争均失效。
- [ ] privacy / relationship clear、原 source / Appraisal / Memory / Cache / Graph 变化覆盖完整擦除闭包，源锁与 grant 锁顺序无死锁。
- [ ] private evidence 过期 / 擦除后 replay 不泄漏 inputs / 内容 hash；opaque tombstone 保留必要去重与预算，存储和 GC 有硬上限。
- [ ] actor 始终为唯一 generation 分配者；expected scope 溢出 / 不一致、原 session / boot / instance 消失、非空闲 / capture 过期拒绝。
- [ ] 合法零 speech revision 可接纳；缺 capture 或伪造 bound owner 不可接纳；await 后新到 speech 不能穿过 Server gate。
- [ ] 设备已捕获未上传 speech 在 Fence 前、Fence 后 Register 前、Register 后 Start 前及 Start 期间均有受控竞争测试。
- [ ] Fence / SessionEvent / Register / Start 乱序、丢 ACK、重复 ACK 与旧 scope 不清除本地更新的阻断。
- [ ] 登记后的 AcousticCandidate 是可恢复临时避让；新鲜附和可恢复原未提交媒体，ExplicitWait 只能明确继续。
- [ ] 恢复不重生成、不重放前缀、不换 generation、不产生第二份 Response/history；旧 Resume / 超期 Continue 不开放门。
- [ ] 权限撤销和永久取消仍可清理、记录迟到事实；重启不恢复旧媒体，原 owner / task 全部有界收尾。
- [ ] DB / loopback / Embedded 软件组合验证通过后再解除 enabled 限制；默认仍 Shadow，不自动打开用户设备。
- [ ] macOS Server + Pi 5 的真实双讲、尾音、恢复完整性、AEC 及长期运行单独记录；不把软件测试当真机通过。
