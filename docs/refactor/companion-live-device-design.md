# CP05 设备本地 capture gate 最小实施合同（设计，尚未实施）

基线：V3 frozen source，619 files，manifest SHA256 `8ea6d4f7cc63b82d22b8c584c8c0b426b11e04f952f64bd15b7776d352cc18a1`。此文只读分析后写入 run 目录；没有修改冻结源码、没有追加运行测试。现有 production `initiative_output_enabled = false` 必须保持，直到专用 Context/policy/source admission 和以下设备闭环均完成。

## 结论与当前缺口

可在现有 Core + Proto + Server OutputProducer + Embedded PlaybackSession/PlaybackControl 内实现；不需要新通用硬件层、第二条 TTS/Playback 通道或伪造用户 Complete。

设备 `PlaybackSession::accept_capture` 已持有 Session mutex，先更新真实 CaptureControlBarrier，再调用 control.observe_capture，最后本地 try_send；因此网络中或尚未上传的新 speech 已有正确本地时钟。问题是 Core `PlaybackControlState::observe_capture` 目前只保存 capture/speech，不创建 Hold。`PlaybackSession::fence` 会 retire 旧 control、保留 capture handle；Register 会创建新的 Running control 并 initialize_capture，仍不会暂停。因此 Server drain 只能覆盖已到达帧，无法保证 initiative 不盖住设备端刚开始的用户 speech。

采用边界：**Register 成功前发现新 speech 可以拒绝本次 initiative，无媒体；Register 一旦成功，随后 speech 一律先可恢复 Hold（包括 Start 尚未到达），由原 Server Hold/Continue 接管。** 这比按“是否已听见首个样本”猜测更确定，也无需额外 started 状态。绝不因为一次新的 speech 就永久 Cancel 已注册主动输出。只有真正取消、来源/捕获所有权丢失、固定 deadline 到期、资源上限或 backend 丢失沿现有终止路径退役。

## 1. 原始不可变身份

在 `crates/alia-core/src/output_control.rs` 的 OutputBinding 增加：

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

Server `OutputProducer::constrain_initiative` 扩展为一次性固定 `(context_id, initiative_capture, deadline)`，保存到原 producer Inner 的 OnceLock。actor 在最后一次 drain/recheck 后选定唯一 admission watermark N，producer 和 InitiativeAdmitted 使用同一 N；此后新的 capture 可形成 Hold，但不能改写 N。相同约束重试只允许同 identity，deadline 仅缩短。`output.rs` 内 Register binding 和清理时重建的 retirement binding 两处必须同时接线；只改 Register 会令精确清理冲突。

跨 RPC 不要求 SessionEvent 先到：Playback Fence/InitiativeAdmitted/注册允许现有网络重排；Register 自身在同一真实 capture owner 上验证。之后事件和已注册 binding 若都可见，应比较 initiative/request/context/scope/N 的原身份；事件不能开 gate。

## 2. 能力协商不能复用 V3 的布尔 true 作为完整证明

当前 V3 `initiative_admission = true` 只说明解析 InitiativeAdmitted foundation，尚无本地 gate。因此 CP05 不能仅凭它放开 live。

建议新增精确支持级别 `initiative_capture_gate_version: u32`，Open/Ready 与 RegisterOutputReceipt 都报告 1；旧 peer 缺省 0。Server 需要原 voice peer 协商为 1，且原 Register ACK 为 1、完整回显 initiative binding，才可发送媒体。普通输出 None 无此新增要求。更新端仍拒绝未协商的 initiative；不能悄悄降级成普通 binding。Root 可统一确定字段编号/命名，关键是能区分当前只支持 foundation 的 peer。

## 3. 原子接纳用现有哪把锁

线性化锁是 `PlaybackSession.state: Mutex<Option<ActiveSession>>`，通过现有 `self.lock()` 进入。不能只读 CaptureControlBarrier 自己的锁然后解锁再 Register。

Register 在该锁内、`control_generation` / `begin_scoped_playback` 之前检查：

1. binding.scope 是当前本地活跃、非 retired scope，原 binding auth 仍持有外层读准入；scope boot/session 不可由新 RPC 激活。
2. `active.capture` 必须存在、真实、未 retired；从其 `watermarks()` 得 `(through, speech)`。
3. `through` 已存在且 >= N，`speech` 不存在或 <= N；未知 capture 直接拒绝。
4. 检查通过才分配原 native owner、创建 control、initialize_capture，并一次性 arm initiative protection。相同 Register 重试只返回原 registration，不重新初始化/续期/开 gate。

