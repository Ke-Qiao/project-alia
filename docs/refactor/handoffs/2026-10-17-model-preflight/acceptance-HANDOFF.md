# Model preflight — Acceptance

日期：2026-10-17。任务基线 `6a8b113`，软件源码基线 `6c62c84`。本分工只读核对仓库内服务指南、provider请求代码、既有真实模型测试和历史原件；未探测端点、启动服务、下载、运行Cargo、使用麦克风／扬声器／执行器或修改源码。前置梳理 **6/6** 完成；本轮真实模型验收 **未执行**。R03保持暂停。

## 前置Checklist

- [x] 找到可复用的真实Easy-Turn adapter、gRPC＋PG测试及官方样本smoke入口。
- [x] 核对Easy-Turn源码／模型／基础模型钉住、CPU路线及历史失败和成功范围。
- [x] 区分合成PCM传输证据、官方有语义样本和用户授权离线录音。
- [x] 核对LLM、embedding、TTS及可选Whisper的请求／认证／音色资料需求。
- [x] 明确health与实际infer的差异、既有期限／并发、取消与原件保留要求。
- [x] 将集中问题和分阶段前置步骤交Root；本轮不擅自执行实模测试。

## 必须集中确认的资料

1. **服务清单与身份**：当前开发Mac上Easy-Turn、major/minor LLM、GPT-SoVITS的部署目录、实际端点、服务版本或commit、模型ID／修订／本地权重身份；embedding和Whisper是否属于首轮范围。认证只提供受控配置路径或环境变量名，不把密钥写入聊天、原件或HANDOFF。仓库示例URL和模型名不是已部署事实。
2. **TTS音色与启动副作用**：选定voice profile、服务端参考音频路径、参考文本／语言、GPT与SoVITS权重路径／版本；这些路径属于TTS服务主机。确认使用既有已加载权重，还是允许Server初始化调用已有权重切换接口；未确认不启动会切换共享服务状态的配置。确认参考音频可用于本轮离线合成。
3. **输入与调用范围**：是否沿用钉住官方Complete／Backchannel／WAIT PCM及合成短文本，是否有允许用于模型输入的额外离线PCM；输出仅保存私有文件，不播放。若选外部收费或云端服务，先明确可发送的合成输入与有界请求范围。设备VoiceSession直接用Easy-Turn transcript，不需要重复调用Whisper；仅当独立HTTP转写入口也列入验收时才准备Whisper资料。

尚未确认的服务、版本、端点与认证保持待定；本轮不会从示例配置推断或尝试连接。用户此前“可用Mac模拟”授权覆盖软件离线模拟，不自动证明任何真实模型服务已经部署，也不等于允许访问任意云端服务。

## 用户答复后的限定本地盘点

用户指定Easy-Turn／GPT-SoVITS位于Developer目录，GPT音色为Juno，Whisper参考项目既有配置。本分工随后只列Developer顶层名称，限定读取对应模型目录、项目语音配置和安装脚本派生的Whisper目录；提取git HEAD、指定字段及stat，不读取密钥文件／API key，不执行模型Python、二进制、安装脚本、服务或网络请求。开发验证不以独立生产账户为前提；未因此建立账户、启用容器或新增部署范围。

| 对象 | 实际只读发现 | 判定限制 |
|---|---|---|
| Developer Easy-Turn源码 | `/Users/ke-qiao/Developer/Easy-Turn` HEAD=`fe175eaa8442c9a0834acf29f8640ee74c2aceca` | 与钉住源码一致；未检查dirty工作树，不等于整个目录字节已冻结。 |
| 项目既有Easy-Turn源码 | `/Users/ke-qiao/Developer/project-alia/run/models/easy-turn-source` HEAD同上 | 可复用既有服务路径，未启动。 |
| Easy-Turn checkpoint | `/Users/ke-qiao/Developer/project-alia/run/models/easy-turn/checkpoint.pt`，3,451,801,684字节；mtime=`2026-09-07T18:10:00.245359Z` | 仅stat与已知字节数相符，本轮没有重做3.45GB哈希、torch加载或推理。 |
| Qwen manifest | `run/models/qwen2.5-0.5b-instruct/alia-artifacts.json`存在，758字节，revision=`7ae557604adf67be50417f59c2c2f167def9a775` | 只读取小manifest身份，未重新hash大权重，也未据此认证全部文件内容。 |
| GPT-SoVITS源码 | `/Users/ke-qiao/Developer/GPT-SoVITS` HEAD=`08d627c3338173c3229286d8787060d6559fe0f8` | git版本是磁盘checkout身份；未认证工作树完全未改或当前服务loaded版本。 |
| GPT配置所指Juno GPT权重 | `/Users/ke-qiao/Developer/GPT-SoVITS/GPT_SoVITS/pretrained_models/Juno-e30.ckpt`，155,312,594字节；mtime=`2026-04-21T19:14:35Z` | 普通文件存在，无大文件hash／载入。 |
| GPT配置所指Juno SoVITS权重 | `/Users/ke-qiao/Developer/GPT-SoVITS/GPT_SoVITS/pretrained_models/v2Pro/Juno_e16_s80.pth`，172,764,966字节；mtime=`2026-04-21T19:00:28Z` | 同上。 |
| Whisper checkout | `/Users/ke-qiao/.local/share/alia/whisper.cpp` HEAD=`968eebe77225d25e57a3f981da7c696310f0e881` | 由项目`setup-whisper-cpp-macos.sh`默认PREFIX定位；不是声称在项目内找到checkout。 |
| Whisper已构建入口 | 上述目录`build/bin/whisper-server`，1,230,256字节；mtime=`2026-05-16T15:49:23.945313Z` | 只stat，未执行`--version`或启动。 |
| Whisper模型文件 | 上述目录`models/ggml-large-v3-turbo.bin`，1,624,555,275字节；mtime=`2026-05-16T15:50:26.930953Z` | 未hash／加载；与项目说明字段`large-v3`不同，实际模型需以后按启动参数核对。 |

