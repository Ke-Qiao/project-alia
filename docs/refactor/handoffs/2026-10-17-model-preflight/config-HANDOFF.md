# 真实模型链前置：配置契约 HANDOFF

日期：2026-10-17。负责人：`v01_fixture`。基线 `6a8b113`，应用源码 `6c62c84ee60e3a5fd387f2172ead432031907599`。初审仅核对已跟踪示例、文档、配置加载和 provider 实现；MP10 用户答复后，按 Root 转达的明确授权，有界读取项目内旧私有配置，并只输出白名单内的非敏感角色、模型、无认证 URL 和路径信息。没有输出／归档凭据值、完整配置或环境变量；未探测端点、启动服务、运行 Cargo、下载依赖或启用录放音／硬件。生产与私有配置均未改，用户集中确认由 Root 发出。

## Checklist

- [x] 核对 LLM roles → model profiles → named provider profiles 的实际绑定与支持范围。
- [x] 核对 embedding 的端点、模型、维度、请求身份与持久 SQL 政策边界。
- [x] 核对 whisper.cpp 和 GPT-SoVITS 的路径、认证支持、真实模型身份及初始化副作用。
- [x] 核对 Easy-Turn 固定资产、服务契约、CPU／MPS 证据边界。
- [x] 核对 base／fragments／env／persisted settings／SQL 政策的实际优先级。
- [x] 向 Root 提交集中确认要点、已存在文档与确认缺口；只读审计 6/6。

这里的 6/6 仅代表前置审计完成，真实模型验收没有开始。MP10 已答复，用户已明确待定的部分保留待定，不重复追问；不能以旧配置或示例值填为已选新方案。默认 shadow、当前政策和原有期限／并发不变，R03 暂停。

## 集中确认要点

以下为初审时交给 Root 的问题依据；MP10 的答复状态见下一节，不是要求用户重新回答全部项目。用户可给现有非敏感部署资料路径；缺失项保留待定，无需重复确认已决定的“Server 在当前 Mac、Easy-Turn 在 Server、Pi 不跑该模型”。

| 项目 | 必须确认的信息 | 原因／当前实现限制 |
| --- | --- | --- |
| 聊天 LLM | major、minor 具体模型 ID／版本；default 选择 major 或 minor；memory_capture 是否使用专用 profile，sentence_expression 是否启用专用模型；各 provider 的运行位置、完整 base URL、API family、认证注入方式 | 支持 OpenAI chat_completions／responses、Anthropic、Gemini。role 指向模型 profile，profile 再指向 named provider；不是把 role 直接填写为供应商名。default 当前复用 minor 或 major，不是独立第三实例 |
| Embedding | 独立 provider／完整 base URL、精确模型 ID、实际向量维度、部署版本／本地权重标识和认证来源 | 真实 adapter 只支持 OpenAI 兼容 `/embeddings`；同名模型换权重不能由 Alia 请求配置身份自动证明 |
| whisper.cpp | 服务版本／启动模型及权重版本、服务基址、已准备输入样本的来源与语言 | 程序追加 `/inference`、`/health`；`model` 配置只作资料标签，不能切换服务端已加载权重。设备 VoiceSession 使用 Easy-Turn 的 transcript，不额外二次 STT |
| GPT-SoVITS | 服务版本、完整 `/tts` URL；默认 voice；远端可见的参考音频路径、参考文本／语言、目标语言；可选 GPT／SoVITS 权重路径及版本；服务是专用可切权重实例还是预加载共享实例 | 路径属于 TTS 服务所在主机，非 Alia Server 本地；非空权重路径会在 init 调用远端切权重接口，失败只告警并继续已有模型，必须核对实际加载模型 |
| Easy-Turn | 是否沿用仓库固定 source／checkpoint／Qwen 和 Python 3.10 CPU 方案；三个既有资产目录、隔离环境路径、loopback 服务地址／端口 | 当前锁 CPU／auto 有历史组件证据；MPS 不支持，不能据“Metal 可用”转为 MPS 验收。服务需现有资产，不应在此次前置阶段自动下载 |
| 配置与认证 | Mac 服务账户／独立部署目录、base 文件路径、同名 `.d` fragments、是否沿用 `settings.toml`；认证方式和注入变量名或受控文件路径，不提供值 | persisted settings 可覆盖 provider 的已注册运行时值；默认命名 LLM 的 env 仅补空 API key，自定义 provider 不自动吃同一变量。STT／TTS／Easy-Turn 当前没有自定义 auth header 配置 |
| 实际验收范围 | 可使用哪些已有非敏感文本／音频样本；是否只使用本机既有服务，哪些远端／付费调用允许；有限请求集合及其原有预算 | 本轮只收集前置，不等于获准启用任何外部付费探测、切共享权重、录音或播放。真实请求时仍不得用健康状态替代模型输出证据 |

