# Model Services

Project Alia treats models as replaceable services behind provider adapters.
This document lists the current service roles without going deep into provider
installation details.

The [software release record](software-release.md) separates verified binaries
and noop lifecycle evidence from model deployment information that still needs
confirmation. Example URLs and model names below are configuration examples,
not evidence that a service is installed or permission to start or probe it.

## 当前决定与 adapter 核查（2026-10-28）

用户已将 **minor 改为 Magpie 官方 DeepSeek Flash Instant**。本机目录没有独立 Instant ID，实际固定 `deepseek/deepseek-flash`、`chat_completions`、`provider_specific.reasoning_effort="none"`；不用 `auto/` 或第三方同名路由。major 原本也是该完整路由与 none，两角色仍保留独立 profile、1024／4096 输出预算，`default` 复用 major；省略 `memory_capture` 使用 minor fallback，`sentence_expression` 继续确定性规则。旧本地 Instant 不再是 Alia 启动依赖，既有资产／实例未因此删除或卸载。本地 Qwen embedding／1024、CPU Easy-Turn、独占 Juno、参考文本及 `ke-qiao` 登录会话保持。路由分类与记忆提取现在也调用 DeepSeek；本地 embedding 不代表全部会话／记忆内容只在本机。

当前已经使用纯 Rust 库：`LlmService` 复用锁定的 Rig 0.44.0 fork `c2ff3e30e6b8d84fc6759eab030656a42cae14ca`，无需增加 adapter 框架。以下是[实际构造代码](../../packages/alia-server/src/providers/llm/service.rs)已接通的聊天协议，不是对任意远端模型的兼容承诺。

| 协议 | 现有配置与实现 | 边界 |
| --- | --- | --- |
| OpenAI Chat Completions | `type="openai"`、`api_family="chat_completions"` → Rig `.chat()` | 官方或兼容端点均须核对实际模型、流、工具与 JSON 行为 |
| OpenAI Responses | `type="openai"`、`api_family="responses"` → Rig `.responses()` | 端点必须实际提供 Responses，不能仅凭 OpenAI-compatible 标签选用 |
| Anthropic Messages | `type="anthropic"` → Rig 原生 Messages | API version 可配置；结构化输出需 `json_schema`，单独 `json_mode=true` 会拒绝 |
| Gemini | `type="gemini"` → Rig 原生 Gemini | 已处理 `/v1beta` 与 JSON MIME；具体模型能力仍需组件验证 |

四路默认构建启用，[现有 HTTP fixture](../../packages/alia-server/src/providers/llm/service/http_tests.rs)覆盖流终止、工具参数／身份、预算、取消和协议请求格式，已包含在软件候选回归中。本轮只读核查，未重跑回归或发起模型请求。原生工具继续进入 Execution；新 minor 仍须在原路由 5 秒、记忆捕获 3 秒期限下验证，不放宽预算。若经 Magpie 的 Chat 兼容入口访问其他厂商模型，Alia 一侧仍选择 `openai`／`chat_completions`。

用户已澄清“system One”为口误并撤回，不列为待确认项或新增适配。Embedding 与聊天协议分开：当前真实 [embedding factory](../../packages/alia-server/src/bootstrap/factory.rs)仅接 OpenAI-compatible `/embeddings`，会拒绝 Anthropic／Gemini embedding provider；已选本地 Qwen 路径不变。

## 新 minor 与新库准备验证（2026-10-28）

[本轮汇总](../../run/fresh-start-20261028/summary.json)分别固定配置、有限模型调用、隔离新库与启动材料证据，不合并为生产整链通过。

[当前独立配置](../../run/fresh-start-20261028/config/server.production.pending.toml)、[组件配置](../../run/fresh-start-20261028/config/server.components.toml)均经原 Rust loader／factory 检查；运行软件仍为 `6123691`，没有改生产代码或依赖。生产配置保留 DB port 1、空 JWT、无 channel／discovery，不能直接启动生产。[Magpie 目录证据](../../run/fresh-start-20261028/config/magpie-deepseek-catalog-v2.json)支持官方路由和 none 参数；目录标签不能固定可变供应商 alias 的权重版本，也不把其他原生 endpoint 标签当作本轮实测。