Juno选择按用户指定，不额外推定其他音色或checkpoint。已有`GPT_SoVITS/configs/tts_infer.yaml`的`custom`及`v2ProPlus`段明确指向上表两份Juno文件，device=`cpu`、is_half=`false`、version=`v2ProPlus`；`go-api.command`也写明默认Juno-e30＋Juno_e16_s80。`Juno_zhcn 3/`目录另有e5／e10／e15／e20／e25／e30及多个SoVITS训练步数文件，不能仅按“Juno”或最高文件名认定它们已载入。

该启动脚本的实际配置为`api_v2.py`、port9880、`GPT_SoVITS/configs/tts_infer.yaml`、`./runtime/bin/python3`；**默认host为`0.0.0.0`**。本轮未执行；后续开发验证应明确使用loopback启动参数，不能直接照跑而扩张监听范围。旁边还有`GPTSOVITS_MLX/`候选目录，本次仅核对顶层及与voice命名相关目录，未选择、执行或据此更换Python GPT-SoVITS路线。

`Juno_zhcn 3/demo/`现有`audio.wav`及`audio (1)`至`audio (5).wav`六个文件，但未听取内容、读取声学样本或猜写reference transcript。项目当前`config/server.d/profiles/voice.default.toml`与`config/settings.toml`仍为`alia-default`，reference_voice、style_prompt、GPT／SoVITS权重覆盖路径均为空，语言zh；因此参考音频具体选择和匹配prompt仍未从现有项目配置恢复，不能把目录里的任一WAV擅自当作已选参考。

当前受控字段盘点与重构前`3530395`中已跟踪示例一致：TTS endpoint=`http://localhost:9880/tts`、request60s／idle10s；Whisper endpoint=`http://localhost:8080`、说明model=`large-v3`、request30s。重构前voice示例也没有Juno参考音频或权重路径；它不提供额外已确认选择。项目浅层限定盘点未发现Whisper checkout，随后按项目安装脚本准确命中上述`.local/share`目录，无全盘递归搜索。

三个官方PCM小文件已本地读取并计算SHA，分别29,184／104,192／81,920字节，均匹配下方live固定摘要；只做小文件离线身份核对，没有发给模型。剩余事实仍是“磁盘资产和候选配置存在”，不是服务正在运行、指定checkpoint已加载、音色已生效或推理已通过。

## 现有入口与真实证据边界

| 入口 | 可复用能力 | 不证明什么 |
|---|---|---|
| `services/easy-turn/src/alia_easy_turn/smoke.py` | 官方16个WAV经显式单声道／16k转换后通过HTTP提交，逐次写入真实状态和延迟；失败非零，超时停止 | 未经过Alia VoiceSession、LLM、TTS或硬件。 |
| `packages/alia-server/tests/easy_turn_live.rs` | `--ignored`显式运行，production Rust detector调用真实服务，验证Backchannel、模型修订和非空transcript | 只含一个官方样本，不等于四类准确率或整链。 |
| `packages/alia-server/tests/voice_stream_live.rs` | 真实gRPC→Easy-Turn→PG source／Context／Response／control ledger；验证Complete只产生一次识别输入、Backchannel恢复原输出、WAIT保持原输出身份和期限 | 设备认证、VAD位和软件输出是显式fixture；`canonical_response`由测试构造，assistant commit数量断言为0，未经过真实LLM／TTS／扬声器。 |
| `packages/alia-server/src/api/voice.rs` | 已认证的 `/voice/turn-detection`、`/voice/transcribe`、`/voice/synthesize`；`/voice/chat`可做显式HTTP音频链 | turn-detection单独调用不产生普通Generation；HTTP voice/chat不能替代连续设备VoiceSession的验收。 |
| `docs/refactor/checklists/r02-mac-2026-10-14.md` 与既有PCM generate/replay | 双流合成PCM、capture／post-mix证据、有界文件与单consumer重放 | 正弦／静音等合成PCM不能当作真实语言、转录准确性、Backchannel／WAIT语义样本，也不是AEC效果。 |
| `llm_providers.rs`、provider模块内HTTP测试 | 工厂选择、请求字段、SSE／错误／取消等离线契约覆盖 | 其中测试名的“real”可能仅指真实生产路径＋受控HTTP，不代表真实神经模型服务。 |

