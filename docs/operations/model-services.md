# Model Services

Project Alia treats models as replaceable services behind provider adapters.
This document lists the current service roles without going deep into provider
installation details.

The [software release record](software-release.md) separates verified binaries
and noop lifecycle evidence from model deployment information that still needs
confirmation. Example URLs and model names below are configuration examples,
not evidence that a service is installed or permission to start or probe it.

## 下一轮输入清单（2026-10-25）

用户已选择 Magpie 的本地 Qwen3.8 27B 作为 minor、DeepSeek Flash 作为 major，并授权其余按推荐处理。minor 的标准高思考版与 Instant 由业务需求选择：采用 **Instant、关闭思考**，适配路由／分类原 5 秒和记忆捕获原 3 秒期限；复杂任务沿用 major 升级路径。512K 是该服务的上下文配置，不提高 Alia 的上下文、输出、并发或期限预算。本节替代先前的模型待选清单；软件候选 `6123691516fae1c2b0e813652eaa1104c33da9af` 及归档保持独立。

本次只读核对 Magpie 0.1.1436 的模型目录、LM Studio 的模型／已加载实例元数据，以及磁盘资产；没有发送推理请求、启动／加载／切换模型、下载权重、修改运行配置或 SQL、接触 Pi。目录能力不是实际工具调用、延迟或模型输出的验收证据。

| 项目 | 已确认 | 历史候选／已盘点资产 | 下一轮待提供或核对 |
| --- | --- | --- | --- |
| 聊天与角色 | `minor` → `lmstudio/local/qwen3.8-27b-512k-instant`；`major` → `deepseek/deepseek-flash`；`default` 绑定同一个 major profile；省略 `memory_capture` 使用 minor fallback，省略 `sentence_expression` 使用确定性规则 | 两个路由均在 Magpie 目录；Instant 为 MLX 4bit，已存在的实例报告 context 524288／parallel 1／默认思考 off。DeepSeek 目录标签为 V4.1 Flash 1017 | 共用 `http://127.0.0.1:3425/v1`、`chat_completions`；两 profile 的 `provider_specific.reasoning_effort="none"`，保留原输出预算。下一轮验证参数转发、streaming／native tools／JSON、取消和原期限；供应商 alias 与目录标签不能代替不可变权重版本 |
| Embedding | **本地 Qwen3-Embedding-0.6B、GGUF Q8_0、1024 维**，复用 LM Studio `http://localhost:1234/v1/embeddings`，不增加 Alia adapter 或模型框架 | LM Studio 内置 Nomic v1.5 Q4_K_M／768 权重存在、完整 SHA 已核对、目录显示未加载；Qwen embedding 尚未获取。旧 OpenAI／1536 和 Nomic／768 配置不作为本轮有效选择 | 固定 Qwen 文件身份后获取／导入；读取实际 runtime model ID 并验证 1024 维、中文检索和资源共存。核对 SQL active profile，保留 canonical memory，显式切换并重建投影。云端 `text-embedding-v4/1024` 仅为实测不满足需求时的备选，不自动切换或混用向量 |
| Easy-Turn | **复用项目内固定 CPU 栈**：`services/easy-turn/.venv`、`run/models/easy-turn-source`、`run/models/easy-turn/checkpoint.pt`、`run/models/qwen2.5-0.5b-instruct`；loopback 8091、1 worker、并发 1 | source `fe175eaa…` 干净；66 项包版本元数据匹配；checkpoint 及 Qwen 所需 7 文件完整 SHA 匹配。外部 `Developer/Easy-Turn` 副本有修改／环境差异，不选用 | 后续启动时验证实际加载身份、冷／热延迟及与聊天／TTS 共存。元数据匹配不证明 ABI；MPS 不是当前固定栈已通过路径 |
| GPT-SoVITS／Juno | **Alia 独占 loopback 实例**，CPU／FP32／v2ProPlus；复用 `Juno-e30.ckpt`＋`v2Pro/Juno_e16_s80.pth` 及 BERT／HuBERT／SV 基础资产 | 两权重 SHA 分别为 `b8830efd…a01c94`、`050dba36…76d`；现有 runtime 为 Python 3.10.17、torch 2.12.0、torchaudio 2.11.0，兼容性未验证。参考候选 `Juno_zhcn 3/demo/audio (3).wav` 为 8.98 秒／32kHz／mono／PCM16 | 先检查 runtime，再在后续授权窗口核对参考声音与逐字文本／语言，固定完整 `/tts` endpoint 和两权重 loaded 身份。此轮没有试听或转写；版本号差异不直接判作已证实冲突，不与其他客户端共享可切权重实例 |
| 独立 Whisper | **首版 VoiceSession 不启用**；直接使用 Easy-Turn transcript | 既有 whisper.cpp／large-v3-turbo 资产保留 | 以后确有独立 HTTP 音频转写需求时单列启用与验收，不给当前 VoiceSession 增加重复 STT |
| 调用与素材范围 | 先有限组件检查，再生产整链；默认 Companion shadow 保持；既有离线 PCM 和 R1 硬件基线复用 | Magpie 已配置所选上游，LM Studio 目录认证可用；凭据仅在内存中用于本机只读请求，未输出或归档原值 | 真实推理／服务启动仍未授权；进入下一轮时固定非敏感样本、调用预算和私有输出位置。major 的必要上下文仍会送往 DeepSeek，本地 embedding 不代表全部记忆／会话内容都只留本机 |

