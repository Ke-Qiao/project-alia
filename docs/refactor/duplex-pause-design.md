# 自然全双工的可恢复播放暂停契约

当前状态（2026-09-23）：Server / Proto / Embedded 的纵向接线已实现，成对契约为 `alia.voice.v2` / `alia.playback.v3`。Server 保留原 Delivery owner、规范 Context / Response 与持久控制命令；WAIT 进入明确暂停，只有独立且新鲜的恢复决定才能释放原 Hold。独立 `RetireOutput` 取得原输出的退休证据，不以 generation Fence 或新 session 的状态代替原 native 写入收尾。Easy-Turn 只部署在 Server；目标仍是 macOS Server 与 Pi 5。

Voice、生命周期与两端 playback 的定向验证已形成[本轮组件交接归档](handoffs/2026-09-23/README.md)。这些结果来自不同阶段的源码，不能拼接成最终冻结验收；首次完整整合运行发现旧 fixture 与新退休契约的冲突，修复后的同一源码完整矩阵仍由 Root 验证并另行记录。当前不宣称完整 workspace / Linux ARM 矩阵、Pi 5 真机、物理静音、Presented、声学连续性或 AEC 已通过。

历史设计基线（2026-09-20）：Core 与 Embedded 的 [Stage 1 基础机制](duplex-pause-stage1.md) 已提交为 `213d5ed`，通过 macOS 与 Linux ARM 定向验证，主分支整合为 `e6ba75b`（当时 macOS 完整 workspace 875 项通过）。以下现状表、实施顺序与建议时限保留当时依据 `5467e8b` 整理的设计背景，不代表当前尚未接线；当前 wire 与退休契约见[控制协议](duplex-control-protocol.md)。用户已确认区分临时疑似插话避让与明确暂停：前者可在新鲜语义证据确认只是附和后恢复，后者等待明确继续。其余时限是工程默认，实际声学效果仍待验证。

目标是用户开始插话时先保留原播放并抑制进一步输出，再由明确决策恢复或永久取消。恢复只继续仍由原 owner 持有的未提交媒体，不重新生成回答，不从已提交计数重建音频，不复活已终结的 Delivery。本包不实现 AEC、Companion、前端或通用硬件适配。

## 1. 当前行为与必须保留的事实

| 当前组件 | 实际行为 | 对新机制的约束 |
| --- | --- | --- |
| [`TurnController::observe`](../../crates/alia-core/src/turn.rs) | `Wait` 调用 `invalidate()` 提升 generation；`Backchannel` 不提升 generation | 现有 `Wait` 不具备可恢复状态。不能只修改 UI 或枚举说明就宣称可以恢复 |
| [`voice_session`](../../packages/alia-server/src/grpc/voice_session.rs) 的 `TurnDecision::Wait` | 取消 `response_cancel`，以新 generation 调用 `fence_output` | token 和 fence 都是不可逆取消；后来的“继续”目前走新一轮 `Respond` |
| [`speech/duplex`](../../packages/alia-embedded/src/module/speech/duplex.rs) | 要求 WAIT 的 epoch 增长，并对事件执行 scope fence | 两端必须一起升级；旧客户端不能靠忽略未知字段兼容新语义 |
| [`PlaybackSession::fence`](../../packages/alia-embedded/src/module/playback_session.rs) | 退休旧 generation，清除输入和观察，停止原播放 | Pause 不得复用 `fence`、`retire`、`stop_if_current` 或取消 token |
| [`AudioSubmission`](../../packages/alia-embedded/src/module/playback_session/submission.rs) | `OUTPUT_CLOSED` 永久禁止新 native write；终态不再开放 | Hold 必须是独立的可逆门，不能清掉这个终结位 |
| [`DeliveryService`](../../packages/alia-server/src/modules/delivery/service.rs) | 唯一 dispatch permit、实例持有的有界 supervisor、调用者丢弃后仍保留原媒体 owner | Pause/Resume 必须进入这个 owner；不能创建第二次派发或独立的“恢复服务” |
| [`PlaybackProgress`](../../crates/alia-core/src/delivery.rs) | 记录 accepted/submitted/presented 及不可逆 `ObservationEnd` | Held 不是 `ObservationEnd`。后端接纳不等于扬声器静音、已经呈现或用户听到 |

