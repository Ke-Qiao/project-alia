# Delivery 与设备呈现证据设计

状态：待实施设计，2026-09-08。当前 scoped playback 已实现接收身份、顺序和取消栅栏；真实设备呈现回执、播后表达及用户实际听到的内容均未验收。本文只定义下一批完整实施边界，不扩大到 frontend、Pi 性能研究或 AEC 算法研发。

Delivery 表示内容交付到输出端及其已知进度。Execution 表示外部操作的批准、领取和事实结果，两者保持不同身份与状态：响应被替换不能撤销已经发生的 effect；一个工具成功也不证明用户听到了其语音说明。

## 当前已有的保证

- `VoiceScope { device_boot_id, session_id, generation_epoch }` 来自输入原始会话；异步工作不得改用渠道“当前会话”重新盖章。
- 端侧 active session 只能由当前 VoiceSession Ready 本地激活；旧服务端 Start 无权切换会话。Start/Audio/End 携带 scope、playback_id、playback_sequence，Audio 校验 sequence/sample_offset。
- 同一 epoch 可按单调 playback_sequence 追加多个已结束输入的 Push，保持 FIFO；未 End 的 Push 会使新 Start 得到 busy。新 epoch 和 Fence 清除旧媒体，晚到的旧 scope/epoch 不能覆盖当前播放。
- backend 入队带 expected local generation 原子校验，控制不等待媒体容量。close_session 按精确 boot/session 退休整个会话，不要求调用方猜最终 epoch；旧 UUID 的迟到 close 不能关闭新会话。
- 当前网络回执只发 `accepted/cancelled/failed`、`accepted_samples/end_accepted`；`submitted_samples/presented_samples` 均为空。服务端只接受这一已实现的协议子集，不能假定 Proto 中存在 `COMPLETED` 枚举就已有实现。

证据：[VoiceScope](/Users/ke-qiao/Developer/project-alia/crates/alia-core/src/delivery.rs:1)、[Playback v1](/Users/ke-qiao/Developer/project-alia/crates/proto/proto/playback.proto:1)、[端侧 receipt](/Users/ke-qiao/Developer/project-alia/packages/alia-embedded/src/grpc/playback_service.rs:169)、[服务端 Push 校验](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/channels/embedded/playback.rs:58)。真实声卡、机构到位和自然全双工声学效果不在这些软件保证内。

## 已确认的差距

| 差距 | 代码证据 | 需要改正的语义 |
|---|---|---|
| Delivery 返回 `Result<()>`，丢失已确认的身份、计数和证据等级 | [ChannelAdapter 音频接口](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/channels/adapter.rs:188)、[Push 返回:120](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/channels/embedded/playback.rs:120) | `Ok` 目前只证明全部 PCM 输入被接受，不代表播放结束。上层需要 typed admission/progress，不能使用一个 Delivered 布尔状态覆盖所有阶段。 |
| 播后动作与对话完成事件早于播放完成证据 | [TtsDeliveryOutcome::Delivered:279](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/channels/pipeline/streaming.rs:279)、[PostSpeech:213](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/channels/pipeline/voice.rs:213)、[ConversationTurnCompleted](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/channels/pipeline.rs:3683) | scoped Push 在 EndAccepted 后即返回；此后可能仍有设备缓冲尾音。PostSpeech 不能绑定此返回。对话处理/历史持久化完成可以单独记录，但不能作为“播完/听完”的证据。 |
| backend 尚未提供按 playback 身份关联的进度 | [CPAL callback:357](/Users/ke-qiao/Developer/project-alia/packages/alia-embedded/src/module/cpal_audio/cpal_backend.rs:357)、[ALSA writei:463](/Users/ke-qiao/Developer/project-alia/packages/alia-embedded/src/module/cpal_audio/alsa_backend.rs:463) | CPAL 当前忽略 callback 时间信息，填充输出 buffer 只能支持未来 submitted 计数；ALSA `writei` 只证明交给驱动，不能当作 DAC 呈现。需要设备时钟及源采样位置映射，不能按音频长度 sleep 后宣告完成。 |
| 旧表达回执把无法验证的结果升级为 completed | [cancel fallback:115](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/expression/orchestrator.rs:115)、[merge_channel_result:411](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/expression/orchestrator.rs:411)、[legacy receipts:457](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/expression/orchestrator.rs:457) | 无法解析为 ExpressionExecutionResult 的 JSON 被回退成 completed，且 completion_acknowledged=true；legacy executed_actions 也被自动映射 Completed。数据缺失/格式错误应保持未知或失败，不能制造设备确认。 |
| 表达取消没有会话/命令身份，活跃表达状态是进程全局表 | [expression Proto](/Users/ke-qiao/Developer/project-alia/crates/proto/proto/expression.proto:1)、[Embedded cancel adapter:1434](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/channels/embedded/channel.rs:1434)、[ORCHESTRATION_STATE:19](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/expression/orchestrator.rs:19) | Cancel 只有 reason，迟到操作不能精确定位旧表达；全局表只按 channel/user 分键，会让同进程独立 Server 实例共享状态。表达的排他和优先级状态必须归实例/设备命令所有者。 |