建议内核接口：

```rust
// 在现有 control.state mutex 内调用；N 来自原 binding。
arm_initiative_capture(N, maximum_hold_ms) -> Result<()>
// 供 begin_bound 在同一 Session mutex 下核对原 capture 和 gate。
validate_initiative_start(through, latest_speech) -> Result<()>
```

首次及后续 `begin_bound` 必须复用已注册的原 Arc<PlaybackControl>，不得 `new()` 替换。Start 再核对 capture 与 control 的连续水位一致。若 Register 后已经出现 speech，原 gate 必须已 Held/Pausing；可以接受到有界队列但不能开 native write。若已经有覆盖该 speech 的新鲜合法 Resume，则允许原 gate Running；若水位变化但既无有效 gate 又无已接受的覆盖性 Resume，拒绝 Start，不把它当静音。

内部维护 `released_through`（初始 N，仅在真正 Applied 的新鲜 Resume 后前进），避免首次 Start 前 Hold→Continue 已完成时仍把原 N 当最新许可而误拒绝。它不是新的外部权限或可由 API 传入的数据。

锁序保持：binding admission read guard →（Start 还有现有 task_gate）→ Session state → 短 CaptureControlBarrier mutex → Control state / gate atomic。释放 Capture mutex 后再做 backend allocation/注册；Session state 仍持有使 capture 不可插队。现有 backend Shared mutex → Control state 顺序不变。持 Control state 不回调 Session/backend；不在这些锁内 await。Native CPAL callback/ALSA admission 继续仅读原 gate atomics，不新增 callback mutex。capture 的 enqueue 仍是非阻塞 try_send。

## 4. 本地 Hold 与 Server 正式 Hold 的接管

不要为本地 speech 伪造 `ControlCommand`、command_id、Server hold_id、detection 或 Applied receipt。也不要在任何 snapshot 时把 Running 盲目同步到 atomic，从而清掉本地 gate。

在纯 `PlaybackControlState` 加 initiative mode 与一个有界、非 wire-authority 的 provisional local gate，例如：

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

当前 ControlSnapshot Held/Pausing 验证要求 hold_id/reason/remaining_ms 都存在。provisional local gate 没有 Server hold_id，因此不能只把 phase 改 Held 而填一个虚假 hold_id。

最小可行合同是给 `ControlSnapshot` 加可选 `local_capture_hold`（watermark + 剩余预算），并让 phase 表示有效软件 gate：formal Hold 或 local Hold 存在即 Held/Pausing，Retired 优先。formal hold_id/reason/remaining 保持原 Server 含义。验证允许“完整 formal Hold”或“有效 local_capture_hold”支撑 Held；Running 要求二者都不存在；Held 仍要求 writes quiescent。两者并存时有效 deadline 是较早者。

该字段需要 Core + proto codec + receipt validation 一起更新，None 省略，旧普通输出 JSON 不变。`recorded_receipt` 和 `with_snapshot` 使用同一 control mutex生成真实当前状态，observation_sequence 仍由现有原 owner 分配。

Server `apply_control_receipt` 对 Resume 只能在最新事实 **有效 Running 且无 local gate** 时开服务端 gate。本地更晚 speech 造成“旧 Resume outcome Applied、当前 Held”时，应保存事实并维持/恢复原输出 Held 等待正式新控制，不能把这种正常竞争当泛化 fatal error 后永久 retire。同样需审阅 repository/control 的 snapshot校验与 gate 状态映射。

必须单独处理 Resume ACK 的两种结果：若命令因较新 speech 被拒绝，原正式 H 仍存在；若命令曾 Applied、随后 speech 才建立 local-only gate，则 H 已在设备上结束，不能让 Server 永远保留 H 并继续向它 Resume（会 NotHeld）。后者需在 Server 原 output gate 内原子保持关闭、结束本地旧 H 的控制意图，按正常递增 command sequence 建立新的正式 H2/Pause，绑定同一 output/scope/backend，预算取原约束与 local 剩余预算的较小值；这是同一输出的下一次 Hold，不是新 initiative/scope/Response，也不发送媒体重试。只有 H2 正式接管且来自真实最新 capture 的后续语义证据允许，才能 Resume。若正式 H 已存在则保留它，不重复 mint H2。该区分应由 receipt 的当前 hold_id + local_capture_hold 和不可变 outcome 决定，不能仅按 outcome Applied 推断设备 Running。此竞争需在 Server+Embedded 配对测试中覆盖，否则“保持 Held”会变成无法恢复的死锁。