当前 [`alia.voice.v1`](../../crates/proto/proto/voice_session.proto) 只有采集、鉴权刷新和轮次证据；[`alia.playback.v2`](../../crates/proto/proto/playback.proto) 只有媒体 `Push` 和永久 `Fence`，均无暂停协商。现有 WAIT 行为在新契约启用前保持原意，已有 Cancelled/Unknown/Submitted 观察不会因升级变成可恢复。

## 2. 状态与身份

每个正在输出的 `VoiceScope { device_boot_id, session_id, generation_epoch }` 只有一个播放控制门。该门覆盖本 generation 的所有句段、尚未到达的 Start 和正在入队的媒体，不只覆盖当前 `playback_id`。否则第一句暂停后，同 generation 的下一句仍能播放。

```text
Running --Pause--> Pausing --原生在途写收尾--> Held
Held --明确 Resume + 原权限复核--> Resuming --同一 owner 释放门--> Running
Pausing/Held/Resuming --Cancel/超时/撤权/断连/关闭/故障--> Retired
任意控制 ACK 丢失 --> ControlUnknown：门不被擅自释放，原 owner 继续收尾
```

`ControlUnknown` 是服务器对控制结果的未知状态，不等于设备确认 Held，也不把媒体观察直接写成某个已证明终态。查询或重试同一命令可以获得原事实；超过截止时间走永久取消，取消仍无证据则沿用 Delivery 的 Unknown 和原 owner 清理规则。

以下身份从 Pause 到 Resume 全部不变：

- 原 `VoiceScope`、规范 `response_id`、原 `context_id`、原始用户 source ID/时间/privacy epoch，以及原 source 行已有的 `source_relationship_epoch`。Context 中实际使用的 Relationship epoch、Appraisal revision/digest 与 AffectProjection digest 同样保留，不能在 Resume 时刷新。
- 每个 `PlaybackIdentity` 的 playback UUID、句段 sequence 和格式；原 dispatch permit 及其 owner。
- 嵌入端本地 generation、`SubmissionBackend.instance_id`、实际输出格式、原生播放流身份。
- 已保留的 `RenderedBlock`、其 native frame cursor、队列顺序及重采样器历史。

新增 `hold_id: UUID` 标识一次连续暂停，`control_sequence: u64` 为同 scope 的单调控制顺序，`command_id + intent_digest` 标识不可变命令。Hold 原因至少区分 `AcousticCandidate`（疑似插话的临时避让）与 `ExplicitWait`（明确要求稍候），记入意图，不能靠后来的 Backchannel 改写原原因。它们用于幂等与防止旧 Resume 释放新 Hold，**不是新的 privacy/policy/relationship 时钟**。重复命令不得刷新时限；同 ID 不同内容必须拒绝。

Held 期间的新语音更新“决策已覆盖到哪个 capture sequence”的屏障，不重建 Context，不自动产生新 hold，也不靠不断重置定时器无限保留媒体。成功恢复后再次暂停使用新 hold ID；迟到的旧 Resume 永远不能作用于新 hold。

没有尚存的可恢复媒体或生产 owner 时，返回 `NoResumableOutput`。已经产生 `ObservationEnd::SubmissionFinished` 的句段不能重新开放；保留的后续句段可继续，但已提交句段不重播。End 只是媒体输入结束，尚有未提交尾部且观察仍开放时仍可 Held。

## 3. 语义决策与消息

新增明确的 `Pause`、`Resume`、`Cancel` 决策，不改变 Easy-Turn 的四种模型标签来冒充恢复许可。`Wait`、`Backchannel`、`Incomplete`、VAD 结束、收到静音或暂时没有新语音，单独都不能授权 Resume。已确认的产品规则允许“疑似插话经语义确认只是附和后恢复”：只有本次 `AcousticCandidate` 的原因、仍有效的 detection/capture 屏障以及这项许可全部匹配，才由新鲜 Backchannel 结果形成独立 Resume；旧结果或其他 Hold 的结果不适用。`ExplicitWait` 须等待明确继续，不受前一种附和许可影响。新问题或明确停止沿用不可逆取消和 generation 替换；明确“继续”也必须认定为恢复原回答且原权限仍有效。此项不再需要另行确认。