## 证据契约

以下为目标契约，不是当前已发出的所有回执。音频计数统一使用 **该 playback 的源 PCM 样本数**；设备采样率、重采样、通道展开及硬件时钟另带映射信息。不能把设备输出帧数直接填写为源样本数。

| 事实 | 必需证据 | 允许对外表述 |
|---|---|---|
| `accepted` | scope/identity 校验通过，连续源样本已进入有界接收队列 | 设备已接受 N 个样本。EndAccepted 只表示输入结束且总数一致。 |
| `submitted` | 对应样本实际交给 backend/驱动的计数，带 backend 实例/时钟标识 | 已提交给音频后端。仍可能在系统缓冲或被取消。 |
| `presented` | 可验证的设备播放游标/时钟已通过对应样本位置 | 设备时钟证明该前缀已经呈现；不证明用户听到、音箱接通或声学质量合格。 |
| `completed` | EndAccepted，且设备呈现证据覆盖全部源样本，无尚待呈现尾部和已知失败 | 这一 playback 的设备呈现完成。只有此证据可驱动真正的 PostSpeech。 |
| `cancelled` | 精确 scope/playback 控制已生效，旧软件队列失效；保留最后已知前缀 | 未呈现部分不再继续交付。不能把已提交/已呈现前缀改成零，也不能保证没有硬件缓冲尾音。 |
| `failed` / `unknown` | 明确故障，或超时/断线导致最终呈现无法核验 | 交付失败或结果未知；保留已知前缀，不能默认重播整段。 |

计数在同一 identity 下只能单调增加，并满足 `presented ≤ submitted ≤ accepted ≤ offered`；未提供的证据字段是 unknown，不是零。若后端只能证明 submitted，则 presented 保持空，不能发 completed。未来收到 presented 而缺少 submitted 的协议组合应拒绝或在明确版本契约中规范化，不能猜测。

## 所有权和合法迁移

Server 的实例级 Delivery 所有者持有 playback 身份、原始 VoiceScope、生产者任务、已接受计数、progress receiver 和终止结果。接口应返回可查询的 admission/handle，让 pipeline 交出播放进度监督责任；interaction token 取消只能要求对应 scoped fence，不能直接 drop 唯一进度任务后声称取消成功。该所有者接入 [Server runtime 设计](/Users/ke-qiao/Developer/project-alia/docs/refactor/server-runtime-design.md) 的关闭监督。

合法主路径为 `Created → Streaming → EndAccepted → Completed`。accepted/submitted/presented 是该路径上的独立单调事实，允许输入尚在 Streaming 时设备已经呈现部分前缀。任意未终结状态可进入 Cancelled、Failed 或 Unknown；终态不可被迟到旧回执复活。EndAccepted 之后拒绝新 Audio，但 **不能结束呈现证据通道**。当前 adapter 在 EndAccepted 后拒绝后续 receipt，需要与端侧同时升级并声明能力，不能单端启用新增字段。

服务端设备会话持续监督与每次 Push 的输入生命周期不同。输入半关闭后，设备仍需报告已排队音频的进度；撤权、会话退休或 runtime 停止仍要清掉剩余尾部。每次新 Push 分配单调 playback_sequence，不能因不同 playback_id 自动 stop 同 epoch 的前一条。

端侧 PlaybackSession 继续唯一拥有 scope/floor 与本地 generation。backend 的每个 render segment 必须保留 playback 身份及源样本范围，不能在混合缓冲后只剩一个全局计数。CPAL callback/ALSA 写入路径只做有界计数和轻量通知，不进行网络、数据库或阻塞分配；进度聚合与 gRPC 回执放在受监督的工作任务。静音补零不增加源音频 presented；underrun、设备重建、seek/reset、采样率变更必须显式处理时钟连续性。

取消及关闭保留现有原子 expected-generation 栅栏。迟到 Cancel/receipt、旧 boot、旧 session、旧 playback_sequence 都不能改变新的状态。正常关闭顺序是停止新 Delivery admission → 精确取消/退休当前 scope → 等待控制与进度任务退出 → 保存最后已知证据 → 停 backend。设备失联时可返回 Unknown，不能等待无限或重放可能已呈现的整段。

## 表达与对话语义

表达的 `accepted` 只表示命令通过校验/排队；`started` 表示执行器开始；`completed` 必须有对应命令完成证据。开环舵机控制通常只能证明控制序列已发送，不能在没有位置反馈时声称机构到位。能力声明要明确支持的证据等级。

