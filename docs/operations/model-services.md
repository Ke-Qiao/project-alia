# Model Services

Project Alia treats models as replaceable services behind provider adapters.
This document lists the current service roles without going deep into provider
installation details.

The [software release record](software-release.md) separates verified binaries
and noop lifecycle evidence from model deployment information that still needs
confirmation. Example URLs and model names below are configuration examples,
not evidence that a service is installed or permission to start or probe it.

## 下一轮输入清单（2026-10-25）

真实模型方案未定不阻塞本轮软件候选固定。下面集中区分已确认的部署方向、历史候选和真正待定输入；不读取或归档密钥原值，也不从旧私有配置推导当前选择。

| 项目 | 已确认 | 历史候选／已盘点资产 | 下一轮待提供或核对 |
| --- | --- | --- | --- |
| 聊天与角色 | 复用当前 Rig／Execution 和角色装配；模型待定 | 曾拟测试 `https://botcf.com/v1`；旧主文件为 DeepSeek `deepseek-v4-flash`，持久设置另有不同 minor；示例的 Qwen／Gemini 也不是选择 | 每个实际启用角色的 profile：`major`、`minor`、`default`，以及 `memory_capture` 专用或 minor fallback、`sentence_expression` 专用或确定性规则。逐项给 provider ID、精确 model/version、base URL、API family、凭据来源名称；确认 streaming／tools／JSON 支持 |
| Embedding | 独立于聊天；模型与维度待定 | 旧文件有 OpenAI `text-embedding-3-small`／1536，持久设置有 local Nomic／768，二者冲突 | 选定 endpoint、model/version、实际 dimensions、凭据来源，核对 SQL active embedding profile；不得直接沿用旧值或静默关闭 Memory/cache |
| Easy-Turn | 在 Mac Server，Pi 5 不承载；已有 Python 3.10 CPU 固定栈 | `Developer/Easy-Turn` 与 `run/models/easy-turn-source` 历史 HEAD 为 `fe175eaa…`，固定 checkpoint／Qwen 目录已盘点；不是本轮 loaded 证据 | 确认复用的隔离环境、source/checkpoint/Qwen 绝对路径、实际文件身份及 loopback endpoint；启动后核对加载版本、能力、冷启动／后续延迟和服务 owner。MPS 不是当前锁文件已通过路径 |
| GPT-SoVITS／Juno | 用户指定本机 Developer 内服务及 Juno 权重方向 | 历史 checkout `08d627c…`、CPU／v2ProPlus，`Juno-e30.ckpt` 和 `v2Pro/Juno_e16_s80.pth` 文件存在；旧 Alia 配置的权重／参考路径为空 | 确认完整 GPT＋SoVITS 权重组合与摘要、voice ID、服务版本和 `/tts` URL；选参考音频及逐字匹配的文本、参考／目标语言、TTS 主机上的绝对路径；明确独占实例还是共享服务、谁可切权重 |
| 独立 Whisper | **是否进入首版单独待定**；现有 VoiceSession 使用 Easy-Turn transcript，不重复 STT | 历史发现 `/Users/ke-qiao/.local/share/alia/whisper.cpp`、HEAD `968eebe…` 与 `ggml-large-v3-turbo.bin`；旧配置 `large-v3` 只是说明标签 | 先回答首版是否需要显式 HTTP 音频转写；需要才固定服务／实际权重／base URL、样本语言和验收用例。它不作为已有设备语音链的重复环节 |
| 调用与素材范围 | 先有限组件检查，再生产整链；默认 Companion shadow 保持 | 既有离线 PCM 和 R1 硬件基线可复用 | 非敏感文本／离线 PCM、参考素材使用范围、允许的远端供应商和有限调用预算、私有输出位置；凭据只提供受控文件位置或变量名 |

这些资产盘点来自[此前模型预检交接](../refactor/handoffs/2026-10-17-model-preflight/acceptance-HANDOFF.md)，不是本轮重新扫描或运行结果。聊天基址可用不代表同站点有 embedding；磁盘存在或权重切换 HTTP ACK 也不代表实际加载字节已核实。当前角色只使用配置中已实现的五个聊天角色，不从历史方案恢复其他角色入口。

收到输入后，先生成任务独立配置，核对 base／`.d`／环境补空／`settings.toml`／SQL policy 的有效组合，并完成不涉及现场硬件的真实组件预检。然后通知现场窗口，在 Mac Server＋Pi Embedded 上集中验收：**生产启动 → 讲话 → 模型处理 → 回复播放 → 打断／恢复 → 取消与下一轮对话**。该轮包含正常换气长句、系统 Start／Stop、生产单次初始化后的真人语音；复用已通过的插拔、方向拾音、试听和静默恢复基线，不把 fixture 音频算成真人语音通过。

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