当前选择与只读证据在 [selected-plan.json](../../run/model-plan-20261025/selected-plan.json)、[Magpie 目录](../../run/model-plan-20261025/selected-chat-catalog.json)、[LM Studio 目录](../../run/model-plan-20261025/lmstudio-models-readonly.json)、[Instant 资产](../../run/model-plan-20261025/instant-assets.json)、[Nomic 资产](../../run/model-plan-20261025/nomic-bundled-asset.json)、[Easy-Turn 资产](../../run/model-plan-20261025/easy-turn-readonly-inventory.json)和[Juno 资产](../../run/model-plan-20261025/gpt-sovits-juno-assets.json)。先前推荐、[历史交接](../refactor/handoffs/2026-10-17-model-preflight/acceptance-HANDOFF.md)及冲突配置保留为历史，不覆盖本次决定。

Instant 的本地 hub 清单实际指向 `orcarouter/Qwen3.8-27B-Uncensored-MLX-512K-Text`，不是标准 `qwen/qwen3.8-27b` 仅切换思考参数。已记录 12 个文件的 SHA，包括约 16.05 GB 的三个权重分片、config／tokenizer／chat template 与 hub 映射；没有可据以认定的上游 revision。当前固定的是磁盘资产，不能证明已驻留实例仍使用相同字节。下一轮须验路由分类、记忆 JSON 与工具参数质量，Execution／授权边界保持原样。

