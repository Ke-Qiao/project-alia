# 持续语音与播放协议 v1

本文件描述本轮实际接入的控制边界。Easy-Turn 固定运行在 Server；设备侧保留采集、播放和绑定失效处理。物理播放时钟、AEC、可恢复暂停和真机双讲仍需后续主线验收。

## 采集与规范轮次

`alia.voice.v1.VoiceSession/Stream` 是设备到 Server 的双向 gRPC 流。第一帧必须是 `OpenSession`，携带协议版本 1、设备启动 UUID 和渠道标识。Server 验证当前签名和绑定，分配新的会话 UUID，返回 `Ready`。设备验证身份和格式并在本地激活该播放会话后，才开始发送采集帧。

当前输入为 16 kHz、单声道、PCM16 little-endian；每帧最多 1,600 个采样。`sequence` 从 0 连续递增，`sample_offset` 是该流内的单调采样偏移。凭证刷新帧不消耗音频序号。采集溢出、乱序、格式错误或流中断会结束该会话，重新唤醒后必须获得新会话 UUID，不能把旧缓冲拼接进去。

VAD 只提供语音活动证据。Server 以有界窗口调用 TurnDetector，模型观测携带请求 ID 和 generation。只有当前窗口版本的有效观测才可提交决策：

| 观测 | 当前策略 |
| --- | --- |
| complete | 提高 generation，停止旧回答，建立播放栅栏，再把转写送入可靠语音处理入口 |
| incomplete | 保留窗口并继续听，不生成回答 |
| backchannel | 清理这一段附和输入，保持当前回答和 generation |
| wait | 提高 generation、取消当前回答并停播；当前还没有按实际播放游标恢复的能力 |

每个渠道只有一个会话所有者。每个回答保留其产生时的 boot/session/generation；迟到的异步任务不能在发送时查询“当前会话”并借用新身份。

## 凭证和会话退出

设备每 120 秒签发新的认证 token 并发送 `RefreshAuth`。Server 在同一个会话循环中重新验证签名、设备、scope、渠道和当前绑定，成功后才替换凭证；每 30 秒复核当前凭证。300 秒签发时间新鲜度约束保留。

设备还立即及每 250 ms 检查本地绑定快照。解绑、进入交接状态或 server ID、endpoint、公钥变化时，停止旧流并关闭它的本地播放会话。该检查覆盖上传结束后仍在队列中的音频尾部。设备网络任务退出、取消或错误时由所属会话的关闭 guard 清理；旧 guard 不能关闭新 UUID 的会话。

## 播放接收与栅栏

`alia.playback.v1.Playback/Push` 接收 `Start → Audio* → End → EOF`，并返回接收回执。每一帧都携带同一个 `VoiceScope`、播放 UUID 和递增的 `playback_sequence`。Audio 另有从 0 开始的连续帧序号和采样偏移；End 声明这一段的总采样数。

只有本地 `Ready` 处理可以激活一个会话。服务端的 Start 和 Fence 不能创造或替换设备的会话身份。设备拒绝不同 boot/session、过旧 generation 和已使用的 playback sequence。

同一个 generation 的后续语音片段在上一 Push 完整结束后按 FIFO 追加，不因新 playback UUID 而丢弃前一片段尚未播放的尾部。较新的 generation 会淘汰旧音频。网络身份检查与本地队列使用同一个预期 generation，必须在入队时再次原子校验，避免“校验后等待队列空间”期间发生取消却仍写入旧音频。

`Playback/Fence` 使用独立 RPC，不等待媒体队列容量：

- 普通 Fence 只提高匹配 boot/session 的最低可接受 generation；迟到的低 generation 请求不能中断较新的回答。
- `close_session = true` 按精确 boot/session 关闭整个会话，不受它曾经经过多少代回答影响。新会话的不同 UUID 保护它免受旧关闭请求影响。
- `applied = false` 可能表示幂等或不匹配，调用方必须核对返回的活动身份和 generation，不能只依据 gRPC 成功判定栅栏已经建立。

端侧 `server_turn` 模式拒绝旧的无会话身份播放、停止及直接提示音 RPC，防止绕过新协议。旧 utterance 模式暂保留旧 RPC 路径；现有主动表达和本地提示音还需要进入规范 Delivery 后接入新协议。

## 证据边界

当前回执只证明音频已被队列接受，包含 `accepted_samples` 和 `end_accepted`。`submitted_samples` 与 `presented_samples` 没有设备证据时保持缺失。本轮不得依据 gRPC 返回、接收完所有 PCM 或计算出的音频时长填写实际播放完成。

本地 generation 控制测试、真实 loopback 协议测试与真实 Easy-Turn 推理分别提供不同证据。它们不证明 Pi 的实际声学停止时间、回声消除效果、用户听见音频或机构动作到位。