STT／TTS 若位于需额外认证的服务后面，必须明确现有可兼容接入方式；不能把密码塞进 URL 作为现成方案。Easy-Turn adapter 明确拒绝 URL username／password／query／fragment，禁止 redirect，并使用 `no_proxy()`，没有通用 API key 配置。当前服务 CLI 仅接受 loopback IP。

## MP10 用户答复与旧配置有界核对

用户已答复：LLM／embedding 待定，准备以 `https://botcf.com/v1` 测试；Easy-Turn 在本机 Developer 文件夹，GPT-SoVITS 也在 Developer、权重名 Juno；Whisper 在项目中，允许参考以前配置。部署目标是当前开发验证，不要求严肃的独立账户，必要时可使用容器。此答复确立候选及资料查找范围，不证明 BotCF 支持哪些模型／API family，也不代表已连接、已授权无限请求、已加载权重或已创建容器。本 agent 未访问该 URL 或任何模型端点。

已获授权的读取范围仅为项目 `config/server.toml`、递归排序的 `config/server.d` 27 个 TOML、`config/settings.toml`、相关已跟踪示例／脚本，以及重构前 `3530395` 的配置文件路径列表。提取时仅输出允许的非敏感字段；URL 去掉 userinfo／query／fragment，不输出完整配置、API key、token、密码或环境内容。未修改这些私有文件，未将其复制到证据归档。

| 读取来源 | 可恢复的旧值 | 不可据此认定的事情 |
| --- | --- | --- |
| `config/server.toml` + `server.d/profiles/llm.toml` 等排序片段 | major／minor 均为 `deepseek-v4-flash`，provider=`deepseek`、OpenAI `chat_completions`、base=`https://api.deepseek.com`；default=major | 不代表下一轮已选择 DeepSeek，也不代表 BotCF 上存在同名模型 |
| 同一 base／fragments | sentence_expression=`gemini-2.5-flash`；embedding=`text-embedding-3-small`、OpenAI、1536 维；memory_capture profile 存在但未绑定专用 role | 不代表这些外部服务当前可达或允许计费；未启用新政策 |
| `server.d/providers/whisper_cpp.toml` | endpoint=`http://localhost:8080`，model 标签=`large-v3`，期限 30 秒 | 不证明实际 whisper-server 版本、运行位置或已加载该权重 |
| `server.d/providers/gpt_sovits.toml` + `profiles/voice.default.toml` | endpoint=`http://localhost:9880/tts`，voice=`alia-default`，语言 zh；reference_voice、GPT／SoVITS 权重路径为空 | 没有恢复出 Juno 的实际文件路径／hash，也不证明远端当前加载 Juno |
| 当前 base／fragments | 未发现 Easy-Turn provider 配置块；现实现缺省 disabled | 不是禁止后续启用，只说明不能把旧主配置当作已连通 Easy-Turn 的证据 |
| `config/settings.toml` | 保留另一套旧方案：embedding role=`local_embedding`、Nomic `text-embedding-nomic-embed-text-v1.5`／768 维、旧私网本地服务；stt／tts desired=noop；minor model=`gemini-2.5-flash`；major provider desired=`local_openai`，其旧 endpoint 指向 DeepSeek | 这些 settings 不能与 base 混拼成“当前已生效方案”。role／实现选择有启动边界，实际 adapter 从 base 构造，部分运行时 model／endpoint 才会后覆盖。不得以旧 desired selection 覆盖当前政策或悄悄启用服务 |