建议第一实施包由服务器在收到符合策略的近端语音证据后，先发 Pause，再等待语义结果。它有网络往返延迟，不宣称本地即时抑制。设备本地 VAD 提前闭门可另行接入同一状态机，但其防回声触发规则尚未验证，不能默认启用。两种路径都不能把 VAD 当用户意图。

新契约须显式版本协商。建议新 voice/playback 协议版本成对启用，Ready 声明 `software_hold` 能力，Start 绑定规范 response ID 与 scope；不支持的一端拒绝开启可恢复模式。最终包号随协议实现确定，不能将旧 WAIT 的 epoch 增长要求静默移除，也不增加 Cargo feature 开关。

独立于媒体流增加 `Playback.Control` 与只读 `Playback.ObserveControl`。下列字段构成最低限度契约，具体 Rust/Proto 类型在实施时落地：

| 消息 | 必需内容与校验 |
| --- | --- |
| Pause | exact scope、response ID、hold ID、Hold 原因、command ID、control sequence、不可变意图摘要、剩余暂停预算；携带触发的 detection ID/through_sequence（人工控制则明确其来源） |
| Resume | exact scope/response/hold、下一 control sequence、command ID/digest、已确认的 backend/stream 身份、明确恢复决策及其覆盖的 capture sequence；不能携带重放 PCM 或替代 source |
| ControlReceipt | exact 命令身份与序号、`Pausing/Held/Running/Retired/Rejected`、实际模式 `SoftwareGate/AlsaNativePause`、本地剩余时限、backend/stream 身份、各原 PlaybackIdentity 的 checkpoint、具体原因 |
| ObserveControl | exact scope/hold/command；只返回原 owner 的当前事实，不创建状态、续期或释放门 |
| 永久 Cancel | 复用 scope fence 与精确 playback fence；先关闭控制门，再按原观察取得最终事实，不依赖 Pause 已 ACK |

Pause 的首次接收先安装 scope 门，再等待原生在途写收尾。即使匹配 Start 仍在途，门也必须阻止它开始输出；未匹配已登记的规范 Response 时只能暂存有界的待确认门，不能临时造 Response 或来源。服务器在 Response prepare/首个 Start 前建立 scope 到原 Response 的登记；登记失败、预算到期或确认发现不属于该 Response，永久退休这次门及匹配 scope。门不能激活另一个 session。

Resume 在服务器重用现有 `DetectionId + CaptureWindow.revision` 的过期证据检查，并在发送前处理已到达的采集。设备在释放门的临界区再次检查：从该决定覆盖的 `through_sequence` 之后，是否已有新的 `speech_active` 采集证据。若有则拒绝旧恢复决定、继续 Held，等待新决定；不因多收到静音而拒绝。设备采集与控制需共享此屏障，不能仅在收到 RPC 时检查一次。此检查只防止过期决定，不能证明 VAD 检测到了所有近端语音。

命令队列必须独立于媒体队列。重复 Pause/Resume 返回原结果；乱序 Resume、另一个 hold、较旧 generation、已关闭 session、不同 backend 或已过期门均不能释放输出。若命令已经执行但 ACK 丢失，应返回原已执行事实；不能改报“从未执行”。只有在门的同一锁域内证明命令未被接纳，才能给出 `Rejected`。gRPC 错误、超时、缓存未命中或身份无法核实只表示未知/拒绝访问，不伪造设备拒绝证据。

## 4. 原生暂停边界

先安装可逆输出门，阻止新的媒体 native write；已进入的 write 仍由原 owner 收尾。CPAL 必须完成已经开始的完整声道帧，ALSA 必须记入 `writei` 实际成功的前缀。只有这些写收尾、原队列与渲染状态确实保留后，才返回 Held。控制 RPC 超时或调用者 Drop 都不能析构原 write/stream owner。

