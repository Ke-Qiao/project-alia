# R02 R1 前置核对 — 主机音频路径 HANDOFF

日期：2026-10-14；Owner：`settings_auth_fix`；分支 `codex/architecture-refactor`。只读检查前轮 clock 检查点及当前实现；本分工 **6/6**，不表示 R1/AEC 现场验收完成。只新增本文，未修改生产或测试，未运行 Cargo、Git、SSH、音频设备、服务或模型，也未新增依赖。R03 保持暂停。

当前用户确认：**采用硬件 AEC 优先路线**。实物/固件版本、对应厂商资料，以及功放/扬声器选型和参考规格仍未确定；Pi 使用 SSH 连接不证明 R1 已接入、识别或启用 AEC。当前不实现软件 AEC consumer，先补齐硬件处理输出与参考路由的事实，再按下述主机路径核对。这个路线选择不改变本文的代码静态结论，也不构成硬件工作状态的验证。

## Checklist

- [x] 读前轮 clock 检查点、现有硬件资料及音频说明，沿用已完成时钟/参考工作。
- [x] 追踪 CPAL / ALSA 的设备与格式选择、fallback 和配置含义。
- [x] 追踪 native capture → 转换/增益 → AudioBus → VAD/wake/VoiceSession。
- [x] 核对多声道平均、processed/raw/reference 混入的条件风险。
- [x] 核对 postmix 参考、单 reader、取消/关闭、帧与延迟的已知和未知边界。
- [x] 向 Root 报告重要发现，写明后续必要资料/条件门槛并停写；中央文档与提交由 Root 完成。

## 首要结论

**当前主机在收到多声道 capture 时，会把所有输入声道平均成单声道；`capture_channels=1` 不是选择某个处理后声道。** 若 R1 的同一实际输入端点混有处理后语音、原始麦克风或播放参考，这条路径会将其一起平均，可能破坏所需语音信号。当前没有该实物/固件的已核验端点与声道表，所以这是明确的条件性接入缺口，不能写成 R1 已实际混错声道的故障。

若经核验的端点输出只有硬件已处理的 mono PCM，现有路径可直接把它转换为 speech 所需格式。软件没有自动再加 AEC；无需因为参考总线已存在就先造一个主机 AEC。若设备要求 host 选取多声道中的某一路，才需要按已确认映射补最小选择能力；当前不猜“第0路/第6路”，不以默认平均替代这个决策。

## 设备与格式选择：请求值不等于设备实际值

| 位置 | 当前事实 | 对 R1 核对的含义 |
| --- | --- | --- |
| `packages/alia-embedded/src/config/types.rs:377`、`:457` | 请求默认 capture=16000Hz/1声道，playback=24000Hz/1声道，device=`default`，增益1.0；没有 AEC mode、processed channel、raw/reference channel 配置 | 默认值不是 R1 的 USB 原生格式、固件模式或声道身份 |
| `packages/alia-embedded/src/module/cpal_audio.rs:808`、`:830` | CPAL 的 `default`/空串使用系统默认输入；其他值按设备名称 `contains` 找到的第一个设备；输出同理 | 不是 VID/PID/serial/物理路径绑定；相似名称/默认设备切换必须现场核对 |
| 同文件 `:839`；`cpal_audio/cpal_backend.rs:22` | CPAL 优先匹配请求声道+支持的请求采样率；其次相同声道的最大采样率；再退到第一个支持范围的最大采样率；查询失败还可用 default input config | 请求 mono 并不保证 native mono；实际格式可能多声道/不同采样率。启动日志/原生 metadata 有 actual format，但未核验声道语义 |
| `packages/alia-embedded/src/module/cpal_audio.rs:794`、`:935` | `auto` 遇显式 ALSA PCM 名称选 ALSA，否则先 CPAL；Linux CPAL启动失败可退到可解析的ALSA名字，包括`default` | 需固定实际设备选择与解析结果，不能把系统 `default` 当作已识别 R1 |
| `packages/alia-embedded/src/module/cpal_audio/alsa_backend.rs:240`、`:257` | ALSA 申请交错 PCM，`set_channels_near` 和 `set_rate_near`，实际声道/速率独立返回；允许 ALSA rate resample | 配置 target mono 不约束原生端点必须mono，也不指定声道选择；ALSA插件还可能在主机代码之前转换，语义须核对 |

