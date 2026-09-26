# Voice / Companion foundation handoff — 2026-09-26

Root 收口补记：V3 已完成，workspace 1005/0/5、5 项 ignored 单独通过、两平台 no-default 检查通过；详见 [统一摘要](validation-summary.json)。下文“运行中”保留为组件交接当时状态。

本包交付 snapshot-only voice initiative foundation。production reserve 仍明确拒绝；CP05 live 主动语音未完成。Root 的 macOS V3 workspace 验证在本次 handoff 写入时仍运行、尚未验收，本文不声称全量测试通过。此 agent 当前没有 Cargo、PG、模型服务或运行中的验证容器，也未进行 Git 提交。

## 已接入的只读机会与限制

`VoiceSessionService::with_initiatives` 向原活跃 actor 注册 bounded mailbox；`VoiceInitiativeHandle` 不依赖 Embedded wire 类型，可在 Server no-default 构建存在。外部 snapshot 不占用 epoch、不创建 OutputProducer、不生成 Response、不输出媒体。

机会绑定原 owner/channel/session/boot，来自原 AdmittedSource 的持续认证，不等于摄像头 presence。actor 要求真实且最近接纳的 capture（不超过 2 秒）、没有 active utterance、Busy/retry/pending detection、没有任何 alive 原输出（Held 也算 alive）；认证/DB await 后重新 drain/recheck。机会 TTL 5 秒。`observed_at` 是观测时间，`last_capture_at` 是真实接纳 DeviceFrame 的时间，不能以 snapshot 时间代替后者。当前不满足条件时返回 unavailable/error；尚未提供完整 busy/held diagnostics 事实分类。

`CaptureWindow` 新增只读 idle / last-received 信息，不改变普通 DU11 turn 判断。session transport idle deadline 固定基于最后真实 DeviceFrame；连续 snapshot mailbox 不能续期。合法 RefreshAuth 本身是收到的 DeviceFrame，因此按原语义延长 transport idle，但不能伪造 capture freshness。

新增 Open/Ready `initiative_admission` 和独立 `InitiativeAdmitted` payload；Embedded 拒绝未协商、nil/变更身份、旧 boot/session、非恰好下一代 epoch、未发送或回退 watermark、重复 request/initiative。普通 TurnEvidence/Complete 路径保留。

## reserve 基础不等于 live 可用

`OutputRuntime.initiative_output_enabled` 默认 false，当前没有生产 builder 打开它；actor reserve 最前就 fail closed，cancel 原请求，不读取 Context、分配 scope 或推进 epoch。Root 当前仅把 snapshot 用于 shadow。

底层 reserve foundation 保留了原 Context/Source 检查、bounded queue、超时/drop 取消、原 request/initiative 去重、原 Arc<OutputProducer>、scope、cancel、固定且仅能缩短的 deadline。Producer 可固定 context_id 并拒绝换 Context 的 Response。这些内部路径尚未构成可启用的 CP05 完整合同：

- 专用 CompanionOutput Context Purpose、持久 policy/candidate admission 及完整 live source gate 仍需补齐；当前通用 Context 验证不能作为主动输出目的证明。
- 设备本地 speech 可能先发生、对应 frame 尚在网络中；Server drain 无法看到它。当前新 epoch/control 会重新 Running，设备只记 capture watermark，没有自动 local Hold。此竞争在 reserve 禁用的 shadow 阶段不会触发，但禁止据此打开 live。
- 当前 initiative capability bool 只证明 foundation，不证明未来 local gate 合同；后续须显式区分版本或能力，旧 peer 不允许降级主动输出。

## 新增 12 项 foundation 测试与证据边界

以下为新增的 12 个唯一测试，不把重复运行计入数量。此次 Linux 工作只做 all-targets 编译，没有执行这些测试；其执行结果由 Root 的统一验证收口，本文不将“已编译”写成“已通过测试”。

Core capture（1）：

- `idle_snapshot_requires_semantic_completion_and_preserves_frame_time`

Embedded InitiativeAdmitted（2）：

- `initiative_requires_negotiation_and_real_capture_watermark`
- `initiative_rejects_identity_epoch_and_watermark_changes_without_fencing`

Handle/drop/timeout（3）：

- `dropping_reservation_waiter_cancels_original_request`
- `replacement_keeps_new_mailbox_and_rejects_old_scope`
- `reservation_timeout_cancels_original_without_enqueuing_a_retry`

Voice actor（6）：

- `initiative_snapshot_requires_capture_and_never_allocates_output`
- `initiative_snapshot_rechecks_speech_arriving_during_authentication`
- `initiative_snapshot_rejects_original_output_even_when_held`
- `initiative_snapshot_polling_cannot_extend_capture_idle_deadline`
- `authenticated_refresh_frame_extends_transport_idle_but_not_capture_freshness`
- `initiative_closed_output_gate_cannot_advance_scope_or_read_context`