ExecuteExpression/CancelExpression 必须带不可变 command_id 和归属 scope；迟到旧 cancel 只影响其旧命令或旧 epoch。普通设备动作若没有语音 scope，需要独立设备运行实例与命令身份，不能凭空绑定当前语音会话。优先级、合并和抢占只能在此明确所有者范围内进行。

无法解析的表达 JSON、缺失 action receipts、空 command_id 或 completion_acknowledged 自相矛盾，均不能回退为 completed。删除 legacy 自动升级逻辑，并同步修改测试预期。输入 EndAccepted 后没有 presented 能力时，PostSpeech 应记录 `unsupported_evidence` 而跳过；不要用保守时长 sleep 伪装完成。

将“对话生成/历史持久化完成”“Delivery 已接受”“设备呈现完成”分成不同事实。旧 ConversationTurnCompleted 若保留用于后台调度，应明确仅表示对话处理完成，并禁止其消费者据此推断用户听完；需要实际呈现的行为订阅专门 Delivery 事件。Memory 的来源时间/源消息验证保持 P2 契约，不因 Delivery 的处理延迟刷新。

## 分包实施与独占范围

以下全部待实施；自然全双工硬件验收在最后单独记录。

| 包 | 完整结果 | 建议独占文件 |
|---|---|---|
| D1：诚实结果与实例所有权 | typed Delivery admission/progress、取消/未知终态、纠正 PostSpeech 和旧表达 completed 回退、实例级表达状态 | `crates/alia-core/src/delivery.rs`、Server `channels/{adapter,embedded/playback}.rs`、`channels/pipeline/{streaming,voice}.rs`、必要 `pipeline.rs/event.rs/state.rs`、`expression/{orchestrator,types}.rs` 及测试；共享装配由 root 统一修改。 |
| D2：真实 backend submitted 与进度传输 | backend 分段身份和源样本映射、提交计数，输入 EOF 后保留证据流、并发取消与停机 join | Embedded `module/{cpal_audio*,traits,playback_session}.rs`、`grpc/playback_service.rs`、Server `channels/embedded/playback.rs`、必要 `playback.proto` 能力/版本扩展与测试。共享 Proto/Cargo 由 root 负责。 |
| D3：设备呈现和表达命令确认 | 支持的后端提供设备时钟呈现，其他后端明确未知；表达带 scope/command identity，旧 cancel 不能影响新命令 | CPAL/ALSA 后端、PlaybackSession/receipt、`expression.proto`、Embedded expression service/motion runtime、安全校准相关接口、Server embedded/expression adapter。任何硬件实现文件先分配独占；不改变校准/限位默认值。 |
| D4：设备证据验收 | 实际端到端呈现、取消尾部、断线未知与声学测试记录 | 真实硬件测试脚本及验证文档。没有硬件证据时，只交付软件与submitted层，不宣称 D4 完成。 |

D1 可以在尚无硬件 presented 的情况下完整交付：结果保持 Accepted/Unknown，播后动作明确跳过。D2 不得为了给 presented 填值扩大证据。D3 需要按实际设备能力决定可支持的层级；Mac/ALSA/Pi 的表现分别记录，不能套用其他机器性能或时钟结论。

## 真实测试出口

- 模拟 gRPC 设备先 EndAccepted、长期扣住完成证据：Server 不能发布呈现完成、不能执行 PostSpeech；生成/持久化事实可以独立结束。
- 输入半关闭后继续报告单调 submitted/presented；倒退计数、超过 offered、身份混用、End 后 Audio、重复终态全部拒绝。能力版本不兼容有明确失败。
- 以可控 fake backend 分开推进“接收、提交、呈现”三步；验证重采样和部分写入的源样本映射、补零不计入呈现、同 epoch 多 Push FIFO 和部分取消前缀保留。单元测试用手动推进时钟，不用等待音频估算时长制造结果。
- 队列已满时 Fence 及时生效，旧 generation 入队原子失败；旧会话/旧命令 cancel 和迟到回执不影响新播放。断线或回执期限到达产生 Unknown，恢复连接不自动重播整段。
- 表达返回 `{}`、错误字段类型、缺失 receipt、仅 accepted、控制序列发送但没有位置反馈时，一律不得制造 completed 或机构到位。两个同进程 Server 实例的表达优先级状态互不影响。
- 实际 CPAL/ALSA 设备分别记录 submitted 与时钟呈现证据、underrun/reset、取消后剩余硬件尾音。外部声学观测可核对延迟，但设备 clock presented 不等于人实际听到。
- 真实硬件自然全双工另测同时播放/采集、回声下误触发、打断和跨轮污染。AEC 和声学验收继续挂起，不以软件 stream 并发通过替代。

执行后分别报告软件协议、backend submitted、设备 clock presented、机构反馈、声学五层结果及未测项；不把这些证据压成一个“播放成功”勾选。
