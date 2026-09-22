# Duplex media byte budgets

工作分支 `codex/duplex-media-budget`，从 `779f4f8` 继续。此表跟踪 C 包，暂停控制和 output actor 的纵向接线由主代理完成。

- [x] **CM01 恢复检查**：读主 checkpoint 与本包 HANDOFF，确认工作树基线及 owned 文件。证据：`run/duplex-media-handoff/HANDOFF.md`；起始工作树干净。
- [x] **CM02 类型迁移**：TTS 必须带预算并返回 EncodedAudio；全部生产调用和 fake 编译，无无预算默认回退；规范 prepare_response 早于 TTS。真实 PG 在每次 TTS 调用时查询已提交的 Response/context/source，并验证两个句子共用同一身份。
- [x] **CM03 额度生命周期**：1 MiB payload / 8 MiB Response / 256 MiB global，租约随 clone/slice/queue/HTTP Bytes 保留；取消、最后 drop 释放，Weak 注册有界清理。6 项隔离 semaphore 测试覆盖同 owner、全局竞争、取消、队列、派生 PCM、注册表上限与 Vec capacity。
- [x] **CM04 实际 HTTP 边界**：大 Content-Length、未知长度/无限 chunked、累计限额、错误正文 4 KiB、pitch 分拆与 EOF、取消、正常小 WAV。8 项实际 loopback HTTP 测试通过；另 1 项错误 WAV 格式测试通过。
- [x] **CM05 解码边界**：WAV 切片保留租约；PCM 转换/decoder 工作区与原编码额度分开，复制前检查独立上限；默认 Channel 不无限累积流。测试覆盖 incomplete PCM frame 加新输入超限时，原 pending 数据和 capacity 不变。
- [x] **CM06 定向回归**：预算/HTTP、Pipeline 纯测试、错误准备拒绝、Embedded 输出相关测试通过；含协调后的 PG 2 项，共 59 项通过。PG 窗口已释放。
- [x] **CM07 编译与冻结**：默认/无默认 all-targets、格式/diff 通过；命令、退出码、日志与源码摘要保存在本包证据中；本分支正常本地提交，不 merge/push。

所有本轮日志存放在 `run/duplex-media-validation/`。初始 WIP 的 exit 101 保留在 `run/duplex-media-handoff/check-all-targets.log`，不是本轮验收。

## 实现契约与整合入口

`ResponseAudioBudget::for_response(response_id: Uuid) -> anyhow::Result<Self>` 不变。相同仍存活的 Response 返回同一额度；规范 `prepare_response` 成功后才能调用，不能为了额度临时创造 Response。full、stream、sentence、error notice 四条 Pipeline 路径均在 TTS 前准备规范 Response。独立 voice preview 使用 `for_standalone_request(request_id)`，身份与 Response 域分开。

TTS 的两个必需方法均显式接收 `ResponseAudioBudget`：buffered 返回 `EncodedAudio`，stream 返回 `TtsAudioStream`。默认 helper 只委托，不存在无预算 Vec 返回通道。`EncodedAudio` 只能从 prior reservation 构造；seal 计入 Vec **capacity**。`AudioBuffer` 的 clone/slice 和 `into_bytes` 共享 `Bytes::from_owner` 中的原租约。输入音频或本地 PCM 的 `AudioBuffer::input` 不能构造 TTS 的 `EncodedAudio`。

HTTP buffered 正文最多 1 MiB；stream 每个保留 payload 最多 1 MiB、整个请求累计最多 8 MiB，即使消费者持续丢弃旧块也不能消费无限流。首次 body credit 在发请求前取得，后续 credit 在轮询正文并复制前取得。错误响应及远端 weight setup 的错误正文最多读取 4 KiB。pitch 改写仅保留固定 36 字节前缀，并核验 canonical PCM16 WAV 字段；不完整前缀、闲置超时、累计越界作为错误传播，不能当正常 EOF。

所有额度等待随 future/stream drop 释放。全局 FIFO 由 Tokio semaphore 提供；测试显式取消排队项并检查局部 credit 回收。Weak registry 每次 admit 清理，最后 owner drop 也删除自身入口，最多 4096 个同时活跃 owner，不按历史 UUID 无限增长。

