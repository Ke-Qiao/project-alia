# CP05 设备本地 capture gate 实施合同

2026-09-28：本合同的 Core/Proto/Server/Embedded 接线及软件定向矩阵已实现，完整配对回归为 9 passed / 0 failed / 0 ignored。源码已交还 Root 冻结，当前只更新文档；最新整体状态以 [恢复入口](resume-checkpoint-2026-09-28.md) 和 [checklist](checklists/companion.md) 为准，开发期详细证据见 [voice handoff](handoffs/2026-09-28/voice.md)。本合同不宣称 CP07 全部故障、平台或真机验收完成。

历史设计基线为 V3 frozen source，619 files，manifest SHA256 `8ea6d4f7cc63b82d22b8c584c8c0b426b11e04f952f64bd15b7776d352cc18a1`；这不是当前实现的冻结 manifest。原设计阶段 production gate=false 的临时限制已由专用 Context/policy/source admission 与设备闭环实现替代：当前 production actor 为 `initiative_output_enabled: self.initiatives.is_some()`，由 ServerBuilder 的 live 依赖接线；默认 owner policy 仍为 Shadow，snapshot-only actor Harness 保持 false。启用依赖并不绕过持久准入，也不等于部署或整体验收通过。

## 实现边界

实现在现有 Core + Proto + Server OutputProducer + Embedded PlaybackSession/PlaybackControl 内完成，复用原 TTS/Playback 通道与 native write gate，没有增加通用硬件层或伪造用户 Complete。

设备 `PlaybackSession::accept_capture` 持有 Session mutex，先更新真实 CaptureControlBarrier，再调用 control.observe_capture，最后本地 try_send；因此网络中或尚未上传的新 speech 已有正确本地时钟。Core `PlaybackControlState::observe_capture` 对已 arm 的 initiative 立即创建或加强 local Hold，原 control 把有效 Held/Pausing 同步到 native gate。Register 在同一 Session mutex 内检查实际 capture 后才分配 owner 并 arm；Start 复用该 control。原先“只记录 speech、不能本地关门”的缺口已关闭。

采用边界：**Register 成功前发现新 speech 可以拒绝本次 initiative，无媒体；Register 一旦成功，随后 speech 一律先可恢复 Hold（包括 Start 尚未到达），由原 Server Hold/Continue 接管。** 这比按“是否已听见首个样本”猜测更确定，也无需额外 started 状态。绝不因为一次新的 speech 就永久 Cancel 已注册主动输出。只有真正取消、来源/捕获所有权丢失、固定 deadline 到期、资源上限或 backend 丢失沿现有终止路径退役。

首次 actor.reserve 使用冻结 admit_before（原机会/决策/捕获新鲜度）；原输出建立后使用另一个冻结 output valid_until（原 candidate/source 截止时间）。短机会过期不取消已经接纳的原媒体，恢复不能刷新任何期限；详见总设计的两段期限契约。

## 1. 原始不可变身份

在 `crates/alia-core/src/output_control.rs` 的 OutputBinding 中已加入：

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InitiativeCapture {
    pub initiative_id: Uuid,
    pub request_id: Uuid,
    pub through_sequence: u64,
}

