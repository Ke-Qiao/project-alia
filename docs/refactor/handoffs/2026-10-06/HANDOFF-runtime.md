# Runtime / 单一语音链交接（2026-10-06）

分工：`linux_runtime_1004`。本轮代码、示例配置和当前文档已冻结，Root 统一运行最终验证、记录总数和提交。此交接不代表工作区或真实硬件已验收通过。

## 已完成 checklist

- [x] 删除 `audio.proto`、Embedded `audio_service.rs`、Server 旧 `audio_ingress.rs` RPC；保留认证、账号/source epochs 与撤销校验到 `grpc/voice_auth.rs`。Root 完成共享注册和调用侧整合。
- [x] `SpeechPipeline` 只保留持续 VoiceSession v2：唤醒、有限 pre-roll、连续 PCM/VAD evidence、网络会话、scope 撤销。删除旧 utterance/capture/continuous/thinking/responses 分支和本地 STT 第二套 turn 决策。
- [x] 本地 wake cue 在校验原始 Ready 后最多触发一次。独立 worker 不阻塞 capture/network，保留原 scope，随 session 取消并 join；原生混音与 cue submission 由 audio 分工完成，见 [HANDOFF-audio](HANDOFF-audio.md)。
- [x] Pipeline 删除旧 AudioInput/active-turn interrupt 分支及 TranscriptCorrection 全链。同步删除角色、policy、生命周期字段、settings、OOBE 和示例片段。有效 VoiceTurn owner token、supersede 及持久化保护保留。
- [x] 删除无生产读取的旧 channel interrupt flags 和 `end_continuous_dialogue` 工具。CLI 等当前 channel event bus 入口保留，清除其错误的 legacy 命名。
- [x] ContextPolicy 不再为缺失 `appraisal` 补默认值或升级旧 digest。测试改为当前 seed 不覆盖已存策略、旧 JSON/坏 digest 拒绝且数据库行不改写；保留 typed-f32 JSONB digest 与 negative-zero 正确性覆盖。
- [x] KeywordMatcher 删除自动 cancel 别名和无音频 action 规则。音频引用须非空、权重有限且大于零；store 拒绝没有有效引用的插入/更新，失败 bundle 保留源配置。Root 同步 settings 管理入口的 InvalidArgument 与无写入回归。
- [x] 示例只保留 `config/direct_response.example/wakeword/`。`sample.wav` 原样复用仓库原 thinking 样例，PCM16 / mono / 16 kHz / 0.28 s；README 明确它仅为测试素材，不是自然唤醒提示。本轮没有生成或下载音频。
- [x] `speech.channel_id` 明确选择 Server 实例；同一原值经 `start_session → exchange → capture_stream` 进入 Open 与强制认证 metadata。默认 `embedded` 只代表实际同名实例；两端示例使用 `pi-living-room`。没有恢复 type alias 或扩展 binding 协议。
- [x] channel ID 在反序列化、启动和发送边界拒绝空白、控制字符及非 ASCII，不 trim 后改写路由。设置失败不落盘。日志改为 `continuous capture enabled`。
- [x] Embedded 当前 README、architecture、configuration、audio-and-voice、gRPC 文档和 Server configuration/runtime-systems 已同步唯一语音契约及当前限制；历史验收材料未重写。

## 验证交接

本分工只运行 owned 文件 `rustfmt --edition 2024 --config skip_children=true` 与定向 `git diff --check`，均通过。没有运行 Cargo、PG、容器、服务或硬件动作。样例音频字节校验 SHA-256：`6e9184e6ec2f6fa8ac76d7076ee982855241fce7f0752824d1e7c5b6907c2f4c`。

Root 的命令、输入 manifest、完整运行结果和总数以 [本轮 checklist](../../checklists/cleanup-2026-10-06.md) 与 [本轮记录](../../cleanup-2026-10-06.md) 为准。v1/v2 的已记录失败不抹除；本交接写入时，最终 v3 由 Root 接续，不推断最终通过数。

交给统一验证的关键回归：

- `speech/duplex/tests/cues.rs`：Ready 前不触发提示、capture 不被提示阻塞、会话取消能 join、真实 loopback 缺失/外来 scope 拒绝。
- `tests/speech.rs`：WAV downmix、无效格式、超过五秒/4 MiB 拒绝。
- `pipeline/turn.rs`：owner cancellation、supersede、旧 generation finish 不移除新 owner、迟到取消输入不覆盖当前 owner。
- `tests/appraisal_provenance.rs`：`initialization_preserves_current_policy_and_rejects_unsupported_or_damaged_rows`。
- Embedded `tests/direct_response.rs` / `tests/storage.rs`：无自动 cancel、无效音频引用拒绝、无效更新不改记录、失败 bundle 保留源文件。
- `configured_channel_instance_reaches_open_and_auth_metadata_unchanged`：真实 loopback 对 `pi-living-room` 同时验证 Open 与 metadata。
- `invalid_voice_instance_ids_do_not_replace_runtime_settings`：非法 ID 不更新内存设置或配置文件；现有 roundtrip/Open 测试同步自定义 ID。

## 后续 checklist

- [x] Root 已完成最终工作区、无默认 features、当前 Linux release/test/run 验证，记录实际覆盖范围并正常提交；不要把旧 Linux 或 v2 结果当成最终源码证据。
- [ ] 设计受认证、带原始 owner、确认 capture 关闭的显式 **input-session close** 控制，再接“结束对话”工具。Playback Fence 只控制输出准入，不能冒充麦克风会话关闭回执。
- [ ] 其他本地提示需求若重新引入，必须通过现有 scoped cue admission；不得恢复裸播放、本地 transcript 控制或旧 thinking/follow-up 旁路。
- [ ] R02：硬件 AEC 与声学全双工测量。两路混音不是 AEC，native submission 也不证明物理播放或静音。
- [ ] R03：真实动作硬件证据；V01 根因定位；前端新契约接入、更多嵌入平台/硬件适配、端上模型部署仍按总待办后续执行。

当前自然全双工仍由服务器 Easy-Turn 提供 turn evidence；Pi 只做采集、VAD evidence、scope 和原生输出。本轮不改默认 Shadow、暂停区分政策、绑定或设备动作范围。