现有 `PlaybackBuffer::write_next` 和 `alsa_chunk` 将 `begin_output() == None` 解释为退休并丢弃块。因此不可仅让 `begin_output` 在暂停时返回 None；必须明确区分 `Writable / Held / Retired`，Held 保留 front block 和 cursor。取消优先于 Hold，永久 `OUTPUT_CLOSED` 永远不能被 Resume 清除。

| 后端 | 第一实施包的可用方式 | 不可推导的结论 |
| --- | --- | --- |
| CPAL | 软件门保留原媒体，回调继续写真实零值；不推进原媒体 submitted cursor | `StreamTrait::pause/play` 存在，但不能由 API 成功推断队列无损保留、精确暂停位置或扬声器即时静音 |
| ALSA 支持 native pause | 查询当前已配置 PCM 的 `HwParams::can_pause()`；在前缀收尾后成功 `pause(true)` 才记录 `AlsaNativePause`，以同 PCM 的 `pause(false)` 恢复 | 支持位不能替代调用结果；`snd_pcm_resume` 是挂起恢复，不是应用层取消 pause |
| ALSA 不支持 native pause | 使用明确报告的 `SoftwareGate`：保留媒体，必要时提交真实零帧维持运行；已提交尾部仍可能排出 | 不能把 `drop/prepare` 伪装成无损暂停，再重播已被接受的前缀 |

以上依据本地已锁定官方依赖源码：`cpal 0.15.3` 的 `src/traits.rs::StreamTrait`、`src/host/coreaudio/macos/mod.rs::pause`，以及 `alsa 0.9.1` 的 `src/pcm.rs::{pause,resume,HwParams::can_pause}`。CPAL CoreAudio 实现调用 AudioUnit stop，没有暴露本文所需的呈现游标或保留队列保证。若原生 pause 调用失败且实际状态不能重新证明，退休为失败/未知，不通过猜测切换模式继续。

暂停不是媒体音量归零：后者仍会消费原样本，无法恢复。软件门期间原媒体 source/native 提交游标冻结，输出设备的零帧和参考流 native cursor 可以继续增长；它们是不同计数。已经排入硬件的尾音不在软件门可撤销范围，Held 仅证明进一步媒体提交被抑制。需要声学静音保证时必须另行测量。

复用[实际播放参考流与独立音频时钟](audio-reference-clock-design.md)，记录真实零帧和控制边界；有意 Held 的零值不能伪装成普通欠载，也不能伪造缺失的参考。原生 pause 期间没有实际写入就不合成 PCM 或假 cursor。Resume 的过滤历史只能在流和连续性仍可证明时保留；设备重开、无法证明队列保留的 xrun/suspend 恢复或 native instance 改变，均退休这次可恢复播放。采集时钟与输出时钟始终独立，普通软件 Hold 不创建假的共同采样域。

原生锁顺序沿用 `binding 授权读锁 → PlaybackSession gate → backend 控制 gate`；临界区只安装状态和取得原 owner，不做网络、数据库、native 等待或模型工作。等待收尾后必须按同一身份重查取消/绑定，再发布回执。回调只读取预先安装的轻量门，不执行语义推理或阻塞式控制。既有采集转换器仍有分配/锁，不由本文宣称整个采集路径已硬实时化。

## 5. 媒体背压、时限和生命周期

暂停必须保留原 decoder、resampler、句段顺序和未提交块，不能丢尾、跳块或重新 TTS 后称为继续。现有 Pipeline 句子队列 4、SentenceAudio 队列 2、嵌入 adapter 的媒体通道 8 和 native 有界队列可以复用，但条数上限并不约束单个 WAV/TTS payload 的大小。

实施时统一计算该 Response 被保留的编码 payload、解码 PCM、渲染块及 in-flight 结果字节，并保留已有最多 120 秒源音频限制。首包建议硬上限：单个编码 payload 1 MiB、服务器每 Response 留存 8 MiB 且全实例 256 MiB、嵌入端每 scope 留存 4 MiB。预算在读取/解码/分配前预留，超额报错，不能先接收一个无界 `Vec` 再检查；共享块只按实际分配计数，复制块分别计数。测试按实际格式与采样率证明这些上限；后续调值不能移除上限。Held 后不再启动新 TTS 请求；已启动请求只能写入预留的有界槽，LLM/TTS/网络流通过已有 owned worker 背压。无法在上限内完整保留或 provider 中止流时，永久取消，不能从丢失处自动续生成。控制、鉴权与取消不能等待这些媒体槽。