最后三项使用 transport-only/disconnected DB fixture；暂停 Tokio 时钟的测试不启动 Delivery/PG recovery，不把虚拟时间套在真实 SQL 后台任务上。正常 actor source/auth 测试仍由 Root 独占 PG 排程。本包没有正向完整 live reserve、真实主动声学播放、设备在说话竞争修复或新 TTS 行为通过的声明。

## Linux ARM64 V2 / V3 编译记录

根目录为 `run/validation-20260926/`，所有本轮 source/evidence/cache 都独立保存。使用保留的 Rust 1.95 ARM64 image：

`rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1`

Rust host 为 `aarch64-unknown-linux-gnu`，rustc `1.95.0 (59807616e 2026-04-14)`。在任务容器内安装真实 Debian protobuf 3.21.12、ALSA 1.2.8、D-Bus 1.14.10 开发依赖，约 4 MB 下载；不使用 DOCS_RS 或假 native 依赖，不安装 PG client，不启动或使用 PG。

两个 package 分开顺序执行：

```text
cargo check -p alia-server --no-default-features --all-targets --locked --offline -j2
cargo check -p alia-embedded --no-default-features --all-targets --locked --offline -j2
```

V2：619 files，manifest SHA256 `eaa32696907d0653908b136c9919a22dd03f41406c044d01d52532ad677b2235`。

- Server exit 101：`companion/service.rs` 从 `tokio_util::task` 根模块导入不存在的 `TaskTrackerToken`，E0432，与 Root macOS 同一错误。
- Embedded exit 0。
- host/container 源码前后 hash 全通过，Cargo.lock 未改变。失败原始日志与 source 副本保留，未覆盖为成功结果。
- 证据：`linux-arm64-v2/evidence/result-summary.json`、两个 `*-no-default-all-targets.log`、`source-*-host.json`、`source-*-container.log`。

V3：619 files，manifest SHA256 `8ea6d4f7cc63b82d22b8c584c8c0b426b11e04f952f64bd15b7776d352cc18a1`。相对 V2 只有 `packages/alia-server/src/modules/companion/service.rs` 的修复；差异见 `linux-arm64-v3/evidence/source-difference-from-v2.json`。

- Server exit 0，约 19.03 秒，无编译 warning。
- Embedded exit 0，约 2.32 秒，无编译 warning。
- 619 文件 host/container 编译前后均与 manifest 完全一致；Cargo.lock 前后相同，SHA256 `845d709734b382d24baaec28bd9d69a524e92c4b41366184de02f0ef7a322dfc`。
- 汇总：`linux-arm64-v3/evidence/verification-summary.json` 和 `result-summary.json`；完整 command/log/exit/source hash 同目录。
- 复用 V2 任务容器真实 native 环境创建独立 V3 任务容器，使用 APFS clone 的独立缓存，无额外下载。两个本任务容器均已 stop exit 0；保留 container/image/cache/source/evidence。未改动用户容器、历史 worktree 或旧 9/23 cache。

这证明 frozen V3 在 Linux ARM64 真实依赖环境下能编译 Server/Embedded no-default 全 targets；不证明 Pi5 实机运行、物理麦克风/VAD/AEC、扬声器呈现或 Linux 测试执行。

## 下一包 CP05 设备合同草案

详细只读设计：`run/validation-20260926/cp05-device-contract.md`。尚未实施。

最小路线是在 OutputBinding 中增加可选 `initiative_capture`，固定 initiative/request/原 capture watermark，与原 scope/context 一起贯穿 Register/Start/Control/Retire。普通 None 保留原 JSON/digest 行为。实际线性化锁为现有 `PlaybackSession.state`；在该锁内核对真实 CaptureControlBarrier 的连续 watermark 后才分配 native owner；capture 接纳与 Start 也在同一锁序下检查，不仅在 SessionEvent 上做网络水位判断。

Register 前的新 speech 可拒绝本次机会，不产媒体；Register 后 speech 应立即复用原 Arc<PlaybackControl> 的软件 gate Hold，保留队列/cursor，然后让原 Server Pause/ConfirmWait/Continue 接管。不能默认永久 Cancel 主动输出来绕过自然双工。较旧 Continue/Observe/重复 Applied receipt 不得打开更晚本地 speech 的 gate；有效 Resume 需覆盖当前真实 speech，明确 Wait 只接受 ExplicitContinue，deadline 只能缩短，恢复不能重发已提交样本。

需单独表示 provisional local Hold，不能伪造 Server hold_id/command/detection；ControlSnapshot/codec/校验需要相应真实事实。Root 须协调 Core+Proto、Server OutputProducer/receipt reconciliation、Embedded PlaybackSession/gRPC 和原生 CPAL/ALSA 配对测试的共享文件。建议合同/纯状态与接线/配对测试分包，所有 live 前提满足并统一验证后才打开生产准入门。