真实Easy-Turn既有资料：`services/easy-turn/VALIDATION.md`记录2026-09-08 CPU FP32官方样本16/16；首次请求1448.90ms，之后286.01–536.95ms，排除模型加载时间。当时服务smoke使用180s，不得把该历史实验值移作本轮扩大期限的依据。MPS在钉住Torch2.2.2上因`torch.isin`缺失失败，已有启动前拒绝；`auto`选择CPU，没有静默MPS回退成功的结论。

`docs/refactor/handoffs/2026-09-28-r04/models-v4-result.json`记录两现有live目标的历史命令exit0、10.37s、无source drift；09-26同类原件为20.81s。09-23 `voice-component.md`保留首次8s检测超时和后续通过，冷启动原因当时只是推测。它们均不认证当前10-17软件、当前服务状态或本轮整链；不得删除失败或无变化重复求绿。

## Easy-Turn固定契约

服务指南：`services/easy-turn/README.md`；常量：`src/alia_easy_turn/provenance.py`；依赖：`requirements-macos.lock`及`build-constraints.txt`。

- 源码 `fe175eaa8442c9a0834acf29f8640ee74c2aceca`；模型 `7d1f8b06776b575ac22e2fd4aa7913da2f4b8b98`；Qwen `7ae557604adf67be50417f59c2c2f167def9a775`。
- checkpoint 3,451,801,684字节，SHA-256 `1cb535bb0ffe3ffa0ac941bae4898aeb87adef7b88bb58f665af08eded6f6070`。Qwen各文件由已有`alia-artifacts.json`校验。Python3.10、Torch/Torchaudio2.2.2为历史验证栈；本轮不升级或安装。
- 当前路线是Mac Server CPU、一个进程／worker。指南的prepare会显式下载约4.5GB，本轮不执行；serve本身不下载。部署路径存在及实际内容仍须下一阶段在已确认目录核对，不能只引用旧报告。
- 监听仅loopback；现有服务契约没有认证字段或token参数。需要远端代理／认证的部署必须先确认与当前adapter的相容性，不能擅自扩大暴露范围。
- `/v1/detect`是JSON＋base64原始`pcm_s16le`，16kHz、单声道、完整采样、100ms–30s，无WAV头；请求体上限1,284,096字节。保留request_id与generation_epoch关联，不接受调用者提供模型prompt。
- 真实神经推理并发为1，无排队；HTTP连接另有32上限。Rust adapter默认max_concurrency1，用try-acquire直接Busy。超时／断连不终止native推理，服务保留该slot直至真实完成，不能超时后马上重发重叠推理。

官方离线PCM可复用目录为`run/easy-turn-stream-pcm`，用户答复后已确认三个文件存在且小文件SHA匹配。gRPC live测试固定三个摘要，必须保留原转换字节：

| 文件 | SHA-256 |
|---|---|
| `complete_real_1.pcm` | `99d723b782fa39adf1e2a80e1e8ae6e8909cf75dd27007ee3d4cafcdaf32570d` |
| `backchannel_real_1.pcm` | `6511d724d7618a84fe044c3bbc931ba240ba557e11bf138f273d31f3da4477d8` |
| `wait_real_1.pcm` | `cf2152f26f0e0a0768b368ac924a071516de37de273b70c133eed206880bcdb7` |

该fixture按100ms片段发送，并显式标记speech_active，结尾700ms静音触发判断；这不测试物理VAD。用户授权的其他离线PCM可作为另外的部署样本记录结果，不能替换这三个文件后放宽固定语义断言。新增合成语音也要记录来源，不能混称官方样本。

## Provider资料与health边界

**LLM**：major/minor和memory_capture／sentence_expression等role按`profiles/llm.toml`与routing实际选择核对。OpenAI兼容adapter根据`api_family`分别POST `/chat/completions`或`/responses`，使用Bearer、model、消息、token上限及stream；“兼容”不意味着端点实现了两种API或tools／JSON能力。确认模型实际ID、所需SSE／tool／JSON支持和允许启用的role，不能从示例local Qwen／Gemini默认推断现有服务。`health_probe_enabled=false`时配置key返回Degraded，空key为Unhealthy；开启后GET `/models/{model}`仍非推理。保留probe=false，不把上一轮noop全healthy断言照搬，也不为了health变绿启用外部probe。