机械时限如下。Stage 1 提交的本地验证硬上限仍为 15 秒；下一协议包将工程上限调整到 120 秒、Server 默认保留 60 秒并允许配置，始终受原输出总截止时间约束。已通过异步问题征求用户时长偏好，尚未收到回复，60 秒按明确告知的默认值继续实施；不要将其记成用户已确认的时长：

| 时间范围 | 建议约束 |
| --- | --- |
| 原生 Pausing 收尾 | 复用现有精确取消的 1.5 秒观察预算；到期不假 ACK，原 native owner 继续受监督收尾 |
| 设备保留媒体的最长 Hold | 下一包首次 Pause 默认 60 秒，最大 120 秒，且不超过原输出剩余期限；后续命令均不延长 |
| 等待语义的临时避让 | Server 另设最多 3 秒的语义判定期限；到期永久取消，不以沉默自动恢复 |
| 明确要求稍候 | 初始 ExplicitWait，或同一 hold 的 ConfirmExplicitWait；后者只撤去临时语义期限，不改变设备最初的保留截止时间，且只能明确继续 |
| 派发观察 | 保留现有 120 秒绝对 deadline，暂停和重复命令均不重置 |
| DB owner lease | 保留现有 15 秒租约及 heartbeat；租约续期不延长语义 Hold deadline |

Stage 1 已支持从 AcousticCandidate 到 ExplicitWait 的显式 `ConfirmExplicitWait`：绑定同一 hold、命令身份、下一序号和新鲜 capture 证据，只改变原因一次，不释放门、不续期。因此 Server 不能先给设备 3 秒预算，再将“确认明确暂停”当作延长保留期限；首次 Pause 应授予本次固定设备预算，并独立管理更短的临时语义期限。设备从首次接收建立本地单调时限，服务器用 DB 时间持久化自身截止时间。跨机器不直接比较单调时间或将 DB 墙钟当设备时钟；重试保持原命令的 `budget_ms`，剩余时间仅作为外层有效性限制，不通过改变命令续期。数据库暂停到期、设备本地到期、总观察期限任一先到，都取消。

当前 `playback_service` 在输入开放时有 5 秒 input/EOF idle timeout、120 秒观察上限、约 1 秒 progress heartbeat 和 250 毫秒鉴权检查。合法 Held 必须显式暂停“等待新媒体”的 idle 计时，或在原截止前转入专门 Held 分支；仅停止轮询 Push 会被当前实现取消。progress、Control、鉴权刷新和采集继续运行；Resume 后恢复媒体 idle 检查，总 deadline 不变。`Push` 断开、读写超时、绑定撤销、会话替换、服务关闭都终结可恢复资格。

Held 不能由 `VoiceTurnRegistry` 中 LLM 任务的生命周期单独持有：该任务结束后，原生播放尾部仍可能存活。控制入口必须关联现有 Delivery supervisor 和 embedded PlaybackSession 的 owned task；它们继续拥有 stream、生产任务、取消任务及 join。Shutdown 先关闭 admission，再永久关闭所有 Hold，等待原任务；无法按时 join 明确报告 incomplete，不能启动替代 owner。

## 6. Delivery、权限与持久化

在现有 Delivery repository/service 内增加暂停控制账本，保留 Sending/Observing 等媒体状态，独立保存控制状态。建议一份 `voice_output_controls` 当前行（唯一 scope，绑定原 response、owner instance、hold、控制水位及首次/最长截止时间），以及有界 `delivery_control_commands` 命令记录（唯一 command ID、scope+sequence、intent digest、动作、请求/回执）。复用 `delivery_events` 保存可查询审计，不另建 PCM outbox、Response 副本、privacy epoch 或派发服务。

首包每个 Response 最多接纳 32 个可逆控制命令，重复 ID 不占新名额；达到上限退休这次可恢复控制，永久取消不受该上限阻挡。不能淘汰旧身份后重新接纳它。结束后保留至少控制水位/退休事实以拒绝迟到请求，生命周期归属于原 Response。数据库记录只证明请求/回执持久化，不证明设备执行。