本地 embedding 选择依据是中文／多语言定位、较小的共存开销和现有接口适配，不是 Alia 实测优于云端。[Qwen 官方模型卡](https://huggingface.co/Qwen/Qwen3-Embedding-0.6B-GGUF)列出 0.6B／1024 维和 Q8_0；拟固定 revision `370f27d7550e0def9b39c1f16d3fbaa13aa67728`、文件 `Qwen3-Embedding-0.6B-Q8_0.gguf`、SHA-256 `06507c7b42688469c4e7298b0a1e16deff06caf291cf0a5b278c308249c3e439`。Alia 目前不区分 query/document 指令，先按现有 plain input 验中文检索，不为选型增加 reranker 或协议。[Nomic 官方模型卡](https://huggingface.co/nomic-ai/nomic-embed-text-v1.5)的英语定位和任务前缀要求与当前契约不同，保留其可复用资产，不据此断言中文不可用。

[Magpie 接入文档](https://usemagpie.ai/docs/integrate)支持现有 OpenAI-compatible 聊天入口；本机 `magpie-alia` 只是调用方标识，不是供应商密钥。使用完整 provider/model 路由，避免 `auto/` 或裸模型名带来上游歧义。Embedding 直接走 [LM Studio embeddings](https://lmstudio.ai/docs/developer/openai-compat/embeddings)，不能由 Magpie 聊天目录推定其转发 embeddings。LM Studio 的真实 credential 留在受控来源，Alia 命名 provider 的 `api_key` 不会自动从任意同名环境变量读取。

[独立配置片段](../../run/model-plan-20261025/model-sections.toml)只记录本次选择，缺少待核对的 embedding runtime ID／凭据和参考逐字文本，**不是可启动的完整配置**。组装独立候选时须从最终有效角色表删除旧 `memory_capture`／`sentence_expression` 绑定；TOML 深合并中的省略不会删除旧值。不得直接叠加到旧私有配置后假定已完成切换。聊天基址可用、磁盘文件存在或权重切换 ACK 均不证明实际推理身份。

下一步先补齐 embedding 文件／runtime ID、Juno runtime 与参考逐字文本，生成任务独立配置，核对 base／`.d`／环境补空／`settings.toml`／SQL policy 的有效组合。得到启动与有限调用授权后，完成不涉及现场硬件的真实组件预检；原期限或资源共存不满足时保留失败并调整方案，不放宽期限求绿。组件出口具备后才进入现场窗口，在 Mac Server＋Pi Embedded 上集中验收：**生产启动 → 讲话 → 模型处理 → 回复播放 → 打断／恢复 → 取消与下一轮对话**。该轮包含正常换气长句、系统 Start／Stop、生产单次初始化后的真人语音；复用已通过的插拔、方向拾音、试听和静默恢复基线，不把 fixture 音频算成真人语音通过。

实际部署后另做升级／恢复及至少 24 小时混合负载，覆盖断网、模型超时、进程退出、设备重连、内存／线程／句柄／队列与日志磁盘上界。验收前固定记录上界与观察方式，不以放宽原期限或并发求绿。AEC 等商家确认后独立验证；具体首版 MCP 服务和第三方插件按实际目标验证，通用框架回归不代替它们。V01／V02 历史根因继续开放，扩大无人值守主动运行前再处理对应条件；ROS、执行器现场、固件重写和其他平台不因模型接入自动恢复。

## Embedded Models

Embedded keeps local audio intelligence in the existing Rust host:

- sherpa-onnx for local wake word keyword spotting, or R1 firmware wake events
- Silero ONNX through Rust `ort` for VAD activity evidence on the continuous capture stream

Model paths are configured in `config/embedded.toml`. The helper script
`scripts/download-sherpa-models.sh` populates the KWS and Silero VAD model
directories. Embedded does not run local STT or make semantic turn decisions.
Its bounded VAD endpoint evidence is interpreted by the Server's existing voice pipeline.

Embedded audio/model behavior is described in
[alia-embedded audio and voice](../../packages/alia-embedded/docs/audio-and-voice.md).

## Server Turn Detection

Easy-Turn runs only on the Server host in the current macOS Server + Pi 5
Embedded deployment. Embedded keeps wake word, VAD, continuous capture and
playback; it does not host this model. Enable it in
`config/server.d/providers/easy_turn.toml`:

```toml
[providers.easy_turn]
enabled = true
endpoint = "http://127.0.0.1:8091"
timeout_ms = 5000
max_concurrency = 1
```

There is no Embedded `speech.enabled` switch. For a later authorized device
deployment, select the capture/playback, wake-word and VAD modules, configure
`speech.channel_id`, and satisfy the normal binding/session requirements. Keep
the all-noop staging profile for preparation without devices; choosing real
audio modules can open devices during startup. Continuous VoiceSession is the
only device voice path. Preparation, pinned
revisions and the service HTTP
contract are documented in the [Easy-Turn service guide](../../services/easy-turn/README.md).
`/health` confirms readiness and `/v1/capabilities` reports the actual model,
format and concurrency limits. Readiness alone does not measure first-request
latency; record initial and subsequent calls separately when assessing load.

Server uses bounded PCM windows to obtain a transcript and Complete,
Incomplete, Backchannel or WAIT evidence. This is continuous capture with
windowed inference, not a model-native incremental decoder. The accepted model
transcript goes directly into the recognized voice input
path and is not sent through a second STT request.

Use matching voice v2 and playback v3 peers. A fresh Backchannel may release a
temporary acoustic hold; explicit WAIT retains the same output until explicit
Continue. The 3-second temporary semantic deadline, configurable 60-second
hold default (hard maximum 120 seconds), and original output lifetime are
separate from `timeout_ms`. Increasing a model HTTP timeout does not extend
resume authority. These are engineering defaults; see the
[voice protocol](../refactor/voice-protocol.md) for exact ownership and expiry
rules. Model labels and `inference_ms` do not prove physical speaker pause,
AEC or end-to-end acoustic latency.

## Server STT

Server STT currently uses a whisper.cpp-compatible HTTP service. The server
provider is configured under:

```text
config/server.d/providers/whisper_cpp.toml
```

The authenticated HTTP voice/transcription APIs can use this provider for
explicit audio requests. Device VoiceSession uses the Easy-Turn transcript
described above and does not issue a second STT request. The Whisper endpoint
is a base URL to which the adapter appends `/inference` and `/health`; its
configured `model` is informational, while the actual weights are selected by
the service startup configuration.

## Server LLM

Chat providers use the pinned Rig fork through `LlmService`; native tool calls
enter the shared Execution ledger. The former text-based tool protocol and
standalone provider parsers have been removed.

Chat roles select a model profile in `profiles.llm.<id>`, which selects a named
adapter in `providers.llm.<id>`. Major and minor are the main chat roles:

- minor LLM is usually a smaller/cheaper model and can be used for routing or lighter work
- major LLM is usually OpenAI-compatible and can also point at LM Studio if it
  exposes an OpenAI-compatible API

Record the actual provider ID, model ID/version, complete base URL, API family
and credential source for each selected role. OpenAI adapters support
`chat_completions` or `responses`; the chosen service must support that exact
family and the required streaming, tool and JSON behavior. Anthropic and Gemini
use their own adapters. An OpenAI-compatible label alone does not establish
these capabilities.

`default` reuses minor or major when their profile IDs match; a distinct profile
creates an independent chat model. All five roles participate in startup,
health and settings updates, with shared instances processed only once. An omitted
`memory_capture` role uses the minor fallback. An omitted `sentence_expression`
role uses deterministic expression rules. Dedicated optional roles must be
included in the deployment inventory: a local major model does not make other
roles local. Do not change capture policy or execution approval to simplify a
model test.

Configure these under:

```text
config/server.d/providers/openai.toml
config/server.d/providers/gemini.toml
config/server.d/providers/anthropic.toml
config/server.d/policies/routing.toml
```

Model profiles are in `config/server.d/profiles/llm.toml`. The repository example
includes remote providers for some roles; it is not an approved deployment.

## Server Embedding

The embedding role is independent of chat roles. The current real adapter uses
an OpenAI-compatible `/embeddings` endpoint. Confirm its provider, model ID,
actual vector dimensions, service/weight version and credential source. Only
`text-embedding-3*` requests send a dimensions parameter; other models must
already return the configured dimension. Response identity, ordering and vector
shape are checked, but the request descriptor's `request-config.v1` revision is
not proof of remote weight identity.

Memory and cache bind semantic use to the active profile in SQL policy. A
configuration mismatch does not authorize rewriting that policy; canonical
keyword retrieval can remain available while mismatched semantic use is
unavailable. Keep model identity confirmation separate from policy changes.

## Server TTS

Server TTS currently targets the GPT-SoVITS WebAPI. Provider endpoint and timeout
settings live under:

```text
config/server.d/providers/gpt_sovits.toml
```

Voice profile and reference-audio parameters live under:

```text
config/server.d/profiles/voice.default.toml
```

Files referenced by GPT-SoVITS config are paths on the remote TTS server, not
local files inside `alia-server`.

Confirm the service version, complete `/tts` URL, selected voice, reference
audio and matching reference text/language, target language, and actual GPT and
SoVITS weights. `reference_voice` maps to the remote `ref_audio_path`, and
`style_prompt` maps to `prompt_text`. Record a trusted asset/version identity,
not just a display model name.

When the default voice has nonempty weight paths, Server initialization calls
the service's `set_gpt_weights` / `set_sovits_weights` endpoints. Every configured
switch must succeed. Failure is returned, and synthesis remains blocked while
the remote state is unconfirmed, including a GPT success followed by a SoVITS
failure or cancellation. Reapplying the weight settings retries the complete
configured pair. Desired settings are retained on failure; they do not certify
the remote state. Weight changes wait for the original audio streams to finish
or be dropped, and new synthesis waits for the switch owner.

With no weight paths, Alia uses a preloaded instance and reports Degraded with
an unverified model identity. A successful switch acknowledges the requested
paths; neither this acknowledgement nor HTTP health attests the remote model
bytes or proves Juno acceptance. Use a dedicated instance when Alia owns weight
switching, since another client can otherwise change its state independently.
Initial software validation can save synthesized audio privately without
playing it.

Easy-Turn, whisper.cpp and GPT-SoVITS adapters currently have no configurable
API-key or arbitrary authorization-header support. Confirm any gateway's
compatibility before choosing it. Easy-Turn's CLI listens only on loopback; its
adapter rejects URL credentials/query/fragment and disables proxy/redirect
following. Do not infer a supported remote authenticated deployment from a URL.

## Configuration and Acceptance Boundaries

Review the full effective configuration before any later model run:

1. The selected base file and recursively sorted same-name `.d` fragments are
   merged first. See [Server loading order](../../packages/alia-server/docs/configuration.md#loading-order).
2. `DATABASE_URL` and `ALIA_JWT_SECRET` override their fields. Provider API-key
   variables only fill empty keys in the matching default provider IDs
   `openai`, `anthropic` and `gemini`; they do not automatically fill custom IDs.
3. Persisted `settings.toml` overrides registered settings, with module-specific
   application timing. A provider can initialize from base configuration before
   later runtime settings apply, including TTS weight-switch side effects.
4. Committed SQL Context policy takes precedence over its old TOML projection;
   embedding profile admission remains enforced. Settings are not a universal
   replacement for startup configuration or policy authority.

Do not archive raw API keys, full settings responses or private settings files.
Record the configuration identity and credential source name/path without its
value. Unconfirmed deployment values stay marked pending in the release record.

Keep LLM `health_probe_enabled=false`. With a configured key, a disabled probe
reports Degraded; an empty key reports Unhealthy. This is not an inference test,
and the noop staging assertion that all modules are Healthy must not be copied
into a real-model acceptance. GPT-SoVITS health also accepts HTTP 400 from
`GET /tts`; it does not synthesize speech. Easy-Turn readiness/capabilities,
Whisper health and remote model names likewise need separate real-output and
loaded-model evidence. Preserve original request deadlines, concurrency and
cleanup ownership instead of changing them to obtain a passing result.

The two existing ignored targets `easy_turn_live` and `voice_stream_live` call
real Easy-Turn. The latter constructs its response/output in a fixture; neither
is a real LLM-to-TTS chain test. Reuse them only for their stated scope. The
[next-stage acceptance steps](../refactor/handoffs/2026-10-17-model-preflight/acceptance-HANDOFF.md)
separate provider checks, Easy-Turn integration and the still-pending combined
model chain. No live target is enabled by this documentation update.

## Memory

Memory is an Alia-owned core system. Capture policy, privacy filtering,
relationship modeling, and scheduled memory meditation live in the server
runtime instead of a replaceable provider adapter. Configure memory policy
under:

```text
config/server.d/policies/memory.toml
```

Automatic memory capture is agent-policy driven by default. `capture_mode =
"llm_selected"` asks the assistant-side LLM to extract only durable user facts,
preferences, stable relationship context, goals, or habits before storing
Alia `MemoryNode`s. External services do not decide what should be remembered.

When Memory is enabled, startup accepts Healthy or Degraded and keeps it
enabled. Persistent Unhealthy after the configured attempts fails startup;
there is no silent policy change that disables Memory to make startup succeed.

Memory privacy controls are available through `/api/v1/memories/privacy`.
Setting `automatic_capture_enabled = false` switches capture to
`explicit_tool_only`, while the memory system remains available for explicit
user actions such as the memory UI/API or `memory_store` / `memory_forget`.
Automatic and chat-tool memory writes block sensitive categories by default
(credentials, identifiers, health details, religious views, and political
views). Sensitive memories are only accepted through direct user-managed memory
entry.

Prompt injection uses only configured retrieval layers, defaults to
`core`, `preference`, and `relationship`, strips metadata, filters sensitive
content, and applies a bounded character budget before adding memories to the
runtime prompt.

## Deployment Reminder

The server expects provider endpoints to already be reachable. Model download,
container startup, GPU/Metal tuning, and provider-specific performance settings
should live in setup scripts or provider deployment docs rather than in the core
architecture docs.

Server provider configuration is described in
[alia-server configuration](../../packages/alia-server/docs/configuration.md).
Memory capture modes and provider-neutral capabilities are described in
[memory operations](./memory.md).