**Embedding**：若本轮使用，需实际model、端点、维度和认证；POST `/embeddings`，30s包含完整响应。仅text-embedding-3系列请求显式发送dimensions，仍校验响应profile／维度／顺序。不能把noop或模型名字符串视为向量语义能力已验证；不要擅自改变已有SQL模型身份或政策。

**GPT-SoVITS**：endpoint应指向`/tts`，POST请求包含text/text_lang、服务端ref_audio_path、prompt_text/prompt_lang及既有推理参数，返回WAV，buffered streaming_mode0／streaming1。音色参考路径／权重是真实服务主机上的资源。init或相关设置可能GET `set_gpt_weights`／`set_sovits_weights`，失败只warn并沿用已加载模型，因此初始化成功和desired profile都不是所选权重生效证据。GET `/tts`的400也会被health判Healthy；必须另取服务端加载身份与实际合成结果。pitch header fallback默认false，不顺手开启。

**Whisper（可选）**：仅明确独立HTTP转写验收时准备。base endpoint＋`/inference`，multipart file、language、verbose_json、token_timestamps；原始PCM包装为16k单声道WAV后上传。配置model字段只是说明，实际模型由whisper-server启动选定。现有adapter没有专用认证配置；若实际服务要求网关认证先确认适配，不临时写密钥URL或加新依赖。`/health`成功不证明模型、转录或语言符合预期。

## 下一轮可执行顺序（本轮未执行）

1. **资料与字节冻结**：收到集中答复后核对指定目录、锁文件、模型／服务身份与离线样本摘要，保存脱敏配置和只含必要身份的manifest；明确原端点状态及谁拥有启动／停止权。缺失资产先记录，下载／依赖升级另行决定。
2. **服务层真实最小输入**：仅对已授权服务逐个使用既有入口。先CPU Easy-Turn固定样本，区分加载、首请求与后续请求；LLM用合成短文本、TTS保存私有WAV不播放，optional embedding／Whisper只按已定范围。health作为状态记录，不作为infer通过。首个失败保留并定位，不改deadline或重复求绿。
3. **现有Easy-Turn软件集成**：在Root托管的可丢弃PG和生命周期owner下复用两个live targets；环境为`ALIA_EASY_TURN_URL`、`ALIA_EASY_TURN_PCM`、`ALIA_EASY_TURN_PCM_DIR`、`ALIA_TEST_DATABASE_URL`，由受控环境提供，不在文档硬编码新的账户秘密。既有命令是`cargo test -p alia-server --test easy_turn_live --test voice_stream_live --offline --locked -- --ignored --nocapture`。保留默认libtest并发，不能把其他ignored也顺带启用；两目标是既有显式live范围。
4. **真实LLM／TTS组合证据**：组件通过后再以现有认证API和Generation／Delivery入口串联获准的离线输入，记录实际角色、Response／Context／output身份、真正模型文本和TTS字节，写文件而不开放扬声器。现有voice_stream_live没有完成这一段，不应仅将模型换成真服务便声称端到端全覆盖。复用既有单consumer、精确取消、任务owner和PG清理，不预建另一个通用测试框架；如需小的编排补口先由Root审计现有入口再限定实现。
5. **收口**：比较源码／模型／样本manifest前后，记录原失败、组件计数、整链步骤、冷／热延迟和资源回收，保留默认shadow及批准边界。软件链通过后仍须单独开展有授权硬件的R02／R05验收，R03继续暂停。

## 保留的期限与清理规则

Easy-Turn生产示例5s／并发1，CLI默认服务inference60s；单adapter live既有30s，gRPC live detector8s、capabilities5s、单event10s、控制收敛／wait_open3s、流关闭／server join2s。PG fixture body原45s、CASES3；初始化准入不应偷占新阶段观察窗口。软件hold协商30s、生产默认60s／硬上限120s与临时语义3s是另外的授权期限，不因HTTP超时延长。

TTS示例请求60s、stream idle10s，buffered单payload1MiB／流累积8MiB等原音频预算保留；Whisper示例请求30s，health5s；embedding请求30s。LLM chat／stream的生命周期与5s health probe不同，本次不把health期限错误标成生成期限；下一阶段沿用现有调用方取消和预算，并在执行前明确证据观察界限，不用未知服务无限等待或事后放宽。

本轮没有实模运行，故没有新性能数字、模型准确率或当前部署可用结论。后续运行必须先托管服务／请求／输出／DB原owner，取消后继续收口原join，再核对资源；不将HTTP超时视为native推理已停止。真实麦克风、VAD、AEC、扬声器听感、执行器及完整自然全双工体验均仍在本分工验收之外。