Pause 请求先关闭本实例媒体 admission，持久化不可变控制意图，再由原 supervisor 发送。Resume 则必须先在短事务验证原 Context 和原输出来源，再 CAS 为 `Resuming`，然后发往仍持有原媒体的 owner；设备成功释放后的事实再记为 Running。ACK 丢失不生成新命令、新 playback UUID 或新 dispatch permit。原 owner 不存在时，恢复流程只退休/记录 Unknown，不派发音频。

沿用已整合 Appraisal 的数据库锁序：`policy → active user → privacy state → relationship authority → 原 Context 的规范根 → Response → control row → 按 ID 排序的 Delivery rows`。具体复用 `ContextService::validate_in_transaction` 与 `source::lock_relationship_epoch` 的现有顺序，不能拿了 Appraisal/投影行锁后再倒取 relationship authority。Resume 逐项核对原 source、channel、scope、response/context ID、cancel 标记、lease 和绝对 deadline。事务内不做 RPC、TTS 或等待 native。

Context 的 Memory/历史/Cache/Execution，以及已进入读集的 `Relationship { epoch }`、Appraisal revision/result digest、AffectProjection content digest，均按原规范证据在同一事务验证。Relationship clear 后依赖旧关系的输出不能恢复；没有实际关系依赖的基线 Context 不凭空添加投影根。原消息的 `source_relationship_epoch` 只沿用真实接纳时的值，不能拿新“继续”消息更新它，不能据此重新派生已经失效的 Appraisal。任何实际根撤权都不能靠换 privacy/relationship epoch、刷新 lease 或重签旧内容绕过。

永久取消不要求旧 Context 仍有效；撤权恰恰需要能停止原输出，不能因权限复核失败阻塞 cleanup。Resume 的 DB 授权与设备物理动作不是分布式原子事务：事务后到达的撤权仍由现有取消/鉴权/fence 传播停止，已经进入 native write 的前缀仍记录在原身份下。本文不承诺撤权提交瞬间已提交声波消失。

暂停/恢复不写 assistant 历史，不重复 commit Response。规范 Response 内容与历史仍沿用原子一次提交；暂停期间确有未结束内容时保持未结束，最终永久取消按现有 interrupted/discarded 规则处理。已提交规范内容但仍有音频尾部时，只控制那份原 Delivery，不能为恢复再次生成历史。Body 动作和 Execution 工具副作用不会因声音暂停而自动回滚或重放；若产品需要身体同时停顿，必须明确独立的 Body 控制契约。

## 7. 必须修改的接口与可复用部分

以下是完整接入范围；Core/Embedded 的本地门已在 Stage 1 工作树实现，其余网络、Server 与生产背压仍待实施，不能据本表宣称端到端恢复已经启用：