旧配置还保留 `affect_inference`、`initiative_opening` 和 transcript_correction 等历史角色／profile／policy 项；当前已迁移的功能不能仅因旧文件里有同名 key 就被恢复。尤其 base 的 minor 绑定 DeepSeek，而 settings 的 minor model 字符串仍为 Gemini，表明旧文件混用可能产生 provider／model 错配。只记录该风险，不启动 Server 来“试试看”，不自动迁移、删除或覆盖私有设置。

有界历史／路径核对：`3530395` 的 Git tree 只包含示例配置，没有真实 `server.toml`、`server.d` 或 `settings.toml`，故该提交无法恢复实际私有 Juno 路径。`scripts/setup-whisper-cpp-macos.sh:5` 的默认安装位置是 `$HOME/.local/share/alia/whisper.cpp`，支持 `WHISPER_CPP_DIR`／`--prefix` 覆盖；脚本存在不证明使用过默认位置。本 agent 未运行脚本，也未向用户目录外追查。项目 `run/models` 与 `services/easy-turn/.venv` 目录存在，仅记录路径存在，未扫描实际模型内容、重新计算权重或导入 Python 环境。Developer 下模型部署位置由 Root 的对应子任务继续核对，避免重复和扩大搜索。

后续配置应由当前受支持的结构明确重建，并逐项选择保留哪些已确认旧值；LLM／embedding 的精确型号继续待定，不再追问。实际端点认证方式、Juno 资产身份、Whisper 具体构建和 Easy-Turn 目标目录尚不能由这些旧文件确定。开发验证可以沿用开发账户；容器可用是部署选项，尚未新建容器。

## LLM 与 embedding 已核对契约

入口：`packages/alia-server/src/config.rs:193`、`:247`、`:1088`；工厂：`bootstrap/factory.rs:50`、`:157`；启动角色：`bootstrap/builder.rs:340`、`:510`。

- `[llm.roles]` 绑定 `minor`、`major`、`default`、`embedding`，可选 `memory_capture` 和 `sentence_expression`。`profiles.llm.<id>` 有 `provider`、`model`、`kind=chat|embedding` 和请求参数；`providers.llm.<id>` 选择 OpenAI／Anthropic／Gemini／noop。同一 profile 的聊天角色复用已初始化实例。
- `builder.rs:556` 实际执行 `default == minor ? minor : major`。配置角色 validator 验证类型但没有限制 default 必须等于两者之一，因此验收清单应明确 default 选 major／minor，不能宣称任意第三 profile 已实现。此为既有实现边界，本轮未扩张修复。
- `memory_capture` 省略时使用既有 minor fallback，sentence_expression 省略时使用确定性表达规则；保留当前政策，不为模型验收改变自动记忆或执行批准。示例的专用 profile 不是部署已选定的证明。
- OpenAI 使用 Bearer 认证并按 API family 选择 chat completions／responses；Anthropic 使用 `x-api-key` 和 `anthropic-version`；Gemini 使用 `x-goog-api-key`。没有通用任意 header 配置。必须确认实际网关协议与这些 adapter 兼容，不能仅凭“兼容 OpenAI”标签推定全部可用。
- embedding 工厂只接受 OpenAI 或 noop。`providers/embedding/openai.rs:92` 的不可变 descriptor 绑定 provider ID、规范化 endpoint、model、维度、cosine/vector 和固定 `revision="request-config.v1"`；`providers/adapter.rs:729` 明确 revision 是描述符版本，**不是远端权重版本证明**。实际服务构建／权重身份需另记。
- embedding 请求只在模型名以 `text-embedding-3` 开头时发送 dimensions 参数（`openai.rs:244`）；其他模型必须本身返回配置的实际维度，不能假定本地 `dimensions` 会让任意网关降维。响应 count／index／有限非零向量及维度均验证；有返回 model 字段时还要求和请求一致。
- Memory／cache 的 active embedding profile 由 SQL 政策约束。现有库与新配置身份不一致时不得为“验收通过”隐式重写政策；代码会保留 canonical keyword 路径并阻止不匹配的 semantic 使用（`builder.rs:486`）。默认 LLM health probe=false，常规 health 不等于真实模型推理通过。

## STT／TTS 已核对契约

STT：`config.rs:569`、`providers/stt/whisper.rs:84`、`:118`。