本轮没有打开或枚举设备。R1 尚未接入、USB计划、硬件 AEC 支持能力与参考电气/路由未知，沿用用户确认和中央硬件台账；本文不新增硬件事实。

## Capture 如何进入 speech

1. `packages/alia-embedded/src/module/cpal_audio/cpal_backend.rs:377` 获取 callback 时间与原生完整帧，`:383` 在转换前调用 `observe_capture_samples`。ALSA 成功 `readi` 前缀也经过相同 helper（`alsa_backend.rs:102`、`:133`）。native reference因此保留后端交付的所有声道和样本容器，但不说明这些声道是 ADC raw 还是固件已处理数据。
2. `packages/alia-embedded/src/module/cpal_audio.rs:720` 的 `CaptureConverter::convert` 无条件调用 `downmix_to_mono`（`:889`）：每个交错帧转为i16后求各声道算术平均。它没有声道索引、映射或语义标签。即使target声道大于1，也是先平均，再 `interleave_mono`（`:902`）复制相同mono值；不是保留独立声道。
3. 如果实际采样率与target不同，`:738` 使用有跨块last sample/phase的线性插值转换，再复制target声道。它不是播放侧rubato，不是AEC、波束形成或漂移控制。连续性变化会先reset capture converter（`cpal_backend.rs:386`；ALSA同类处理）。
4. `cpal_backend.rs:400` 对转换后PCM施加capture gain，`:423` 发布 `AudioFrame`，包括转换后游标和native批次timing；没有经过AEC processor。`AudioBus` 本体在 `module/context.rs:22`，converted broadcast和native reference是两条独立消费路径。
5. `module/speech.rs:98` 订阅converted `AudioBus`。`config/types.rs:269` 强制continuous voice配置为16kHz mono；`speech/duplex.rs:382` 校验实际converted format、游标和native连续性。`:143` 按最多1600样本切片，先VAD，待唤醒后上传；VAD提供speech_active，不丢掉静音PCM。`:269` 直接封装 `pcm_s16le`，没有Opus编码步骤。
6. 播放期间capture仍继续（`speech/duplex.rs:93`）。广播丢帧、时序不合法等损伤会结束会话、清pre-roll、重订阅并reset VAD/wake，要求新唤醒（`:106`、`:114`）；不会以补零修复不连续。

所以硬件已消回声的输出只要位于**选中的唯一语音输入路径**，就会进入上述普通PCM转换与增益处理。代码既不验证“硬件AEC已启用”，也不识别哪一路是processed；`capture_channels=1` 与speech格式检查只证明最终格式，不证明声音来源正确。

## 播放参考与 consumer / 清理

实际播放先由Renderer转换到已选后端格式、施加volume（`cpal_audio/cpal_backend.rs:326`），随后两路voice/cue mixer写最终CPAL callback；`cpal_backend.rs:468` 在输出填完后记录实际格式样本。ALSA只记录 `writei` 接纳的完整前缀（`cpal_audio/playback.rs:1143`），不是未提交TTS源PCM。该软件参考不会自动发给R1，也不证明已经接入阵列AEC的物理参考端口。

`module/context/audio_reference.rs:14` 固定每块最多1024个f64样本、队列64块，capture/playback共用。`:137` 单独claim非Clone reader；`:196` 只有reader可消费；`:210` Drop仅释放claim，不清队列、不关闭设备。`module/audio_evidence.rs:160` 的诊断会话显式持有reader，pump有界，cancel/limit停止观察，finish/Drop才释放；当前没有正常运行的AEC reader或自动诊断录制。claim它不会改造speech broadcast，也不能与另一个reader同时各拿一份reference。