| 文件/模块 | 最小必要变化与复用点 |
| --- | --- |
| [`core/turn.rs`](../../crates/alia-core/src/turn.rs)、[`core/delivery.rs`](../../crates/alia-core/src/delivery.rs) | 保留旧决策兼容边界，增加独立控制状态/命令/回执校验；复用 VoiceScope、PlaybackIdentity、单调 checkpoint、不可逆 ObservationEnd |
| [`voice_session.proto`](../../crates/proto/proto/voice_session.proto)、[`playback.proto`](../../crates/proto/proto/playback.proto) | 新契约协商、明确恢复证据、Response 绑定、独立 Control/ObserveControl；媒体 receipt 与控制 receipt 分离 |
| [`server/grpc/voice_session.rs`](../../packages/alia-server/src/grpc/voice_session.rs) | 当前检测结果过期屏障复用；Pause 不取消 response token；语义恢复必须显式决定，终止仍 fence |
| [`channels/adapter.rs`](../../packages/alia-server/src/channels/adapter.rs)、[`channels/embedded/playback.rs`](../../packages/alia-server/src/channels/embedded/playback.rs) | 控制能力与原实例命令入口；复用 exact Fence、owned producer/receipt observer，不 Drop Push 代替 Pause |
| [`pipeline/turn.rs`](../../packages/alia-server/src/channels/pipeline/turn.rs)、[`pipeline/delivery.rs`](../../packages/alia-server/src/channels/pipeline/delivery.rs)、[`pipeline/streaming.rs`](../../packages/alia-server/src/channels/pipeline/streaming.rs) | 原 Response/scope 到 Delivery owner 的登记、独立生产门及字节预算；prepare/frozen Context 和有界句段队列复用 |
| [`modules/delivery`](../../packages/alia-server/src/modules/delivery)、新增迁移与迁移登记 | 控制 CAS、原权限 Resume gate、原 supervisor 命令/回执持久化；复用唯一 permit、heartbeat、Unknown 不重放和原子历史提交 |
| [`embedded/speech/duplex.rs`](../../packages/alia-embedded/src/module/speech/duplex.rs)、[`grpc/playback_service.rs`](../../packages/alia-embedded/src/grpc/playback_service.rs) | 新协议校验、capture/Resume 屏障、独立控制和 Held idle 分支；复用绑定检查、owned Push、在途写 join |
| [`playback_session.rs`](../../packages/alia-embedded/src/module/playback_session.rs)、[`submission.rs`](../../packages/alia-embedded/src/module/playback_session/submission.rs)、[`traits.rs`](../../packages/alia-embedded/src/module/traits.rs) | scope 可逆门及能力/事实接口，Held 与 Retired 分离；原 scope fence 和不可逆取消位保留 |
| [`cpal_audio.rs`](../../packages/alia-embedded/src/module/cpal_audio.rs)、[`cpal_audio/playback.rs`](../../packages/alia-embedded/src/module/cpal_audio/playback.rs)、[`cpal_audio/alsa_backend.rs`](../../packages/alia-embedded/src/module/cpal_audio/alsa_backend.rs) | 保留 renderer/buffer/native owner，CPAL 软件零值及 ALSA 实测能力分支，部分写后 ACK；native控制不进模型/回调等待 |
| [`audio_clock.rs`](../../crates/alia-core/src/audio_clock.rs)、[`cpal_audio/clock.rs`](../../packages/alia-embedded/src/module/cpal_audio/clock.rs)、[`context/audio_reference.rs`](../../packages/alia-embedded/src/module/context/audio_reference.rs) | 记录有意暂停/释放与实际零值，复用原生 cursor、独立时钟和最终 loss snapshot；不新增呈现断言 |

实施顺序：先完成纯状态/幂等和 embedded 可逆门；再接媒体保留、两类后端及 owned 生命周期；随后接协议和 Delivery CAS；最后在经确认的语义策略下启用实际触发。前几步可由显式测试命令验证，不依赖先猜测“嗯/等一下/继续”的产品解释。

最先可独立实施的文件边界是新增 `crates/alia-core/src/playback_control.rs` 与 `lib.rs` 导出，以及 embedded 的 `module/playback_session.rs`、新增 `playback_session/control.rs`、`playback_session/submission.rs`、`module/traits.rs`、`cpal_audio/playback.rs` 和必要的 `cpal_audio.rs` 软件门装配。纯类型负责 Hold 原因、身份、控制顺序和终态规则；embedded 包实现可调用的 hold/resume/observe 方法、scope 门、在途写计数与保留渲染块，并通过现有生产渲染入口验证。首包不编辑 `turn.rs` 的 WAIT 策略，不开放 wire 命令，不改服务器/数据库/自动语义，不做 native pause 能力宣称；ALSA 真实 pause 及协议持久化按后续步骤接入。没有协议来源的生产自动恢复不会因此启用。

## 8. 最小验收与不得替代的硬件测量

这些是后续实施必须运行的测试，本次只完成文档和源码核对，没有新增执行结果。