- 默认 provider 为 whisper.cpp 兼容 HTTP，配置基址后直接追加 `/inference`，multipart file + language + verbose_json + token_timestamps；PCM16kMono 在上传端加 WAV 头。不要把 `/inference` 再写进 base endpoint。
- 默认请求期限 30 秒。语言初始为 auto，运行时 key `core.providers.whisper_cpp.language` 可覆盖；provider struct 中的 model 不进入 `WhisperConfig` 或 inference payload。`config/server.example.d/providers/whisper_cpp.toml` 已正确写明 informational。
- `/health` 是服务健康检查，不是转写质量或实际权重身份验证。设备连续 VoiceSession 已使用 Easy-Turn 的 transcript；独立 STT 主要用于显式音频转写 API，不能重复计算同一设备语音。

TTS：`config.rs:596`、`providers/tts/gpt_sovits.rs:143`、`:249`、`:728`、`:797`。

- endpoint 是完整 `/tts` 地址；default_voice 指向 voice profile。neutral `reference_voice` 映射 `ref_audio_path`，`style_prompt` 映射 `prompt_text`，`language` 是目标文本语言；provider_specific 存 prompt_language、GPT／SoVITS 权重和额外参考音频等。这些远端路径必须由部署者确认可见性和具体资产。
- 默认 request timeout=60 秒、stream idle timeout=10 秒，sample-rate pitch fallback=false；本轮不改期限或开启该 fallback。后续应验证生成的真实 WAV／stream，无需送到扬声器。
- 当默认 profile 配置非空权重路径，init 会对去掉 `/tts` 后的服务 GET `set_gpt_weights`／`set_sovits_weights`，带 weights_path 查询。init 对失败告警后继续使用服务当前模型，故 init／health 成功均不能证明指定权重加载成功。运行时设置也可触发权重配置，独立部署或共享服务是否允许切换必须先明确。
- TTS health GET `/tts`，2xx 或 400 都可代表健康；它验证接入，不验证合成效果。STT/TTS adapter 未提供 API key／自定义 auth headers。

## Easy-Turn 已核对契约与历史证据

来源：`services/easy-turn/README.md`、`VALIDATION.md`、`packages/alia-server/src/adapters/easy_turn.rs:25`。

- 固定 Easy-Turn source=`fe175eaa8442c9a0834acf29f8640ee74c2aceca`，checkpoint revision=`7d1f8b06776b575ac22e2fd4aa7913da2f4b8b98`，Qwen revision=`7ae557604adf67be50417f59c2c2f167def9a775`；checkpoint 3451801684 bytes、SHA-256=`1cb535bb0ffe3ffa0ac941bae4898aeb87adef7b88bb58f665af08eded6f6070`。本轮只读此既有记录，没有重新访问／哈希实际权重。
- Python 3.10、Torch／Torchaudio 2.2.2 固定锁。既有 2026-09-08 CPU official sample 16/16 是历史组件证据，不是本轮部署确认；本锁 MPS 缺少所需 `torch.isin`，显式 MPS 会启动失败，auto 走 CPU，无静默 fallback。
- Server 默认 Easy-Turn disabled，启用时 adapter 追加 `/v1/detect`；默认 timeout_ms=5000、max_concurrency=1。服务单进程／单 model slot，原生调用超时不会释放 slot，必须等实际返回；不能通过重复请求绕过占用。
- 传输为 16 kHz、单声道、PCM s16le，无 WAV 头；服务接收 100 ms–30 s 有界窗口，返回 ID／epoch、transcript、四类状态、模型身份与 inference_ms。accepted transcript 直接走 recognized input，不是独立第二次 STT。服务 readiness、HTTP 成功、真实模型类别、CPU 内存／延迟与物理 AEC／扬声器延迟分别记录。
- 本轮没有新建服务、放宽 Easy-Turn 超时、切 MPS、安装依赖或重新下载模型。真实模型前置若采用历史路径，必须先获用户确认该路径仍是目标资产，后续再只读验证资产身份。

## 配置覆盖顺序与资料缺口