实际填零/hold静音和丢失reference不同；gap/sequence/loss、stream重开与时钟提示保留。前轮已完成native原始callback/sample回退分段、整批Unknown和历史Replay的ClockDiscontinuity；不重复改造。采播各自stream/clock，时间估计、游标与主机观察时间不是物理对齐证明。

资源归属现状：capture stop发控制信号并join线程（`cpal_audio.rs:121`），CPAL runtime在控制退出后drop stream（`:506`）；runtime字段先native stream、后clock owner（`cpal_backend.rs:10`），关闭后发布最终参考统计。播放stop先sender.shutdown再join（`cpal_audio.rs:269`），renderer/buffer按既有owner清理；ALSA停止PCM后drop owner（`alsa_backend.rs:231`）。观察者取消不调用这些控制路径。上述代码顺序/历史回归不等于已实测USB拔插、native析构延迟上界或物理尾音；同步join本身没有新加墙钟期限。

## 帧、声道与延迟：当前可写的值

| 项目 | 明确值/规则 | 尚不能据此得出的结论 |
| --- | --- | --- |
| speech格式 | 16000Hz、mono、PCM S16LE；`SpeechConfig::validate`与CaptureClock校验 | 不证明R1输出端点本来是mono/processed |
| speech单次切片 | 最多1600样本，即最多100ms；短尾不补齐 | 不是固定100ms、20ms或AEC frame |
| VAD示例窗口 | 512样本/16kHz，即32ms；`sherpa.rs:219`在内部累积到window_size | 是VAD配置，不是R1固件AEC或未来主机AEC窗口 |
| ALSA capture period | `opus_frame_ms` 默认20，只用于请求capture period；实际 `get_period_size` 另读，硬件可能协商不同 | 当前VoiceSession未因该配置进行20ms Opus编码；不能承诺实际callback节奏 |
| native reference块 | 最多1024个样本，按实际channels组成完整帧，双向共用64槽 | 不是固定时长、不与speech切片一一对应、不提供采播对齐 |
| playback请求 | 默认24kHz/mono；CPAL buffer请求60ms并按支持范围夹取，target cushion默认300ms、max2000ms | 不是实测DAC延迟、端到端延迟或取消尾音；后端实际format可能不同 |
| 硬件AEC | 用户说明R1支持；当前无host启用/模式/参考配置 | 模式、USB端点/声道表、参考电气/路由、DSP延迟、输出链、残余回声/双讲效果全部待核对 |

`alsa_playback_lock_to_capture_rate=false`默认保留；该开关只在ALSA播放启动时选请求速率（`cpal_audio.rs:427`），既不证明USB采播同钟，也不把播放自动接入R1参考。不能因麦克风与扬声器都接Pi就开启它或宣称参考已连通。

## 后续最小门槛与本轮结论

- 取得对应实物/固件的输入端点、原生格式及各声道语义，特别确认对外是否只有processed mono，还是同一多声道端点含raw/reference。确认前不将当前全声道平均路径作为R1验收输入。
- 对照真实OS设备名/USB稳定身份与实际协商格式；若名字或default存在歧义，先固定部署选择。没有本轮现场数据，不提前扩大设备身份框架。
- 若只有单独processed mono端点，复用现有链并核验配置；若需多声道选取，后续仅补明确的选择/拒绝错误映射及转换前选路测试，不预造任意路由框架或附加AEC。
- 确认硬件AEC参考的实际输出链、连接/电气范围、启用模式和已知DSP延迟。postmix软件reference是诊断证据，不替代物理回路；再决定是否还需主机软件AEC及其consumer格式/帧契约。
- 真实音频质量、延迟、双讲与尾音仍需后续明确录放音窗口和保留规则。此轮仅静态审计，没有新的设备/测试结果，也没有已复现需立即改生产的硬件故障。

本分工完成后停写；Root负责汇总厂商资料、用户待补信息、中央checklist与提交。R03不推进。