| 层级 | 最低验收 |
| --- | --- |
| 纯状态/协议模拟器 | Pause 不提升 generation；没有匹配 Hold 原因、新鲜证据和已确认许可的 Backchannel/Incomplete/静音/超时不放行；新鲜附和只可恢复 AcousticCandidate，ExplicitWait 只可明确继续；重复、乱序、旧 hold、epoch 溢出、Resume 与新 speech 竞争；旧 WAIT 仍永久取消，新协议不能误连旧语义 |
| 实际 PG，随机 schema、两事务屏障 | Resume 与 Memory clear、relationship clear、policy 变更、Appraisal/其他根版本变化及取消竞争；失去 ACK 后同命令幂等；暂停阻挡同 generation 新句段 admission；过期/重启不拿新 permit、不重放；不产生第二份 Response/history；时限不随 heartbeat 或重试延长 |
| CPAL 生产渲染入口与受控 native owner | 在完整声道帧中途暂停，已进入帧收尾；参考是最终增益/重采样/量化后的实际零值；保留块与 resampler，恢复不丢、不重、游标不回退；控制 waiter Drop 后原 write owner 存在；取消优先且不能解开终结位 |
| Linux 真实 libasound 软件 PCM | 调用实际 `can_pause/pause`，记录设备返回；支持时验证同 PCM 原生暂停/释放，不支持时验证声明的软件门路径。真实 write 接纳的前缀与明确注入的部分写/错误用例区分；ALSA start/wait/pause 失败、暂停中关闭都有原 owner 收尾，无假 Held |
| 两端 gRPC loopback | 媒体槽满、Start/入队阻塞时 Control 仍可到达；Held 下采集、进度、鉴权不断；input idle 分支与总 deadline 区分；断连/解绑/服务停机不可恢复；丢失 Control ACK 后查询原事实，旧 Resume 不能打开后来的门 |
| 有界压力与生命周期 | 多句段/大 payload 达字节上限明确失败，未启动 TTS 被背压；hold/命令/观察数均有界；重复暂停不无限续期；无后续 PCM 的关闭仍能观察最终参考丢失，全部后台任务可 join 或明确 incomplete |

软件 `null` PCM 不具有真实声卡节奏，也不一定支持 native pause。不得为获得绿色结果把“unsupported”改报“native pause 已验证”；应记录该分支未验证，并验证软件 fallback。模拟器和错误注入可以验证契约竞争，不能替代实际声卡。

仍须在目标 macOS 音频设备与树莓派 5 的麦克风/声卡上测量：近端插话到实际尾音停止的延迟、驱动保留队列行为、恢复爆音/缺失/重复、采集与播放漂移、回声误触发、真实双讲和 AEC 重收敛。当前 Easy-Turn 仍部署服务器；本文不声称 AEC、物理 Presented、用户听觉无缝恢复或树莓派验收已完成。

## 9. 已确认决定与剩余产品问题

已确认：区分疑似插话的临时避让与明确 Wait。临时避让可在匹配本次原因、fresh detection 和 capture 屏障的附和确认后恢复；明确 Wait 必须等待明确继续。原始 Backchannel 标签不是通用恢复许可，旧 WAIT 的永久取消不能复活。以下问题不重复征询这项决定：

1. **首次抑制由什么证据触发？** 建议首版由服务器收到近端语音证据后触发；是否接受额外网络延迟，还是需要设备 VAD 先行闭门，并为回声误触发另定门槛？当前没有 AEC 实测可支持后者默认可靠。
2. **暂停能保留多久，到期如何解释？** 先用有限、可配置的工程默认：临时 3 秒、明确等待默认 60 秒、可配置且不超过 120 秒，并受原输出剩余寿命约束，超时取消；是否需要更长等待或重新询问可后续调研。重新询问是一轮有新来源的输出，不能偷换成旧音频恢复。
3. **“暂停”要求软件停止继续提交，还是可感知的即时静音？** 前者仍有已提交的设备尾音；后者需要真机能力与延迟验收，不能只靠 CPAL/ALSA API 定义满足。
4. **身体动作是否同步暂停？** 本文只控制声音及其尚未生成的媒体生产，不重放工具、不撤销已发生副作用。若需要身体协同，应先明确姿态保持、停止与恢复的独立规则。

剩余问题不妨碍先构建身份、暂停门、保留媒体、权限复核、幂等控制与验收基础。后续实际启用按已确认的两类恢复规则推进；不得把其他尚未确认的建议扩成额外自动恢复许可。