使用原 Rust `LlmService` 诊断和原提示词，各一次新 minor 请求通过：路由分类 **788.167ms／原 5 秒**，瞬时问候的记忆捕获 **854.765ms／原 3 秒**，得到期望的 JSON；[完整结果](../../run/fresh-start-20261028/minor/summary.json)保留原始响应，没有重试。两个样本不代表业务语料准确率、并发／长上下文／断网表现或 p95；记忆样本只覆盖不应写入的瞬时问候。

核对本机 provider 元数据时发生一次凭据输出失误：DeepSeek key 曾误入工具结果和新建本地 JSON；本地 JSON 已脱敏，未提交或修改原凭据，旧工具输出无法撤回。用户已确认在 Magpie 更新 key，之后才执行上述请求；没有读取或记录新 key。仅保留[不含原值的事件记录](../../run/fresh-start-20261028/config/credential-handling-incident.json)。

此前只通过直接 HTTP 的 Juno 本轮已补齐 **Alia `create_tts`／`GptSovitsTts.init`／stream adapter**：独占19880，固定权重和文件名参考文本，一次真实合成通过；init **484.084ms**、首块 **5892.716ms**、全流 **6773.710ms**，7块／304684bytes，结束后编码预算为0。原总60秒／idle10秒不变，没有第二次求绿、播放或试听；这仍不是 Delivery／Pi／声学整链。原始流和[adapter结果](../../run/fresh-start-20261028/juno/adapter-result.json)保留，临时诊断代码不进入生产实现。任务19880已停止、原9880和源码／权重／参考文件保持；关闭时上游 `resource_tracker` 报告1个待清理semaphore，原警告保留，未测OS资源上界或通过重跑抹去。

