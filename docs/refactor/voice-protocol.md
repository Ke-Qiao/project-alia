# 持续语音 v2 与播放 v3

本文件描述整合候选已接入的协议与控制边界，整包及平台验收结果以[整合检查表](checklists/duplex-integration.md)为准。Easy-Turn 固定运行在 Server；设备侧保留采集、VAD、播放和绑定失效处理。当前可恢复暂停使用软件门控，实际声学停止、播放时钟、AEC 与 Pi 真机双讲仍需独立验收。

## 成对升级与持续采集

`alia.voice.v2.VoiceSession/Stream` 是设备到 Server 的双向 gRPC 流，必须与 `alia.playback.v3.Playback` 成对部署。第一帧 `OpenSession` 携带 `protocol_version = 2`、设备启动 UUID、渠道标识、`software_hold = true` 和 `max_hold_ms`（1..120000）。Server 验证凭证和绑定，分配新的会话 UUID，返回带相同版本、软件暂停能力与协商上限的 `Ready`。设备在原绑定仍有效时校验 Ready 并激活本地会话，随后才上传采集帧。

Server 与 Embedded 应从同一套协议源码成对更新。旧 voice v1、playback v1/v2、缺少软件暂停能力或非法上限的连接被拒绝；没有把旧版 WAIT 自动解释成新版 Pause 的兼容降级。新 Ready 不能使已退休的原输出重新生效，断线重连或换绑应取得新的会话 UUID。

当前输入为 16 kHz、单声道、PCM16 little-endian；每帧最多 1,600 个采样。`sequence` 从 0 连续递增，`sample_offset` 是该流内的连续采样偏移。凭证刷新帧不消耗音频序号。采集溢出、乱序、格式错误或流中断会结束会话，重新唤醒后不能把旧缓冲接到新会话。

持续上传会覆盖正在生成或播放回答的时段。VAD 只提供近端活动证据；Server 接受每个 speech frame 时，先同步关闭原输出的生产门并更新采集屏障，不等待控制 SQL 或设备 RPC。设备的暂停确认仍通过独立 Playback 控制路径取得，这不承诺本地 VAD 检出瞬间已经物理静音。

## 两类暂停与语义请求

Server 以有界音频窗口调用 TurnDetector。模型结果只有在 detection/request、窗口版本和原来源仍有效时才能用于决策；凭证重验等待期间新到的 speech 也会使旧 Continue 失效。

| 输入或观测 | 当前策略 |
| --- | --- |
| 接受新的近端 speech | 对仍存活的原输出建立 `AcousticCandidate` 临时 Hold，等待新鲜语义证据 |
| `incomplete` | 保留输入窗口继续听，不生成回答，也不授权恢复 |
| `backchannel` | 仅对临时 Hold 请求恢复；不会解除明确的 `ExplicitWait` |
| `wait` | 将原 Hold 确认为 `ExplicitWait`，保留原回答、cancel token 和 generation；重复 WAIT 不续期 |
| `complete`，完整短语为“继续”“请继续”“continue”等 | 请求恢复仍存活的原输出；没有原输出时不重建或重播回答 |
| `complete`，完整短语为“停止”“别说了”“stop”等 | 不可逆取消原输出并推进 generation，后续 Continue 不能复活它 |
| 其它非空 `complete` | 作为新问题，取消旧回答、推进 generation、建立新栅栏，再入队新的语音请求 |

Continue/Stop 使用完整短语匹配，不使用子串判断。例如“继续讲这个问题”是新问题。`TurnEvidence.output_decision` 的 `PauseRequested` / `ResumeRequested` 只说明语义请求已接纳，不是设备 `Applied`、`Held` 或播放完成回执；`response_enqueued` 只说明进入有界处理入口。

每个回答保留原 request、Response、Context、boot/session/generation、来源 privacy/relationship epoch、Channel adapter、Server instance 与后端身份。暂停保留同一个 producer、在途生成工作和媒体所有权；恢复后继续这条原输出，不重跑 LLM/TTS，不换 Context，不从头重放 PCM。Resume 必须通过原来源重验、采集新鲜度和原控制回执校验；未知提交或 ACK 只能观察原命令，不能放行生产门。

## 工程时间预算

Server 可在策略片段中配置：

```toml
[policies.duplex]
hold_budget_ms = 60000
```

配置只接受整数 1..120000，默认 60000ms。当前 VoiceSession 使用配置与设备协商上限的较小值，控制账本还要求预算不超过原设备注册上限。以下都是工程默认或硬边界，不是用户确认的理想等待时长，也不是声学效果指标：

| 期限 | 当前边界 |
| --- | --- |
| 临时 `AcousticCandidate` 的语义等待 | 自首次建立临时 Hold 起最多 3s；期间须得到可接受的新鲜语义证据，不能因继续采集或模型重试而续期 |
| 明确 `ExplicitWait` 的保留 | 使用首次 Hold 的原预算，默认 60s、最大 120s；从临时暂停确认 WAIT 也不重新起计时 |
| 原输出总寿命 | 从原 owner 建立起最多 120s，原音频观察期限还可能进一步缩短；Resume 不重置这个期限 |