句子 TTS 的 producer/audio/LLM 本地 futures 以 `try_join!` 传播错误，不把 TTS 或 decoder 失败仅写日志后返回成功。真实 PG 失败用例验证原 Response 没有成功提交、没有完整回复重播；已有固定 error notice 若获独立规范 Response，可单独尝试。

主任务的 output actor 必须从规范 Response 的 `for_response` 持有 clone，直到原 producer 和所持媒体都终止；不得在暂停时重新建预算或释放 owner 后把续传当新响应。C 包的音频类型和所有 TTS 调用已经接通；**Held/Resume 背压与原 owner 的跨暂停生命周期由 DU01 整合包继续接入和验收**。

## 范围与限制

编码预算不证明全进程 RSS、reqwest/HTTP/TLS/socket 内部缓存、HTTP JSON/base64 序列化字符串、Proto PCM/native 全局额度或声学恢复。decoder 的输入与 pending 工作区另有 2 MiB 限制；PCM 输出和 Proto 队列属于下一阶段，不与全局 encoded credit 混算。原 AudioBuffer 在 decoder 显式复制并 drop 后可以释放 encoded credit；还持有原切片或派生 AudioBuffer 时则保留。没有引入新依赖、修改 Proto 或宣称硬件呈现/AEC 已验收。

## 验证记录

以下命令都在本工作树执行，`--locked --offline`。真实 HTTP 只使用 `127.0.0.1:0` 测试服务。PG 仅使用任务库 32768，fixture 每次随机 schema，正常、panic、超时路径都清理；没有访问用户数据库 5433。

| 命令（公共前缀 `cargo`） | 结果 | 日志 |
| --- | --- | --- |
| `check -p alia-server --all-targets --locked --offline` | exit 0 | `check-default-final.log` |
| `check -p alia-server --no-default-features --all-targets --locked --offline` | exit 0 | `check-no-default-final.log` |
| `test -p alia-server --lib providers::tts --locked --offline` | exit 0，21/21 | `tts-tests-final.log` |
| `test -p alia-server --lib channels::pipeline::integration_tests::sentence_ --locked --offline -- --test-threads=1` | exit 0，2/2，显式任务 PG URL | `pipeline-pg-sentence-final.log` |
| `test -p alia-server --lib channels::embedded::playback::tests --locked --offline` | exit 0，9/9 | `playback-final.log` |
| `test -p alia-server --test pipeline --test embedded_channel --locked --offline` | exit 0，17/17 + 3/3 | `pipeline-unit-final.log` |
| `test -p alia-server --lib channels::pipeline::error_notice::tests --locked --offline` | exit 0，4/4 | `error-notice-final.log` |
| `test -p alia-server --lib channels::pipeline::streaming::tests --locked --offline` | exit 0，3/3 | `sentence-expression-final.log` |

平台为 macOS / Darwin arm64，Rust `1.94.1 (e408947bf 2026-03-25)`。default 测试实际编译并链接执行；无默认特性为 all-targets 编译检查，本包没有 Linux/Windows 或物理音频硬件运行证据。最终 21 个变化 Rust 源文件经 `rustfmt --edition 2024 --config skip_children=true --check` 检查；`git diff --check` 通过。TTS/PG 结果采于最终语义修改后，随后仅格式化；最终 manifest 在格式化后固定，后续定向编译/测试未再改源码。

恢复时保留的失败证据：`check-default-initial.log`（fixture import/AudioPayload 迁移，exit 101）、`check-default-second.log`（测试常量 import，exit 101）；`tts-tests-initial.log` 20/21（256 次 acquisition 耗尽 Tokio cooperative poll budget，测试改为 unconstrained 观察 semaphore 排队，未改生产公平性）；`pipeline-pg-sentence.log` 1/2（错误地把独立固定 notice 也计为回复重播，改为核对实际请求文本、原/notice Response 身份与原提交状态）。这些日志不作为绿色验收。

最终源文件 SHA-256 清单为 `run/duplex-media-validation/source-manifest.json`；默认/无默认检查及定向测试的机器可读结果为 `checks.json`。原 `run/duplex-media-handoff/HANDOFF.md` 是历史 WIP 记录，保留以解释 `779f4f8`，不代表本次完成状态。
