# Duplex media budget — WIP checkpoint

状态：2026-09-20 按主代理的额度收口要求停止扩展。**本包未完成、当前不能编译，不可合并或作为完成验收。** 此检查点保留真实中间状态，供下次继续。

- 工作树：`/Users/ke-qiao/Developer/project-alia/run/worktrees/alia-duplex-media-budget`
- 分支：`codex/duplex-media-budget`
- 基线：`8c3f5450fcedfe250cba7936de6fcdf8076c7040`
- 唯一编辑者：`/root/text_delivery`。未引入其他 worktree overlay。
- 本包未访问 PG，未修改 Proto、Embedded、Cargo.toml 或 Cargo.lock。
- 本包无 Docker 容器。唯一编译 exec `96416` 已结束，退出码 101；没有仍运行的子任务。
- 主代理授权保存 `wip:` 阶段提交；不合并主分支，不 push。

## 已确定契约

目标是编码音频的真实保留额度，不是全进程 RSS 或所有 PCM/HTTP 内部缓冲的证明：单 encoded payload 1 MiB，同一规范 Response 当前保留 8 MiB，进程全局 256 MiB。TTS HTTP 完整响应最多 1 MiB；流式单 payload 最多 1 MiB，另有整次 HTTP 响应累计 8 MiB 上限，防止未知 Content-Length 的无限流。错误正文最多读取 4 KiB。预算必须在应用复制/累积 body 之前取得。

主代理已认可 `ResponseAudioBudget::for_response` 作为原 output actor 的预算入口，actor 后续应保留 clone，直到 producer 与媒体结束。所有 TTS 必须先完成规范 `prepare_response`，不能为额度创建假的规范 Response。独立 voice API 使用明确的 StandaloneRequest 预算身份。

已落盘的精确 Rust API 位于 `providers/tts/audio_budget.rs`：

```rust
pub enum AudioBudgetOwner { Response(Uuid), StandaloneRequest(Uuid) }
pub fn ResponseAudioBudget::for_response(response_id: Uuid) -> anyhow::Result<Self>;
pub fn ResponseAudioBudget::for_standalone_request(request_id: Uuid) -> anyhow::Result<Self>;
pub fn ResponseAudioBudget::owner(&self) -> AudioBudgetOwner;
pub fn ResponseAudioBudget::reserved_bytes(&self) -> usize;
pub async fn ResponseAudioBudget::reserve(&self, bytes: usize) -> anyhow::Result<AudioReservation>;
pub async fn ResponseAudioBudget::copy_from_slice(&self, bytes: &[u8]) -> anyhow::Result<EncodedAudio>;
pub fn EncodedAudio::into_buffer(self) -> AudioBuffer;
pub fn EncodedAudio::into_bytes(self) -> axum::body::Bytes;
pub fn AudioBuffer::input(bytes: Vec<u8>) -> Self;
pub fn AudioBuffer::slice(&self, range: impl RangeBounds<usize>) -> Self;
pub fn AudioBuffer::derived_pcm(&self, bytes: Vec<u8>) -> anyhow::Result<Self>;
```

`EncodedAudio` 是进一步收紧后的返回类型：只有先取得 reservation 才能构造，防止 ingress/PCM 的无 encoded 额度构造器被当成 TTS 返回值。`AudioBuffer` 则用于 Channel 的通用数据。`Bytes::from_owner` 持有 RAII reservation；clone、slice、转 HTTP Bytes 不复制媒体、不重复计费，也不能提前释放原额度。seal 按 Vec capacity 计入保留值。以上行为**尚未通过测试**。

TTS trait 已要求两个显式实现，无旧 Vec 默认回退：

```rust
async fn synthesize_request(&self, request: &TtsRequest, budget: ResponseAudioBudget)
    -> anyhow::Result<EncodedAudio>;
async fn synthesize_stream_request(&self, request: &TtsRequest, budget: ResponseAudioBudget)
    -> anyhow::Result<TtsAudioStream>;
// TtsAudioStream = Pin<Box<dyn Stream<Item = Result<EncodedAudio>> + Send + 'static>>
```

plain `synthesize` / `synthesize_stream` helper 仍有，但必须带预算，且只委托上述必需实现。Noop 已适配；旧测试 fake 未适配。

## 已落盘但未验收的实现

