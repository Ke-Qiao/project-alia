# Delivery 与设备证据

状态：2026-09-08，D1 / D2 实现已冻结并通过最终整包回归。本文描述当前实现与剩余边界；测试和提交见[构建记录](progress.md)。D3 身体命令见[设计](body-command-design.md)。真实声卡、机构到位、AEC 与自然全双工声学效果仍未验收。

Delivery 保存内容向输出端交付的意图与已知事实。Execution 保存外部操作的批准、领取和结果。响应被替换不能撤销已发生的 effect，工具成功也不证明用户听到了语音说明。

## 规范响应与唯一发送

Response 在首次输出前固定用户、请求、渠道、可信来源消息、source privacy epoch 和生成意图。逐句音频 Delivery 各自保存不可变内容，共属一个 Response。最终正文与唯一助手会话消息在同一 PostgreSQL 事务提交；重试返回原 UUID，改意图或最终正文拒绝。

Delivery 在网络 I/O 前保存播放 UUID、原始 VoiceScope、单调 playback sequence、内容与所有者。数据库 CAS 只将 dispatch permit 交给一个监督者；重试读取原记录，不能从已有记录重建发送许可。过期或失联转成 Unknown，不作为重放队列。用户查询、取消及错误语义见 [Delivery API](delivery-api.md)。

新响应、媒体准入与心跳、最终历史提交复核原来源版本；撤权或遗忘后不能给旧任务换版本。已发生的交付事实仍须保存。缓存、ContextPack 与 Companion 的完整来源失效继续属于未完成主线。

## 播放 v2 与取消

`alia.playback.v2.Playback/Push` 使用 `Start → Audio* → End → EOF` 输入，保留独立回执。Start 携带 contract version 2，Server 先收到序号 1、接收量为零的对应确认才发送 Audio。RPC 路径直接升级，避免旧设备忽略精准取消字段后执行旧整轮停播。

每帧与回执携带 `PlaybackIdentity { VoiceScope, playback_id, playback_sequence }`。VoiceScope 是原始 boot/session/generation，不能在输出时借用当前会话。只有本地 Ready 可激活会话；Start/Fence 无权创建或替换。Audio 序号、源偏移连续，End 总数一致。同 epoch 后续 Push 在上一输入 End 确认后 FIFO 追加，不清上一片段尾部；新 generation 淘汰旧媒体。

媒体接纳与 generation 失效使用同一边界，排队期间失效的旧媒体不能迟到进入。等待 Start 时撤权也须在实际接纳前复核，不能先推进 epoch 再检查权限。输入 EOF 不解除尾部的绑定监督，见[语音协议](voice-protocol.md)。

精准取消携带原 playback UUID/sequence，立即关闭该身份准入，再等待已进入的输出边界收尾，ACK 携带最终快照。Push 和取消使用同一身份的单调回执序号；取消快照可能是 ACK 丢失后首次可见检查点，Server 不能重盖序号。没有最终证据则 Unknown。whole-scope Fence 只证明匹配 boot/session 的 generation 栅栏；close_session 退休匹配会话。迟到低 generation 或不同 UUID 不能停止新会话。

## 证据与单位

所有计数使用**均值下混后的源 PCM 帧**。源立体声一帧仍算一帧；重采样、设备声道展开不改变单位。字段缺失表示无证据，不能自动填零。

| 事实 | 证据和边界 |
| --- | --- |
| accepted | 身份校验、连续源帧进入有界队列。EndAccepted 只关闭输入。 |
| submitted | 实际 CPAL callback 完整帧写入或 ALSA writei 成功返回，关联后端实例、格式、源位置映射。物理呈现仍未知。 |
| presented | 设备播放游标经过对应源位置，不证明用户听到或声学质量。当前后端不提供。 |
| completed | EndAccepted 且呈现证据覆盖全部源帧，才可触发真正 PostSpeech。 |
| cancelled | 未 dispatch 已撤销，或精确控制与已进入写入边界已核验。保留真实前缀，不能承诺硬件缓冲尾音为零。 |
| failed / unknown | 故障，或断线、超时、证据缺失。保留已知前缀，禁止默认重播。 |

同一身份满足 `presented ≤ submitted ≤ accepted ≤ offered`。序号严格递增，计数、End、已知后端不可回退。终态不可复活；未知终态后取得的真实证据独立审计，不据此触发重播。`SubmissionFinished` 对应 Submitted，不能升级 PresentedFinished 或触发 PostSpeech。ConversationTurnCompleted 只表示处理/历史完成。

## 后端与重采样

输出格式在启动时固定；ALSA 共用采集时钟时可配置 capture sample rate，播放期间不改硬件时钟。已删除无实际作用的 `alsa_playback_follow_source_rate`。每个 playback 使用跨分片连续的 Rubato sinc 状态，补偿一次滤波延迟。End 零填充排空到 `ceil(source_frames × output_rate / source_rate)`，不截掉已接受尾部。

提交前缀使用补偿后的名义源时间映射 `floor(actual_output_frames × source_rate / output_rate)` 并封顶源长度；该软件映射不是设备时钟。静音补零不增加前缀。队列有界背压，控制独立于满媒体队列。

取消停止新输出租约，已进入的完整 callback 帧或 writei 先回报实际提交量再发布终态；不能丢掉立体声半帧后把下一片段写到错误声道。声卡故障后 scoped Push 不隐式重启 native 设备，恢复归生命周期所有者。DSP 测试覆盖分片相位、末端脉冲、源位置映射和包括 192 kHz → 8 kHz 的频谱检查，不代替 Pi 真机性能或声学验收。

## 任务所有权

Server DeliveryService 拥有有界 admission、媒体 future、精确控制与进度持久化；调用方丢弃等待不丢失媒体 owner。心跳和媒体数据库观察可并行轮询，避免暂停持锁媒体 future 后再等同一锁。

关闭顺序是停止新投递 → 取消原身份 → 收拢真实工作 → 关闭 channels → 释放资源与数据库。数据库、精准控制、整体观察都有期限，心跳不刷新绝对期限。超时只结束本次等待，仍运行的 native/adapter future 保留到实际退出；不可恢复 panic 保留 owner 与依赖并报告 incomplete，不再 poll 已 panic 的 future。

Embedded PlaybackSession 拥有 Push actor、输入 lease 和观察者。End native enqueue handle 完成并释放输入 lease 后才确认输入结束，后端观察继续。response drop、撤权或会话关闭取消原身份并等待真实工作。PlaybackSession 在 Audio 之前清理，不能先释放声卡依赖。

## 表达与下一主线

Server 表达归 ExpressionRuntime 实例，已删除全局活跃表与 legacy JSON 自动升级 completed。回执须匹配命令、scope 和 typed actions。当前 Embedded 未实现 `expression-command-v1`，身体动作明确 skipped。Affect 持久化改为实例有界队列并随 runtime 排空，见 [Expression D1](expression-evidence.md)。

以下仍是本轮未完成主线，不能归入用户延期项：

1. D3 身体唯一 writer、有限反重放、绑定/voice 栅栏、精准取消及 Submitted/位置反馈分级。
2. CLI/Text Delivery 的持久发送所有权与回执，独立文本输出不能用空 Audio 冒充交付。
3. 缓存、在途 ContextPack、输出和 Companion 派生状态的完整来源失效。
4. 实际播放时钟、AEC、可恢复暂停、带作用域本地提示音、真机双讲和动作安全验收。

Web/iOS/Android、Embedded 通用适配、未来设备推理/合并部署按[台账](deferred.md)保留。