新的 [Mac 服务材料](../../run/fresh-start-20261028/services/README.md)只要求 LM Studio 的选定 embedding 驻留，不再要求旧本地聊天实例或强制两模型 JIT 策略；敏感日志／登录设置仍未应用。15 项 ready fixture、3 份 plist、最终13项 Server gate／凭据FILE／exec拦截检查和停服日志轮转检查分别留证；它们不等于已安装、模型整套已就绪或运行中日志有硬上界。新空库真实发布二进制的首次启动／同身份重启结果见[新库入口](upgrade-recovery.md#新生产数据来源决定2026-10-28)。

## 历史模型选择与输入清单（2026-10-25）

以下 2026-10-25 各节保留当日决定、配置和验证证据；minor 选择以 2026-10-28 为准。旧配置不可直接作为当前生产配置。用户已撤回使用两个旧 PG 数据源／备份的方案，下面从旧库隔离副本核对身份的建议不再是执行入口。

用户已选择 Magpie 的本地 Qwen3.8 27B 作为 minor、DeepSeek Flash 作为 major，并授权其余按推荐处理。minor 的标准高思考版与 Instant 由业务需求选择：采用 **Instant、关闭思考**，适配路由／分类原 5 秒和记忆捕获原 3 秒期限；复杂任务沿用 major 升级路径。512K 是该服务的上下文配置，不提高 Alia 的上下文、输出、并发或期限预算。本节替代先前的模型待选清单；软件候选 `6123691516fae1c2b0e813652eaa1104c33da9af` 及归档保持独立。

选型阶段只读核对了 Magpie 0.1.1436／LM Studio 目录和资产。随后用户授权下载、独立配置与有限 Mac 组件预检，结果见下节；随后完成独立配置与合成数据库 profile 切换／恢复演练，见[升级恢复](upgrade-recovery.md#选定-embedding-的切换与隔离恢复2026-10-25)。没有切换生产配置或 SQL，没有接触 Pi／麦克风／播放。目录、组件、隔离数据与生产整链证据分别记录。

| 项目 | 已确认 | 历史候选／已盘点资产 | 下一轮待提供或核对 |
| --- | --- | --- | --- |
| 聊天与角色 | `minor` → `lmstudio/local/qwen3.8-27b-512k-instant`；`major` → `deepseek/deepseek-flash`；`default` 绑定同一个 major profile；省略 `memory_capture` 使用 minor fallback，省略 `sentence_expression` 使用确定性规则 | 两个路由均在 Magpie 目录；Instant 为 MLX 4bit，已存在的实例报告 context 524288／parallel 1／默认思考 off。DeepSeek 目录标签为 V4.1 Flash 1017 | 共用 `http://127.0.0.1:3425/v1`、`chat_completions`；两 profile 的 `provider_specific.reasoning_effort="none"`，保留原输出预算。本轮已通过有限 JSON／native tool stream／本地取消检查与原期限；下一轮扩大到实际业务和 Execution 整链。供应商 alias 与目录标签不能代替不可变权重版本 |
| Embedding | **本地 Qwen3-Embedding-0.6B、GGUF Q8_0、1024 维**，复用 LM Studio `http://localhost:1234/v1/embeddings`，不增加 Alia adapter 或模型框架 | LM Studio 内置 Nomic v1.5 Q4_K_M／768 权重存在、完整 SHA 已核对、目录显示未加载；Qwen embedding 已按固定 revision 下载、校验并导入，实际 ID 为 `text-embedding-qwen3-embedding-0.6b`。旧 OpenAI／1536 和 Nomic／768 配置不作为本轮有效选择 | 1024 维、2 次 HTTP 与 1 次 Rust adapter 请求已通过，中文小样本 top-1 为 3/3；真实记忆语料与并发负载仍未验。生产前核对 SQL active profile，保留 canonical memory，显式切换并重建投影。云端 `text-embedding-v4/1024` 仅为实测不满足需求时的备选，不自动切换或混用向量 |
| Easy-Turn | **复用项目内固定 CPU 栈**：`services/easy-turn/.venv`、`run/models/easy-turn-source`、`run/models/easy-turn/checkpoint.pt`、`run/models/qwen2.5-0.5b-instruct`；生产候选 loopback 8091、本轮隔离端口 18091、1 worker、并发 1 | source `fe175eaa…` 干净；66 项包版本元数据匹配；checkpoint 及 Qwen 所需 7 文件完整 SHA 匹配。外部 `Developer/Easy-Turn` 副本有修改／环境差异，不选用 | 独立 CPU 启动及三份官方 PCM 经 Rust adapter 已通过原 5 秒期限；真人语音和并发负载仍待验。MPS 不是当前固定栈已通过路径 |
| GPT-SoVITS／Juno | **Alia 独占 loopback 实例**，CPU／FP32／v2ProPlus；复用 `Juno-e30.ckpt`＋`v2Pro/Juno_e16_s80.pth` 及 BERT／HuBERT／SV 基础资产 | 两权重 SHA 分别为 `b8830efd…a01c94`、`050dba36…76d`；现有 Python 3.10.17、torch 2.12.0、torchaudio 2.11.0 的导入／算子／两次实际合成通过；最终参考为 `我一直很好奇我们的基地怎么会和星际旅程失联.wav`，4.4787 秒／32kHz／mono／PCM16，文本按用户确认直接取文件名 | 独立 `127.0.0.1:19880/tts` 已完成两次仅落盘合成，权重路径与前后 SHA 固定；下一轮做音色／可懂度和真实播放验收。保留依赖与训练键警告，不与其他客户端共享可切权重实例 |
| 独立 Whisper | **首版 VoiceSession 不启用**；直接使用 Easy-Turn transcript | 既有 whisper.cpp／large-v3-turbo 资产保留 | 以后确有独立 HTTP 音频转写需求时单列启用与验收，不给当前 VoiceSession 增加重复 STT |
| 调用与素材范围 | 先有限组件检查，再生产整链；默认 Companion shadow 保持；既有离线 PCM 和 R1 硬件基线复用 | Magpie 已配置所选上游，LM Studio 目录认证可用；凭据仅在内存／子进程环境中用于本机接口请求，未输出或归档原值 | 本轮有限启动和固定非敏感样本调用已获授权并完成，自建实例已停止。后续生产窗口另按当前授权进入；major 的必要上下文仍会送往 DeepSeek，本地 embedding 不代表全部记忆／会话内容都只留本机 |

当前选择与只读证据在 [selected-plan.json](../../run/model-plan-20261025/selected-plan.json)、[Magpie 目录](../../run/model-plan-20261025/selected-chat-catalog.json)、[LM Studio 目录](../../run/model-plan-20261025/lmstudio-models-readonly.json)、[Instant 资产](../../run/model-plan-20261025/instant-assets.json)、[Nomic 资产](../../run/model-plan-20261025/nomic-bundled-asset.json)、[Easy-Turn 资产](../../run/model-plan-20261025/easy-turn-readonly-inventory.json)和[Juno 资产](../../run/model-plan-20261025/gpt-sovits-juno-assets.json)。先前推荐、[历史交接](../refactor/handoffs/2026-10-17-model-preflight/acceptance-HANDOFF.md)及冲突配置保留为历史，不覆盖本次决定。

Instant 的本地 hub 清单实际指向 `orcarouter/Qwen3.8-27B-Uncensored-MLX-512K-Text`，不是标准 `qwen/qwen3.8-27b` 仅切换思考参数。已记录 12 个文件的 SHA，包括约 16.05 GB 的三个权重分片、config／tokenizer／chat template 与 hub 映射；没有可据以认定的上游 revision。当前固定的是磁盘资产，不能证明已驻留实例仍使用相同字节。本轮已验各一条路由／记忆 JSON 样本，下一轮仍须扩大业务语料并验真实工具链；Execution／授权边界保持原样。

本地 embedding 选择依据是中文／多语言定位、较小的共存开销和现有接口适配，不是 Alia 实测优于云端。[Qwen 官方模型卡](https://huggingface.co/Qwen/Qwen3-Embedding-0.6B-GGUF)列出 0.6B／1024 维和 Q8_0；已固定 revision `370f27d7550e0def9b39c1f16d3fbaa13aa67728`、文件 `Qwen3-Embedding-0.6B-Q8_0.gguf`、SHA-256 `06507c7b42688469c4e7298b0a1e16deff06caf291cf0a5b278c308249c3e439`。Alia 目前不区分 query/document 指令，先按现有 plain input 验中文检索，不为选型增加 reranker 或协议。[Nomic 官方模型卡](https://huggingface.co/nomic-ai/nomic-embed-text-v1.5)的英语定位和任务前缀要求与当前契约不同，保留其可复用资产，不据此断言中文不可用。

[Magpie 接入文档](https://usemagpie.ai/docs/integrate)支持现有 OpenAI-compatible 聊天入口；本机 `magpie-alia` 只是调用方标识，不是供应商密钥。使用完整 provider/model 路由，避免 `auto/` 或裸模型名带来上游歧义。Embedding 直接走 [LM Studio embeddings](https://lmstudio.ai/docs/developer/openai-compat/embeddings)，不能由 Magpie 聊天目录推定其转发 embeddings。LM Studio 的真实 credential 留在受控来源。本轮独立配置将该适配器命名为 `openai` 并显式指向本地 LM Studio，以复用现有 `OPENAI_API_KEY` 补空机制；运行器仅在子进程环境填入现有 token，不写入配置或日志。其他命名 provider 不会自动读取任意同名环境变量。

已生成[独立组件配置](../../run/model-preflight-20261025/config/server.components.toml)，由当前 Rust `ServerConfig` 和 adapter factory 实际加载通过，启用真实模型但没有 channels／discovery／后台计划任务；旧可选角色绑定未带入。配置目录没有 `.d` 或 `settings.toml`，子进程清除继承的 ALIA／数据库／模型密钥覆盖，仅注入本地 embedding token。数据库故意指向 loopback:1，**本文件用于直接组件检查，不能启动生产 Server**；未读取或变更生产 SQL policy。先前[选型片段](../../run/model-plan-20261025/model-sections.toml)保留为历史。生产组装仍须核对 base／`.d`／环境／settings／SQL 的有效值，省略可选角色不会删除深合并中的旧值。

当前资产、参考文本、有限组件预检和独立生产候选配置已具备；SQL 切换／Memory 投影重建与前后数据还原已在合成隔离库验证。待部署配置仍关闭入口，真实生产数据库、账号／设备／网络身份和服务账户托管尚须核对，不能直接启动。下一步填实这些绑定、取得一致备份并安排现场窗口，然后在 Mac Server＋Pi Embedded 上集中验收：**生产启动 → 讲话 → 模型处理 → 回复播放 → 打断／恢复 → 取消与下一轮对话**。该轮包含正常换气长句、系统 Start／Stop、生产单次初始化后的真人语音；复用已通过的插拔、方向拾音、试听和静默恢复基线，不把 fixture 音频算成真人语音通过。

实际部署后另做升级／恢复及至少 24 小时混合负载，覆盖断网、模型超时、进程退出、设备重连、内存／线程／句柄／队列与日志磁盘上界。验收前固定记录上界与观察方式，不以放宽原期限或并发求绿。AEC 等商家确认后独立验证；具体首版 MCP 服务和第三方插件按实际目标验证，通用框架回归不代替它们。V01／V02 历史根因继续开放，扩大无人值守主动运行前再处理对应条件；ROS、执行器现场、固件重写和其他平台不因模型接入自动恢复。

## Mac 有限组件预检（2026-10-25）

源码基线 `0238620`（软件实现仍为 `6123691`）；使用临时 Rust 诊断调用当前 `LlmService`／Rig、`OpenAiEmbedding`、`EasyTurnDetector`，没有启动 Alia Server、Execution 工具或数据库。临时 example 已移除，诊断源码／二进制摘要和准确提示词保存在 [rust 证据](../../run/model-preflight-20261025/rust/diagnostic-build.json)。组件原始响应、请求预算、独立配置、身份与清理见 [summary.json](../../run/model-preflight-20261025/summary.json)。

| 检查 | 本轮结果与实际耗时 | 保留的期限与边界 |
| --- | --- | --- |
| minor 路由分类／记忆捕获，Rust `LlmService` | 各 1 次通过；4.175 秒／1.040 秒，合法 JSON，问候路由正确、瞬时对话未产生记忆候选 | 原 5 秒／3 秒；路由只剩约 0.825 秒余量，不能外推长上下文、忙时或稳定 p95 |
| major 原生工具流／本地取消，Rust＋Rig | 各 1 次通过；0.627 秒完整得到正确工具参数／0.384 秒收到首个文本后停止本地读取 | 原 60 秒；没有执行工具，不证明远端推理已停止，也不代替真实 Execution 整链 |
| embedding | HTTP 两次通过（0.231 秒／0.017 秒），另一次生产 Rust adapter 通过；全部 1024 维、有限数值、模型身份与序号匹配；固定中文小样本 top-1 为 3/3 | 原 30 秒；单样本/小语料，不是生产检索质量、SQL profile 切换或重建验收 |
| Easy-Turn CPU＋Rust adapter | 独立启动 10.11 秒；Complete／Backchannel／WAIT 三份官方 PCM 各通过，1.961／0.380／0.440 秒 | Alia 原 5000ms、并发 1；服务原生 worker 上限仍为 60 秒，不把它冒充客户端期限；未测真人或全部语义类别 |
| Juno CPU／FP32，直接 HTTP 合成 | 独立启动 8.10 秒；两份输出分别 6.472／1.223 秒，首个响应体 6.346／0.584 秒；有效 32k mono PCM16、非静音、无满幅裁剪 | 原总 60 秒／流空闲 10 秒；仅保存 WAV，没有播放、试听或音色／可懂度判断；未通过 Alia TTS adapter 发起 |

Qwen embedding 文件为 639,150,592 bytes，SHA 与所选 revision 一致；LM Studio 通过符号链接导入，进程映射到固定 GGUF，实际 model ID 已写入配置。原件位于 `run/models/qwen3-embedding-0.6b-gguf-370f27d/`，清理运行证据时须保留这个模型目录。Juno runtime 的 torch／torchaudio 导入、重采样和 MelSpectrogram 通过，没有安装或升级依赖。

保留的失败与警告：诊断程序初版有一处 Rust enum 引用类型错误，在任何模型调用前修复；第一份独立配置缺少必需 Insights 字段，被 serde 拒绝，原配置和失败仍在，补齐现有字段且关闭后台任务后 v2 加载通过。上述均不是生产模型请求失败。Juno 原有 RequestsDependencyWarning 和 103 个 `enc_q.*` 缺失键保留，unexpected keys 为空；源码中这些键属于训练 posterior encoder，当前 infer/decode 不使用，没有改加载行为或隐藏警告。Easy-Turn 的可选 NPU 导入提示也保留，不为 CPU 预检安装 NPU 依赖。

参考素材遵循用户最新说明，文件名就是文本。先前选中的 `audio (3).wav` 已被替代；用户纠正前已完成一次本地 Whisper 文件转写，原始记录保留且不用于最终配置，之后没有重跑。最初读取其完整 token JSON 遇到无效 UTF-8 的解析错误也保留，不当作 Whisper 推理失败或参考文本依据。首版 VoiceSession 仍没有独立 Whisper。

所有调用使用非敏感合成文本或既有离线音频，推理阶段顺序执行；同机模型驻留与资源快照不等于并发负载或资源上界验收。本轮自建 Easy-Turn 18091、Juno 19880 已停止，仅卸载本轮新增 embedding 实例；原 Magpie／Instant 和 9880 上既有 TTS 保留。没有麦克风、播放、UART、Pi 部署或生产数据库写入。V01／V02、Companion shadow、AEC 和长稳门槛不因这些组件结果改变。

## 生产配置与隔离 SQL 出口（2026-10-25）

[待部署单文件配置](../../run/production-config-20261025/config/server.production.pending.toml)和[隔离配置](../../run/production-config-20261025/config/server.isolated.toml)经实际 Rust loader、SettingsRegistry 和 factory／profile preview 通过。采用已确认模型／原期限，关闭未验的后台计划、Insights 与自动 motion，Companion 仍以 SQL shadow 核对为准；无具体首版插件接入。配置不是已部署状态，数据库 port 1、空 JWT 和 disabled channel 保留到真实绑定落实。

本轮复现旧 settings 会把本地 Qwen／1024 覆盖成 OpenAI／1536，因此新目录不继承冲突层。选定 profile 为 `alia.embedding.d07c6780f61ebf58ba106d7e1cee0a5cfd4405ba6e2c4c62f04690811978aa19`，endpoint 精确为 `http://localhost:1234/v1`。正常 Settings 保存会落盘环境注入的 token；生产 `settings.toml` 必须作为私有凭据文件管理，不能因主 TOML 无 key 就视作无 secret。

[隔离结果](../../run/production-config-20261025/summary.json)覆盖新配置不覆盖旧 SQL pin、stale CAS 409、合法 profile 切换、3 条合成记忆重建、同维 endpoint 身份隔离与显式回切、缺失投影重启修复，以及切换前后各 67 表的实际备份／还原核对。旧 1536 维为 fixture，新 1024 维调用固定本地模型；canonical 内容不变，private_runtime 无投影。Cache 没有旧 active 数据的自动 backfill，不能合称所有向量重建完成。

操作 payload、只读 SQL、原期限、部分成功判定和前向回切／成套恢复边界集中在[升级恢复入口](upgrade-recovery.md#选定-embedding-的切换与隔离恢复2026-10-25)。旧配置指向的 localhost:5432 当前拒绝连接；本轮未读生产 SQL，也没有证明真实账号／绑定、Companion 状态或生产数据量。恢复副本仅只读核验，主隔离库验证重启补建；没有部署或启用音频。

进入真人窗口前还须落实：真实库和原身份／备份；Mac 服务账户及模型启动顺序（Magpie／LM Studio embedding → CPU Easy-Turn／独占 Juno → Server）；Pi 可达 endpoint 与设备 pin／owner；随后成套部署并验实际 Alia TTS adapter、讲话／模型／播放／打断恢复／取消下一轮、换气长句和 Start／Stop。此前 Juno 只验直接 HTTP。实际升级恢复、24 小时混合负载、AEC 和具体插件仍保留独立门槛。

## Mac 常驻材料与本机盘点补充（2026-10-25）

只读核查确认现有模型资产与服务归当前 `ke-qiao` 用户。当前 Magpie／LM Studio 可达、Instant 已加载；embedding 未加载，独占 Juno 19880 和 Easy-Turn 8091 尚未运行。现有共享 Juno 9880 保留。已准备用户 LaunchAgent、独立 Juno 运行 YAML、无推理／无加载的就绪检查和 Server 启动前检查；本轮没有安装或启动这些材料。目录／健康检查不替代真实调用、权重驻留字节或生产整链验收。

详见[服务材料](../../run/mac-production-readiness-20261025/services/README.md)和[本机数据／常驻方式](server-embedded.md#本机数据与常驻准备盘点2026-10-25)。首轮建议当前用户登录会话：模型先就绪，Server 显式启动且不自动重试；无人登录开机方案尚未实施。LM Studio 敏感请求日志当前开启，关闭日志和显式预加载的候选设置只准备、未应用。两套既有 PG 都停止，Rust 卷映射5433、历史Python卷5432；生产账号与SQL身份应从停服数据的隔离副本核对，不新建空库绕过。

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