1. `audio_budget.rs`：进程级全局 semaphore；按 Response/StandaloneRequest 复用 Weak owner 表；每次 admit 清理过期弱引用；最多 4096 live owners；最后 Scope drop 移除自身弱引用。双 semaphore reservation 的 future 被取消应由 RAII 释放；需补验证。
2. `http_body.rs`：有 prior reservation 的 bounded body 累积、Content-Length 检查、4 KiB 错误正文截取。明确 reqwest/TLS/socket 内部 framing 不等于应用媒体保留额度。
3. `gpt_sovits.rs`：用上述路径替代完整 `bytes().await` / `text().await`；流式在复制前拿 credit，累计限额；错误与 weight setup 错误正文也限额。pitch 仅暂存固定 36 字节；改写前核验 RIFF/WAVE、首个 fmt chunk、PCM16、channels/rate/block alignment/byte rate。**流式 try_stream 内 ensure 的写法当前编译失败，须先修。**
4. `channels/adapter.rs`：`AudioPayload.data` 改 AudioBuffer；删除默认无限合并流，未实现 bounded stream 的 channel 明确 unsupported。只改音频数据/默认音频流部分，未改控制方法。
5. `pipeline/streaming.rs`：增加 `prepare_audio_budget(modules,target,channel_id,turn)`，先 await 规范 prepare_response 再取预算；full/stream/sentence 路径开始接入。WAV→PCM 改范围切片以保留原 encoded 租约；silence 增加单块 2 MiB 限制并返回 Result。
6. `pipeline/error_notice.rs`：error TTS 先取得准备后的规范 Response 预算。旧 cancellation fixture 没有真实 Response/Context，需要重构测试，不能填一个假的 response_id 绕 gate。
7. `api/voice.rs`：两个独立 TTS API 用 StandaloneRequest；直接 HTTP Bytes 返回保留租约。JSON/base64 路径仍是独立序列化阶段，不声称 encoded 额度覆盖其额外字符串分配。
8. `pipeline.rs`：只有 STT 输入转换使用显式 to_vec 的类型适配，未改控制/主流程。

## 尚未完成的必要连接点

- `channels/embedded/audio.rs` 两处 PCM 构造仍返回 Vec；应从原 AudioBuffer 调 `derived_pcm`（或无复制切片），保留来源租约至明确的 decoder/PCM 边界。对所有 PCM/header 复制在复制前加独立上限。
- `channels/embedded/playback.rs` 仍 `pending.extend(payload.data)`，新类型不是 iterator。不可为了编译给 AudioBuffer 加隐式按值复制的 IntoIterator。改为明确读取；审清原 lease 在 header 累积及 decoder 输出期间的持有与释放；Proto PCM 队列是独立有限边界，不宣称全局 encoded 额度覆盖它。
- `grpc/audio_ingress.rs` 原 AudioPayload Vec 构造需用明确 input 构造器，沿用已有 ingress 限制。
- `error_notice` 与 pipeline integration fake 的旧 TtsAdapter 方法签名未改；需要使用传入 budget 构造 EncodedAudio。
- Embedded playback fixtures、`tests/embedded_channel.rs`、`tests/delivery_service.rs`、`tests/pipeline.rs` 等 AudioPayload 和 silence/helper 类型适配尚未做。后面的 integration-test 编译可能还有错误，当前 lib 错误已使 all-targets 提前失败。
- 旧 streaming pitch rewriter 单测仍使用 Vec→Vec 接口，未更新为固定 header 接口。应补非 canonical fmt、分拆前缀及不完整 EOF 反例。
- root 的真实 output actor 尚未接入持有预算 clone；Held/恢复背压未实现。这属于后续纵向联调，不得声称端到端暂停媒体已经有界。
- HTTP/预算回归还未编写。也未执行真实 HTTP、单元测试、PG、Linux 或整包回归。

## 当前验证记录

仅运行一次编译检查，命令在此工作树执行：

```text
cargo check -p alia-server --all-targets --locked --offline
exit = 101
```

完整日志：`check-all-targets.log`。诊断为 lib 5 个错误、lib-test 22 个错误，主要是上述未迁移 AudioPayload/TtsAdapter 调用端、try_stream 中 ensure 类型错误和旧 pitch 单测。日志行号对应格式化前的源文件。

之后仅对 11 个 owned 源文件执行 `rustfmt --edition 2024 --config skip_children=true`，退出码 0；`git diff --check` 退出码 0。格式化后未再编译。**不存在绿色构建或通过测试的证据。**

## 下次建议顺序

1. 先阅读此提交和编译日志；确认 root 已提交的 Stage1/后续接口变化，避免覆盖 ChannelAdapter 控制代码。
2. 修 try_stream 的错误传播（使用可在 generator 内传播的 `Err(...)?`），完成最小调用端类型迁移；不增加隐式无预算 TTS 回退。
3. 核查 single-payload、累计流、header 和错误正文在每次应用分配/复制前确已持有足够 reservation；并核查所有带租约的 PCM 派生/队列路径。
4. 补预算单测：同 Response 共用 8 MiB、不同 Response 共享 global、等待取消释放、最后 clone/slice/HTTP Bytes drop 释放、Weak map 清理/上限、reserve/seal capacity、并发公平性。
5. 补真实本地 HTTP 回归：超大 Content-Length 无 body 复制；chunked 单块/累计超限；未知长度无限流；多块正常小 WAV；4 KiB error body；中途取消/流 drop；pitch 分拆、格式验证与 EOF；request Timeout。必须检查额度归零/仍被 clone 持有的事实。
6. 检查 prepare_response 严格早于 TTS 请求，并让原 output actor 接持有预算。需要规范 PG fixture 的管线测试等主代理安排 PG 窗口，不自行复用用户数据库。
7. 编译默认与无默认 all-targets；运行上述定向测试和受影响 Pipeline/Embedded 输出回归。真正通过后再文档冻结和正常完成提交。

本检查点所有源文件摘要保存在 `source-manifest.json`；WIP 提交 SHA 由 agent 最终报告提供。
