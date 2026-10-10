# L12：共享音频预算所有者

基线 `d5481b5cf4a6eeb28f535f08a8b1b9be7b31d5c9`，分支 `codex/architecture-refactor`。本分工实现、静态交接与 Root 同源运行验收完成 **7/7**。未提交、未执行 Cargo/数据库/容器/服务/硬件动作，未访问用户 5433。

## 归属与实际调用链

`modules::delivery::audio` 现在定义 `ResponseAudioBudget`、`AudioBudgetOwner`、`AudioReservation`、`EncodedAudio`、`AudioBuffer` 和共享内存限额。此预算在 TTS 之前由 Interaction/Companion 创建，贯穿 provider 复制、Delivery 原输出持有、Channel 解码及发送，HTTP 预览也使用独立 request owner；它不是 GPT-SoVITS 或任何 TTS 实现的私有资源。

`providers::adapter::TtsAudioStream` 保留 provider 返回流契约。只有 GPT-SoVITS 使用的累计流字节上限移至该实现内部，数值仍引用共享 `MAX_RESPONSE_ENCODED_BYTES`。删除 `providers::tts::audio_budget` 文件、模块声明和旧导出，没有兼容重导出或新 trait/feature/依赖。

本分工完成 provider adapter、GPT-SoVITS/HTTP body、NoopTts、API preview、Interaction 正常/错误路径、Companion 及对应 fixtures 的预算类型迁移。outbound 分工完成 Channel 及所有 `AudioBuffer` 导入迁移，出站 payload 直接引用 canonical budget 类型。

`delivery/service/output.rs` 继续在原 registration 之后、durable register control 之前设置同一 Response 的 `OnceLock<ResponseAudioBudget>`，保持句间空隙及 native tail 中的 scope 存活。这里只改预算归属和 `Arc<dyn DeliveryTransport>` 类型；不移动生命周期步骤。

## 保留的保证

- 单 payload 1 MiB、单 owner 8 MiB、全局 256 MiB、最多 4096 个活跃 owner；独立 PCM payload 2 MiB；GPT-SoVITS 累计流仍为 8 MiB。
- 原 Response UUID 在同一 scope 存活时共享 local semaphore；preview/request 使用不同 owner 变体，不能冒充 canonical Response。
- local credit 先于 global credit 取得，取消 global 等待时 RAII 归还 local permit；字节复制前必须取得 reservation。
- `Bytes::from_owner` 让 clone、slice、队列、derived PCM 和 HTTP response body 共享最后一次释放；保留编码 origin，不因 producer 返回或取消过早释放。
- `AudioBuffer::input` 仍服务入站与本地 PCM，不能制造 provider 的 `EncodedAudio`；derived PCM 仍有独立容量校验。
- 暂停、精确取消、原输出 retirement、当前政策、Shadow 和数据库事务行为未在本分工改变。

## Checklist

- [x] 追踪全部预算定义、producer、consumer、API、fixture、模块导出和资源生命周期。
- [x] 将共享预算实现与 6 个原测试整体迁至 Delivery；删除旧 provider 入口。
- [x] 将 provider stream 别名与 GPT-SoVITS 私有累计上限留在各自正确层。
- [x] 迁移全部非 Channel 调用及 fixtures，与 outbound 分工完成 Channel/AudioBuffer 导入交接。
- [x] 保留 output owner 的预算持有与取消释放时机，未新增不必要抽象。
- [x] scoped rustfmt、`git diff --check`、算法静态等价和原测试逐字对照通过。
- [x] Root 在冻结源码上运行完整回归与 feature 检查，归档最终证据并更新本项。

## 静态证据与 Root 验证入口

静态记录：`run/l12-budget-20261010/static-audit.json`。对比基线旧 `audio_budget.rs` 与新 `audio.rs`，除模块首行文档、provider 返回流 alias/私有上限抽出和测试文件名外，实现逐字相同；迁移后 6 个预算测试逐字相同，SHA256 `33c7b863d82a92a83ff868631e951c25cb9b01faed342e1cceef88c160449de0`。全量 Server 源码与测试已无旧 `audio_budget` 模块引用。这些静态对照不代替运行验证。

Root 统一验证应覆盖 `modules::delivery::audio::tests` 六项（owner 复用、clone/slice/PCM/HTTP 最后释放、global 等待取消回收、公平等待、容量拒绝、owner registry/preview 隔离），原 GPT-SoVITS HTTP/流边界测试，Interaction 音频身份验证、`output_lifecycle`、`duplex_playback` 与无默认 feature 全目标构建。现有测试已覆盖此次纯归属迁移的资源保证，没有重复新增镜像测试。

Linux 播放清理超时由另一个分工跟进。本分工没有修改任何 timeout、取消 fence 或 native cleanup 实现；最终运行结果及未定位风险以 Root 检查点与 timeout HANDOFF 为准。

未执行运行测试、真实模型或物理硬件验证。没有已知预算行为变化；实施阶段待验证的全量 trait/import 编译与跨模块生命周期回归，现已由 Root 在冻结源码上完成，见下文。

## Root 最终验收补记

源码 `d9f97216a6599742c45e0a6389110b6b465e8461`，V4 manifest `bc542c0718b07fc45c1f7e09084a51fddfa3350014eee089514e6a582f9e75ea`，641/641 Git blob 一致。

macOS workspace **1060/0/5**、真实 PTY **3/0/0**、无默认 availability **2/0/0**、提前准入 **1/0/0**；Linux ARM64 Server 默认并行完整测试 **778/0/5**。两端无默认全目标检查及 Linux release/native link 均通过。计数依次为 passed/failed/ignored，子进程摘要不重复计入。 相关取消/音频预算/原实例回归已在全量中通过；详见 [Root HANDOFF](root-HANDOFF.md)。先前“未执行”仅指本 subagent 的实施阶段，现已由 Root 完成上述验证。