1. `config.rs:2216` 选择 base：`ALIA_CONFIG_PATH` → `./config/server.toml` → 用户 config dir → `/etc/alia/server.toml`。显式路径的同名 `.d` 目录递归收集 TOML 并排序（`:2245`）；table 深合并，非 table／array 整体由后者覆盖。路径以 canonical base 所在目录为基准，不以进程 CWD 为基准；GPT-SoVITS 远端资产路径是例外，按远端解释。
2. 合并并反序列化后，DATABASE_URL 和 ALIA_JWT_SECRET 无条件覆盖相应字段。OPENAI_API_KEY／ANTHROPIC_API_KEY／GEMINI_API_KEY 只补 **默认 provider ID openai／anthropic／gemini** 对应类型中空白的 api_key；配置非空 key 优先，自定义 provider（如 local_openai）不自动接收它们（`config.rs:2108`）。因此不能简化为“一切环境变量最高优先”。
3. `builder.rs:313` 从加载后的 config seed settings，再加载同配置目录 `settings.toml`；该文件覆盖 registry 中相同 key（`settings/registry.rs:274`），可包含 provider endpoint／model／key 等。adapter 先由 startup config 创建并初始化，之后才应用非 requires_restart 的 runtime settings（`builder.rs:887`）；所以 base provider 的初始化副作用和最后生效设置两者均需核对。Easy-Turn 在此之前由 startup config 构造，没有该 settings 注册入口。
4. SQL Context policy 在 producers 启动前覆盖旧 TOML policy 投影；embedding 也有提前 preview／apply 与 profile 审核流程。不能把完整 settings 直接当成最高优先的启动配置，不能通过复制旧文件绕过当前 SQL 政策／权限。
5. 当前 `GET /api/v1/settings` 返回敏感原值；本轮不得用完整响应或私有 settings 打印做审计。后续证据应保留配置文件身份、非敏感选项和凭据来源名称，避免归档 token／完整 settings。现有官方示例里的模型名和 loopback 端口只是示例，不是已确认部署事实。

已给 Root 的基线文档收口建议（Root 已接管下列资料更新，本 agent 不修改这些文件）：

- `config/server.example.toml:85` 仍有已移除的“Optional dedicated LLM for STT transcript correction”注释；实际 roles 无该字段，宜删除此旧说明。
- `docs/operations/model-services.md:36` 仍称启用 Embedded `[speech].enabled`；当前 VoiceSession 已迁移，Root 已确认本轮修正文档，不以恢复旧开关方式处理。
- `docs/operations/model-services.md` 已明确 Easy-Turn transcript 不二次 STT、TTS 远端路径；可复用，无需另建未选框架。其 LLM 章目前只概述 major／minor，缺 role/profile/provider 映射、default 复用边界、独立 embedding 模型／维度、真实认证限制。
- `packages/alia-server/docs/configuration.md` 已有 base/fragments 和 settings 恢复语义；可补精确 env 补空规则、persisted provider settings 后应用及 SQL 权威边界。
- GPT-SoVITS 切权重副作用／失败续用已有模型，以及 model label 不等于真实权重证明，宜进入统一验收资料。Easy-Turn README／VALIDATION 的固定版本与历史 CPU/MPS 边界已存在，引用即可，不能把历史结果升级为本轮真实模型链验收。

Root 集中三问已收到 MP10 答复并转达授权。实际配置／模型选择的已知与待定状态见上节；仅项目旧配置已按白名单核对，没有开展无关私密文件扫描。default 指向第三 profile 的行为只记录为后续需明确预期的既有边界，本轮没有生产修复或新的框架设计。

## 后续任务与暂停边界

- [x] Root 已收集 MP10 集中确认，本 HANDOFF 记录已知选择和明确待定项；不记录凭据值、不重复询问已答复项。
- [ ] 按确认的模型服务和部署目录制作具体可审阅配置，核对全部覆盖层与初始化副作用；目前未生成真实配置。
- [ ] 只有确认后的有限真实请求，才验证输出身份／格式／取消／资源收尾并记录成本、样本和时延；未获确认的端点保持不访问。
- [ ] 真实 Pi 音频、R1 AEC 参考接法／电气规格、功放扬声器选型、执行器 R03 和整机 R05 保留原暂停／待办，不从本轮资料收口推导硬件授权。