// OutputBinding 内：
#[serde(default, skip_serializing_if = "Option::is_none")]
pub initiative_capture: Option<InitiativeCapture>,
```

scope 中已有 boot/session/epoch，context_id 已在 binding，不重复。两个 UUID 必须非 nil；sequence 0 合法，不能拿 0 当 absence。普通输出 `None`，序列化仍省略字段，使原普通绑定/control intent digest 字节保持不变。

`crates/proto/proto/playback.proto` 添加对应 message 与 OutputBinding 可选 message field 5；codec 双向保留并严格验证。Register ACK、Start、Control、Observe、Retire、持久化原 intent 必须携带完全相同的 binding，不能重新取当前 capture 水位“刷新”身份。

Server `OutputProducer::constrain_initiative` 用原 producer Inner 的 OnceLock 固定 `(context_id, initiative_capture)`，并通过原 `tighten_deadline` 收紧期限。N 来自 move-only permit 中原 `opportunity.through_sequence`；actor 的最后 drain/recheck 校验原 speech revision、scope 和首次 admit_before，不能用后来到达的 silence 水位改写 N。producer 和 InitiativeAdmitted 使用同一 N；此后新的 capture 可形成 Hold。相同约束重试只允许同 identity，deadline 仅缩短。`output.rs` 内 Register binding 和清理时重建的 retirement binding 均使用该原身份。

跨 RPC 不要求 SessionEvent 先到：Playback Fence/InitiativeAdmitted/注册允许现有网络重排；Register 自身在同一真实 capture owner 上验证。之后事件和已注册 binding 若都可见，应比较 initiative/request/context/scope/N 的原身份；事件不能开 gate。

## 2. 能力协商不能复用 V3 的布尔 true 作为完整证明

历史 V3 的 `initiative_admission = true` 只说明解析 InitiativeAdmitted foundation，不能证明本地 gate。因此当前实现不能仅凭它放开 live。

已新增精确支持级别 `initiative_capture_gate_version: u32`，Open/Ready 与 RegisterOutputReceipt 都报告 1；旧 peer 缺省 0。Server 需要原 voice peer 协商为 1，且原 Register ACK 为 1、完整回显 initiative binding，才可发送媒体。普通输出 None 无此新增要求。更新端拒绝未协商的 initiative，也拒绝当前不支持的版本 2；不能降级成普通 binding。

## 3. 原子接纳用现有哪把锁

线性化锁是 `PlaybackSession.state: Mutex<Option<ActiveSession>>`，通过现有 `self.lock()` 进入。不能只读 CaptureControlBarrier 自己的锁然后解锁再 Register。

Register 在该锁内、`control_generation` / `begin_scoped_playback` 之前检查：

1. binding.scope 是当前本地活跃、非 retired scope，原 binding auth 仍持有外层读准入；scope boot/session 不可由新 RPC 激活。
2. `active.capture` 必须存在、真实、未 retired；从其 `watermarks()` 得 `(through, speech)`。
3. `through` 已存在且 >= N，`speech` 不存在或 <= N；未知 capture 直接拒绝。
4. 检查通过才分配原 native owner、创建 control、initialize_capture，并一次性 arm initiative protection。相同 Register 重试只返回原 registration，不重新初始化/续期/开 gate。

已实现的内核接口：

```rust
// 在现有 control.state mutex 内调用；N 来自原 binding。
arm_initiative_capture(N, maximum_hold_ms) -> Result<()>
// 供 begin_bound 在同一 Session mutex 下核对原 capture 和 gate。
validate_initiative_start(through: Option<u64>, latest_speech: Option<u64>) -> Result<()>
```

首次及后续 `begin_bound` 必须复用已注册的原 Arc<PlaybackControl>，不得 `new()` 替换。Start 再核对 capture 与 control 的连续水位一致。若 Register 后已经出现 speech，原 gate 必须已 Held/Pausing；可以接受到有界队列但不能开 native write。若已经有覆盖该 speech 的新鲜合法 Resume，则允许原 gate Running；若水位变化但既无有效 gate 又无已接受的覆盖性 Resume，拒绝 Start，不把它当静音。

内部维护 `released_through`（初始 N，仅在真正 Applied 的新鲜 Resume 后前进），避免首次 Start 前 Hold→Continue 已完成时仍把原 N 当最新许可而误拒绝。它不是新的外部权限或可由 API 传入的数据。

锁序保持：binding admission read guard →（Start 还有现有 task_gate）→ Session state → 短 CaptureControlBarrier mutex → Control state / gate atomic。释放 Capture mutex 后再做 backend allocation/注册；Session state 仍持有使 capture 不可插队。现有 backend Shared mutex → Control state 顺序不变。持 Control state 不回调 Session/backend；不在这些锁内 await。Native CPAL callback/ALSA admission 继续仅读原 gate atomics，不新增 callback mutex。capture 的 enqueue 仍是非阻塞 try_send。

## 4. 本地 Hold 与 Server 正式 Hold 的接管

不要为本地 speech 伪造 `ControlCommand`、command_id、Server hold_id、detection 或 Applied receipt。也不要在任何 snapshot 时把 Running 盲目同步到 atomic，从而清掉本地 gate。

纯 `PlaybackControlState` 已加入 initiative mode 与一个有界、非 wire-authority 的 provisional local gate：

```rust
struct InitiativeProtection {
    released_through: u64,
    maximum_hold_ms: u64,
    local: Option<LocalCaptureHold>,
}
struct LocalCaptureHold {
    through_sequence: u64,
    expires_ms: u64,
}
```

`observe_capture(sequence, true)` 对 initiative mode：更新真实 speech 后立即建立/加强 local gate；首个本地 Hold 固定 expires，后续 speech 只推进 watermark，绝不续期。budget 不超过当前 Ready 协商的 maximum_hold_ms。普通输出 None 的 observe_capture 行为保持不变。

`PlaybackControl::observe_capture` 在同一 control mutex 中把 **有效状态** 同步为 HELD atomic，并在 `accept_capture` 返回/try_send 之前完成。已进入的短 native write 允许按原设计结算，新的 `ScopeWrite::enter` 立即拒绝；CPAL 输出零并保留 cursor，ALSA 不提交新 chunk，并保留已接受偏移。依赖现有写入计数，不能声称即时物理扬声器静音。

正式 Pause H 到达：依旧接受原 Server 的 command sequence/hold_id；和 provisional local gate 并存或接管，但有效 gate 从始至终关闭，绝无短暂 Running。正式 expires 取 `min(原 local expires, now + budget)`，不会从 RPC 到达时间重新开始。若 Pause 只覆盖更旧 speech，不能清除更晚 local gate；最新 speech 仍由 capture clock保护。

合法 Resume 到达：沿现有 `validate_decision` 检查 original scope/hold/detection、命令顺序、`through <= capture`、`through >= latest speech`、明确 Wait 只接 ExplicitContinue、native writes 已 quiescent、未到固定 deadline。所有条件成立后在一个 control mutex 内同时清 formal Hold 与覆盖范围内 local gate，推进 released_through，并同步 Running；不删除队列、不新建 generation、不重新合成或重发前面媒体。

ConfirmExplicitWait 只加强原 hold，不释放 local gate或延长期限。重复 Pause/Continue、Observe、snapshot 只返回原 immutable outcome 加当前事实；不得重放状态转换。已 Applied 的旧 Continue 在新 speech 后重复查询仍可有 outcome Applied，但当前事实必须为 Held/Pausing，gate 不得打开。

## 5. 要如实表达本地 Hold；不能借用不完整正式 Hold

原 ControlSnapshot Held/Pausing 验证只允许完整的 hold_id/reason/remaining_ms。provisional local gate 没有 Server hold_id，因此当前实现增加独立 local 事实，不能只把 phase 改 Held 而填一个虚假 hold_id。

`ControlSnapshot` 已增加可选 `local_capture_hold`（watermark + 剩余预算），phase 表示有效软件 gate：formal Hold 或 local Hold 存在即 Held/Pausing，Retired 优先。formal hold_id/reason/remaining 保持原 Server 含义。验证允许“完整 formal Hold”或“有效 local_capture_hold”支撑 Held；Running 要求二者都不存在；Held 仍要求 writes quiescent。两者并存时有效 deadline 是较早者。

该字段已同步到 Core + proto codec + receipt validation，None 省略，旧普通输出 JSON 不变。`recorded_receipt` 和 `with_snapshot` 使用同一 control mutex生成真实当前状态，observation_sequence 仍由现有原 owner 分配。

Server `apply_control_receipt` 对 Resume 只能在最新事实 **有效 Running 且无 local gate** 时开服务端 gate。本地更晚 speech 造成“旧 Resume outcome Applied、当前 Held”时，保存事实并保持原输出 Held，接入下一次正式控制，不会将这种正常竞争永久 retire。repository/control 已校验 snapshot 并经同一事务的当前 Context 授权和原 durable grant 对齐 binding/request/N/scope，原控制期限不得宽于 grant.valid_until；续播不重新使用短 admit_before。

Resume ACK 区分两种结果：若命令因较新 speech 被拒绝，原正式 H 仍存在；若命令曾 Applied、随后 speech 才建立 local-only gate，则 H 已在设备上结束，不能让 Server 永远保留 H 并继续向它 Resume（会 NotHeld）。后者在 Server 原 output gate 内原子保持关闭、结束本地旧 H 的控制意图，按正常递增 command sequence 建立新的正式 H2/Pause，绑定同一 output/scope/backend，预算取原约束与 local 剩余预算的较小值；这是同一输出的下一次 Hold，不是新 initiative/scope/Response，也不发送媒体重试。只有 H2 正式接管且来自真实最新 capture 的后续语义证据允许，才能 Resume。若正式 H 已存在则保留它，不重复 mint H2。该区分由 receipt 的当前 hold_id + local_capture_hold 和不可变 outcome 决定，不能仅按 outcome Applied 推断设备 Running。此竞争已在 Server+Embedded 配对测试中验证，包含 Applied 后丢 ACK、原 Observe 返回当前 local Held、H2 接管和 fresh ExplicitContinue 恢复原剩余媒体。

`PlaybackService::control` 的 Pausing 等待需区分 formal hold 和仅 local gate；旧 Continue 查询不应等待命令里已经结束的 hold_id。用原 binding/control owner 等待 entered writes settle，随后重新 Observe 原 command，不能因超时发新scope/重复Resume。现有 drop不释放owner的语义保留。

## 6. 实现文件范围

- Core：`crates/alia-core/src/output_control.rs`（可选身份、校验、digest兼容），`crates/alia-core/src/playback_control.rs`（provisional Hold、deadline、release watermark、快照）。
- Proto：`crates/proto/proto/playback.proto`、`voice_session.proto`、`crates/proto/src/output_control.rs`（可选身份、local gate事实、精确capability）。
- Server：`grpc/voice_session.rs` 与 `grpc/voice_session/initiative.rs`（新能力、固定N），`modules/delivery/service/output.rs`（OnceLock身份、Register/Retire binding、正常竞争下Hold恢复），`modules/delivery/service/output/gate.rs` 与 `modules/delivery/repository/control.rs`（H2 接管、当前 Context 及原准入身份/期限校验）。
- Embedded：`module/playback_session.rs`、`playback_session/capture.rs`、`control.rs`（原子验证/arm/Start）；`module/speech/duplex.rs`（协商及事件一致性）；`grpc/playback_service.rs`（仅local Pausing等待）。
- Native：复用 `module/cpal_audio/playback.rs` 现有 ScopeWrite gate，未改写 CPAL/ALSA 算法。native 软件测试覆盖实际 CPAL callback buffer 和 ALSA partial-write 记账路径；没有增加线程，也没有实际 ALSA 设备或声学验证。
- 构造兼容：原普通 OutputBinding literal 补 None，新增 capability/snapshot fields 同步 codec、测试及原生产构造点；不使用历史 rg 匹配数量作为完整性证明。
- Companion/Context/policy 由同轮其他组件完成，包含专用 Companion Context Purpose、持久准入 grant、原 SourceAuthorization 复核和 live 依赖接线；生产 actor 仍必须经这些条件才能取得一次性的输出授权。

## 7. 已运行的软件矩阵与后续 checklist

以下勾选依据开发期日志及实际断言，不表示冻结后的全部 CP07 已通过。组件定向为 Core 31 + Proto 5 + Embedded 4 + Server initiative 9 + Server H2 gate 1，共 50 个唯一测试；完整 `duplex_playback` 为原 6 项加本轮 3 项，共 9 项。命令、日志前缀与失败修复记录均见 [voice handoff](handoffs/2026-09-28/voice.md)。

- [x] Core：新 speech 本地 Hold、正式 Pause 接管、旧 Applied Resume 的当前 Held 事实、H2 明确等待/继续、首个 local deadline 不续期及到期退役；真实连续捕获和 released_through/Start 检查。
- [x] Core/Proto：普通 None 序列化兼容、零水位合法、initiative 纳入不可变 intent、精确 capability、local Hold codec 与回执校验。
- [x] 设备原 Session 锁竞争：`module/cpal_audio/playback/tests/control.rs` 用线程屏障验证 speech 先于 Register 时拒绝且无 native owner；Register 后 speech 先于 Start 时保留同一控制门与媒体，并可恢复。测试落在真实 Session/native fixture，不是原计划另造 gRPC 控制 fixture。
- [x] Native 软件边界：同一测试分别执行 CPAL callback buffer 和 ALSA partial-write 记账；已进入写入允许结算，之后 local gate 截住提交，原 cursor/generation 保留，H2 ExplicitContinue 只继续剩余帧。End 已接收也仍可 Hold/恢复。
- [x] 协议重排与能力：已注册后迟到的 InitiativeAdmitted、同 epoch Fence、相同 Register 不能清除 local Held；duplex initiative 测试拒绝 boot/session/epoch/watermark 变化与旧 foundation-only gate version 0，也拒绝当前不支持的 version 2。
- [x] Voice actor 定向：未知 capture/当前原输出包括 Held 不产生 initiative opportunity；认证期间到达 speech 被重查；snapshot polling 不延长捕获 idle；snapshot-only Harness 的 false gate 保持无输出。该 Harness 关闭门测试不是 production 仍关闭的证明。
- [x] 真实 PG control：原 Context/grant 的 capture N、request/scope 必须精确相同，注册及后续原输出控制期限不得宽于原授权期限，短 admit_before 不是后续 Resume 的期限。
- [x] 完整 live 正向：scanner → durable Fresh → 原 actor → 固定 TTS → 真实身份绑定的配对 PlaybackService，只有一个 Response/admission/媒体 Push/TTS。
- [x] 完整 H2 竞争：真实设备应用 H1 Resume 后且 ACK 返回前发生新 local speech；丢 ACK 后只 Observe 原命令，由原 owner 建立 H2/Pause，再以 fresh ExplicitContinue 恢复；40 + 120 = 160 原帧，没有 scope 替换或重播。
- [x] 完整源撤销：TTS 合成期间删源，等待原 runner/output shutdown 后复查从未发第一媒体，原 charge 保留。
- [x] 原完整配对回归兼容：首 Start 前 Hold、满媒体队列、End 后尾部、控制 ACK 丢失、Register 回执晚于 Fence、旧输出清理不能影响替换 session 的新 owner，共 6 项均通过。满队列项使用原正式 Hold 场景，不扩称已穷举 initiative local Hold 的所有队列组合。
- [x] 原计划两项组合的生产路径审阅：native fresh Resume 先于第一 Start、initiative local Hold 恰遇满 native 队列均复用已验证机制，没有额外生产分支或当前发现的实质故障缺口；无需为穷举组合新增测试，也不作为 CP06 收口阻塞。下文明确证据组成，不声称两项独立配对排列已运行。
- [x] Root 已完成冻结 V1 的 workspace、实际 ignored 场景及两平台编译：1041 unique macOS、630 输入一致，源码提交 `a79cb1f`。开发期 `frozen=false` 或占位 `source_drift=[]` 不作为冻结证明。
- [ ] 真实 Pi5 + macOS 音频、VAD/AEC、双讲、实际扬声器停声/尾音、恢复完整性、Easy-Turn 实际推理效果与长时间运行另行验收。软件 native 提交边界不等于物理声学事实。

两项组合的审阅依据：

| 组合 | 原生产路径与已有证据 | 收口判断 |
| --- | --- | --- |
| fresh Resume 先于第一 Start | Core `PlaybackControlState::apply(Resume)` 推进 `released_through`；已通过的 `initiative_local_hold_survives_old_applied_resume_and_transfers_to_next_hold` 在真实 Apply 后调用 `validate_initiative_start`。Embedded `PlaybackSession::begin_local` 对 initiative 无条件调用同一 validator，且使用原注册 generation/control；`initiative_register_then_speech_before_start_retains_original_media_for_cpal_and_alsa` 已验证 Start 前 local Hold 与原 native owner。 | 没有根据“第一 Start 是否发生”分出的新控制路径；状态与原 owner 接线均已有证据，不需要新增一份相同机制的配对排列。 |
| initiative local Hold 恰遇满 native 队列 | `playback_session/capture.rs::accept_capture` 在上传前调用 `PlaybackControl::observe_capture`，后者直接在 control mutex 内同步 HELD atomic，不获取媒体队列锁。`PlaybackSession::enqueue` 在 `submit_audio` 回压前释放 Session 锁；`PlaybackSender::submit` 的 condvar 等待释放 shared mutex。local 与 formal Hold 共用同一 gate/`ScopeWrite::enter`。已通过 native initiative 测试验证 local 截断 CPAL/ALSA 提交，已通过 `held_full_native_queue_keeps_original_pcm_and_control_lane_available` 验证满队列下控制、恢复与原 PCM；现有 native `hold_control_bypasses_full_media_queue_and_blocks_conversion_until_resume` 也直接覆盖队列实现，其本轮全量重跑由 Root 记录。 | local Hold 的关门路径不依赖队列容量或排队控制消息；没有新增锁环或媒体满时的专用分支。已有两端证据加路径审阅足以覆盖必要故障机制，不把独立排列数量作为完成条件。 |

前端与通用嵌入端适配继续按 [deferred](deferred.md) 延后。本文件更新不启动服务、用户设备或部署，也不改变冻结源码。