`PlaybackService::control` 的 Pausing 等待需区分 formal hold 和仅 local gate；旧 Continue 查询不应等待命令里已经结束的 hold_id。用原 binding/control owner 等待 entered writes settle，随后重新 Observe 原 command，不能因超时发新scope/重复Resume。现有 drop不释放owner的语义保留。

## 6. Root 需要协调的文件范围

- Core：`crates/alia-core/src/output_control.rs`（可选身份、校验、digest兼容），`crates/alia-core/src/playback_control.rs`（provisional Hold、deadline、release watermark、快照）。
- Proto：`crates/proto/proto/playback.proto`、`voice_session.proto`、`crates/proto/src/output_control.rs`（可选身份、local gate事实、精确capability）。
- Server：`grpc/voice_session.rs` 与 `grpc/voice_session/initiative.rs`（新能力、固定N），`modules/delivery/service/output.rs`（OnceLock身份、Register/Retire binding、正常竞争下Hold恢复），`modules/delivery/service/output/gate.rs` 与 `modules/delivery/repository/control.rs`（只在确有映射/校验需要时修改）。
- Embedded：`module/playback_session.rs`、`playback_session/capture.rs`、`control.rs`（原子验证/arm/Start）；`module/speech/duplex.rs`（协商及事件一致性）；`grpc/playback_service.rs`（仅local Pausing等待）。
- Native：`module/cpal_audio/playback.rs` 现有 ScopeWrite gate 应可直接复用；默认不改CPAL/ALSA算法，只以配对测试证明 local gate确实截住这两个 native路径。现有worker有2ms/20ms有界检查，无需新线程。
- 构造兼容：当前 `OutputBinding {` 共18处匹配（含定义/codec等），精确rg定位后普通 literal补 None；新增 capability/snapshot fields同样机械补缺省，Root统一协调避免并发改tests。
- Companion/Context/policy不在设备补丁内；但专用 Companion Context Purpose、持久准入grant、原 SourceAuthorization复核和显式live enable仍是后续放行的前提。

## 7. 下一包需要的确定性配对测试（此轮未运行）

1. Core playback_control 内测：本地 speech立即有效Hold、普通None不自动Hold、local→正式Pause无Running间隙、首deadline不延长、Wait/Backchannel/Continue规则、freshResume清两道gate、重复旧Applied Resume在新speech后仍Held、local expiry退役不可复活。
2. Core output_control / proto output_control 内测：None旧JSON/digest字节保持；initiative字段改变会改变intent；丢失/替换可选绑定失败；local-only与dual Hold快照合法、Running带local拒绝、0水位合法。
3. `packages/alia-embedded/src/grpc/playback_service/tests/control.rs`：held network frame的本地speech N+1与Register N竞争（两种锁顺序）；Register之前拒绝且没有native allocation；Register之后立即Hold且无Cancel。Start前后顺序、freshResume先于首Start、旧Continue不能开放更新speech、capability0不产媒体。
4. `module/cpal_audio/playback/tests/control.rs`：CPAL callback与ALSA partial-write配对，注册后本地speech且不发送Server Pause时，后续native提交不增长、已入write结算后Held；freshContinue恢复原cursor，未重发已提交帧；尾音/End已接收仍可Hold/恢复；满队列不阻断local gate。
5. `module/speech/duplex/tests/initiative.rs` 与 `tests/binding_admission.rs`：InitiativeAdmitted/Fence/Register重排、同epoch晚Fence不能重置已held gate、boot/session替换拒绝、旧foundation capability true但gate version0拒绝。
6. Server `tests/output_control.rs`、`output_lifecycle.rs`、`duplex_playback.rs` 及 voice actor owned tests：原producer Register/Start/Control/Retire身份始终相同、RPC drop/ACK丢失只Observe、不新scope重发；迟到Applied Resume + 当前localHeld保留可恢复原输出；Context取消/机会期限/owner替换仍终止；production关闭门测试保留。

建议分一个合同/纯状态包和一个Server+Embedded接线/配对测试包，最后Root统一默认/无默认/macOS/ARM64验证。不要在仅第一包编译通过时启用live。