任一期限更早到达，或原 owner、来源、绑定、后端失效，都会关闭原输出；超时不会自动恢复或生成另一份回答。模型 HTTP 超时与这三种输出期限独立，调大模型请求超时不能延长暂停或恢复许可。详细控制约束见[暂停设计](duplex-pause-design.md)与[控制协议](duplex-control-protocol.md)。

## 凭证和会话退出

设备每 120 秒签发新 token 并发送 `RefreshAuth`。Server 验证签名、设备、scope、渠道和当前绑定，成功后才替换凭证；生产会话每 30 秒周期复核，300 秒签发新鲜度约束保留。会话的来源双 epoch 与 adapter 在最初接纳时固定，刷新凭证不能刷新旧来源授权。解绑、隐私或关系 epoch 改变、adapter 被替换会关闭原会话及其排队回答。

设备立即及每 250ms 检查本地绑定快照；Ready/Evidence 接纳同时检查原绑定，周期轮询不能替代这个边界。解绑、重新加载或换绑后，迟到的旧 Ready/Evidence 不能激活输出。设备网络任务退出、取消或出错时，由所属 guard 关闭原采集会话；旧 guard 不得关闭替换后的新 owner。

VoiceSession 只保留控制句柄。最后一个实际 producer/media owner 释放或输出期限到达时，原输出结束，不能因为语音会话仍在监听而无限保留。新问题、Stop、会话关闭和服务关闭均需退休原输出。

## 播放注册、媒体与退休

`alia.playback.v3.Playback/Push` 接收 `Start → Audio* → End → EOF` 并返回后端观察。Server 在 TTS/Start 前准备规范 Response/Context，并以完整 `OutputBinding` 调用 `RegisterOutput`；后续 Start 的 binding 必须精确一致。`contract_version = 3` 的首个 Accepted 回执必须有正数 `receipt_sequence`，且尚未接收 Audio、End 或终态事实，然后才能发送第一帧 Audio。独立 Control/Observe 与媒体观察共享原播放的回执序列，因此首个 Push ACK 可以是 2 或更大；后续回执仍须严格递增。媒体 `playback_sequence` 的分配与这个观察序号分开。

同一次播放的每个媒体帧携带相同的 `VoiceScope`、播放 UUID 和 `playback_sequence`；Server 分配的播放身份序号首项为 1，后续播放递增。单次播放内的 Audio 序号从 0 连续递增，End 声明总采样数；End/EOF 只关闭输入，未提交的尾部与观察所有权仍然存在。同一 generation 的后续片段按 FIFO 追加；暂停保留原 FIFO 与编码预算，并阻止新增生成/媒体提交。控制通道独立于媒体背压。

只有本地通过 Ready 的处理可以激活会话。Start、RegisterOutput、Control 与 Fence 不能创造新会话身份。设备拒绝不同 boot/session、过旧 generation、已退休 epoch 与已使用的 playback sequence；入队时再次原子校验，避免等待队列空间期间取消后仍提交旧音频。

- `Control` / `ObserveControl` 操作原 binding、后端、hold ID、command ID、sequence 和 intent digest。只有精确的可恢复回执及仍有效的本地新鲜度屏障才可释放生产门；未知结果保持关闭。
- 普通 `Fence` 推进匹配会话的最低可接纳 generation；精准 playback Fence 只取消所指原播放。它们不能替代对整个原输出的退休证明。
- `RetireOutput` 按原 scope、binding 与可用的原 backend 身份清理，即使新会话已成为当前 owner，也不能误停新会话。结果只接受可证明的 `NoNativeOwner`，或等待原软件门及已进入写入稳定后的 `Settled`。未知/已淘汰身份、RPC 成功、或泛化 stop 成功都不是退休证明；超时仍保留原操作和所有权。
- `close_session = true` 按精确 boot/session 关闭原会话。新会话的 UUID 与采集所有权保护它免受旧关闭请求影响；普通 Fence 的 `applied = false` 仍须结合返回身份解释。

`server_turn` 模式拒绝旧的无会话身份播放、停止及直接提示音 RPC，防止绕过新协议；旧 `utterance` 模式保留其独立入口，不能用它验证本协议的暂停行为。

## 证据边界

播放 v3 回执区分 accepted、submitted 与 presented。CPAL 完整 callback 帧写入和 ALSA 成功 writei 只证明提交；软件 `Held`/`Settled` 说明门控与已进入写入的边界，不证明扬声器已经静音。`presented_samples` 没有真实设备时钟证据时保持缺失，`SubmissionFinished` 不代表声学播放完成。完整定义见[Delivery 设计](delivery-design.md)。

真实 PG 账本、软件后端配对 gRPC、真实 Easy-Turn 模型分别验证不同部分。它们不能替代 Mac/Pi 声卡、实际停止时间、AEC、双讲听感或长期负载验收；模型 `inference_ms` 也不是端到端声学延迟。
