# L12 出站契约 HANDOFF

基线 `d5481b5cf4a6eeb28f535f08a8b1b9be7b31d5c9`，分支 `codex/architecture-refactor`。本分工实施、静态审查与 Root 最终运行验收完成 **9/9**。本 subagent 已冻结源码，未提交，未运行 Cargo、数据库、容器、服务或硬件动作。

## 实际职责与调用闭环

`modules::delivery::transport` 是唯一出站契约所有者。`DeliveryTransport: Send + Sync` 定义稳定 channel/account 绑定、仅本地身份预约、带持久化 observer 的文本/音频投递、精确取消、Response 绑定注册、可恢复控制、只读控制结果观察、原输出所有者退休。`DeliveryTarget`、AudioPayload/AudioFormat、两个 stream、observer 与 report 同归该契约。AudioBuffer 直接引用 `delivery::audio`，没有旧路径重导出或兼容别名。

DeliveryService、media supervision、VoiceOutput / OutputProducer 仅持有 `Arc<dyn DeliveryTransport>`。ChannelAdapter 改为 `ModuleLifecycle + DeliveryTransport`，继续负责入口认证、双 epoch 准入、通道能力、动作与 voice generation/session admission fence。CLI 和 Embedded 直接实现真实出站 trait，Arc upcast 保留同一个具体 adapter 及其原生资源，没有 facade、downcast、重新查询 registry 或新增 runtime owner。

`fence_voice` 保留 ChannelAdapter：唯一生产调用是 `grpc/voice_session.rs::fence_output`，用于语音入口换代/关闭的 generation 或 session 屏障。Delivery 需要的精确 PlaybackIdentity 取消和原 backend retirement 已移入 DeliveryTransport。该判断来自实际调用图，既有默认“不支持 voice 输出则无须 fence”的条件原样保持。

`audio_streaming()` 作为真实出站 payload 接受能力移入 DeliveryTransport，默认 false；Embedded 返回 true，流式测试 OutputChannel 返回原 streaming 标志。删除 ChannelCapabilities 重复字段，Companion 与 Interaction 读取同一个方法。HTTP list_channels 没有输出该字段，ChannelCapabilities 不可序列化，api/voice 只使用 voice_input / voice_output；没有删除已有 HTTP 诊断字段。

拆分时发现 OutputProducer 原先反向给 Interaction 返回完整 Channel，供表达动作和能力政策使用。因此 `RecognizedVoiceOutput { producer, channel }` 在识别语音入口显式捆绑原 producer 与原完整 adapter。没有两个可错配的独立 Option。gRPC 从原 admitted source 一起构造，Interaction 在原 request/account/scope/channel_id 校验之外验证 `Arc::ptr_eq`（完整 adapter upcast 后与 producer transport 相同），然后继续使用原完整 Channel。当前 registry 同名替换不成为旧输入的所有者。

## Fixtures 与风险回归

全部 ChannelAdapter 实现拆分并迁移新的规范类型；3 组专用 Delivery 测试 `delivery_service`、`delivery_text_service`、`output_lifecycle` 删除无关 ModuleLifecycle / ChannelAdapter 实现，只实现 DeliveryTransport。这些真实 PostgreSQL 投递用例验证 Delivery 不再依赖入口或生命周期 trait。

扩展原 recognized voice 集成测试为 `recognized_speech_keeps_original_channel_and_rejects_same_name_replacement`：

1. producer 绑定原 adapter，输入却捆绑相同 channel_id / owner 的另一实例；拒绝且模型、历史、文本、音频均没有副作用。
2. registry 换成同名新实例，再处理原正确捆绑输入；原 adapter 完成原文本和音频，新实例保持无输出。
3. 保留原 transcript、source time、双 epoch、独立 turn identity、history、共享 TTS 路径断言。

NativePlayback/PanicFencePlayback 的两个 fixture 由 playback subagent 独占拆分；Root 统一验证其原 native future / backend cleanup 保证。精确取消、单次 dispatch、真实进度观察、预算/信用释放及未知 ACK 不重试生产语义没有修改。

## 静态证据

`run/l12-outbound-20261010/static-audit.json` 记录基线和 **25** 个 CLI/Embedded 方法体的归一等价检查；仅忽略规范路径、ChannelTarget → DeliveryTarget 及 audio_streaming 原字段到方法的投影。DeliveryService / media 的正文除 imports 和 trait/target 类型外相同。scoped rustfmt 和 git diff --check 通过；全 src/tests 扫描无旧 ChannelTarget、channels::adapter 音频/文本/observer 路径、旧 TTS audio_budget 路径或旧 capabilities.audio_streaming。

预算 subagent 只读交叉审查了 HTTP 诊断、original Channel 捆绑及新的流式能力路径，没有发现阻断项。不要把静态比较当作 Cargo、数据库或真实硬件运行证明。

## Checklist 与恢复步骤

- [x] 追踪所有 transport 类型、生产实现、调用方、fixtures、exports 和配置依赖。
- [x] Delivery 拥有真实出站接口；删除 Channel 旧方法/类型入口和 AudioBuffer 别名。
- [x] CLI、Embedded、所有测试实现迁移；原资源实例保持。
- [x] 3 组专用 Delivery fixtures 删除 Channel 与 lifecycle 依赖。
- [x] voice fence 归属判断及 audio_streaming 的生产/HTTP 调用闭环。
- [x] 原完整 Channel 与 producer 绑定，补同名替换拒绝和原实例正确输出回归。
- [x] 25 个 adapter 方法和 service/media 正文静态等价、路径扫描、rustfmt/diffcheck。
- [x] 交叉审查与本 HANDOFF 完成，源码冻结，Root 接管所有后续改动。
- [x] Root 在冻结输入上完成编译、相关集成/并发/取消/关闭测试和同源证据归档。

Root 已完成统一 checklist、恢复检查点和源码提交；实际编译中的未使用 import 修正及最终验证轨迹均归档，没有加旧路径兼容层。本分工无新增依赖、feature、平台、模型或硬件动作，默认 Shadow 和现行政策不变。

未执行 Cargo、PostgreSQL、Linux 或实机验证；Linux 播放清理超时的根因与阶段观测由 playback subagent / Root 交接，不在本分工宣称修复。

## Root 最终验收补记

源码 `d9f97216a6599742c45e0a6389110b6b465e8461`，V4 manifest `bc542c0718b07fc45c1f7e09084a51fddfa3350014eee089514e6a582f9e75ea`，641/641 Git blob 一致。

macOS workspace **1060/0/5**、真实 PTY **3/0/0**、无默认 availability **2/0/0**、提前准入 **1/0/0**；Linux ARM64 Server 默认并行完整测试 **778/0/5**。两端无默认全目标检查及 Linux release/native link 均通过。计数依次为 passed/failed/ignored，子进程摘要不重复计入。 相关取消/音频预算/原实例回归已在全量中通过；详见 [Root HANDOFF](root-HANDOFF.md)。先前“未执行”仅指本 subagent 的实施阶段，现已由 Root 完成上述验证。
