# R02 / R03 / R05 准备度 — 独立审查 HANDOFF

日期：2026-10-14。分工：`settings_auth_review`。本文件只记录仓库文档、代码和既有验收入口的只读审查；未执行 Cargo、数据库、Docker、Git、模型推理、SSH、音频采集或执行器动作。生产与测试代码均未修改。Settings 的有界审计由本轮另外两个分工记录。

## Checklist 与进度

- [x] 核对 R01—R06 checklist、延期台账、10/04 与 10/06 音频/动作交接和部署说明。
- [x] 核对 mixer → native reference 与 capture → wake/VAD 的实际路径及消费者。
- [x] 核对签名 Body → 资产读取 → 唯一 writer → 当前后端回执的证据边界。
- [x] 找出可以复用的离线测试、真实模型 smoke 和无外设部署演练入口。
- [x] 列出最少部署/硬件资料、R02/R03 各一个最早可执行的软件子包及后续顺序。

本分工只读准备度审查 **5/5**。这不是 R02/R03/R05 完成数，也没有新增运行通过证据。总台账仍是六轮 **2/6**（R01、R04），各轮规模不同，不换算为项目百分比。

本次读取的 2026-09-07 架构提案自己明确标为 Proposed，其中目标不视为当前实现。用户原 10/04 Desktop 附件在原路径未找到；本次依据仓库内 10/04 保存的审查结论、`run/validation-20261004/audio-readiness-notes.md`、checklist 及后续源码核对，未声称重新读到完整原附件。10/04 音频交接中“尚无多源 mixer”属于历史状态，已被 10/06 实现取代。

## 当前事实与尚未验证的出口

| 范围 | 已有软件 | 仍缺的软件 / 契约 | 硬件或部署待证 |
| --- | --- | --- | --- |
| R02 | Voice 与 LocalCue 实际混音，提示增益 0.2、语音增益 1.0、合成限幅；每源保持原身份、取消和进度；native reference 来源为最终混音及 native 转换之后的实际提交音频。capture 有独立 stream/clock/cursor/continuity，损坏后结束当前 session、清 pre-roll、重置 VAD/KWS。 | 生产 AEC consumer、明确的参考消费所有权、AEC 分帧、采集与播放时钟映射/漂移处理、同步证据记录与重放工具。 | 采播设备/格式/独立时钟、硬件 AEC、布局、实际双讲/尾音/暂停恢复、声学损伤与端到端延迟。 |
| R03 | 严格签名 Body 请求/回执、owner/lease/boot/nonce 身份、唯一 writer、预先验证完整 plan、精确取消、提交进度、部分失败与迟到清理；真实 loopback gRPC 的未绑定/错误 controller/旧 boot 拒绝已有测试。 | `Animation` 签名覆盖文件名和参数，未绑定所读资产 bytes；无受信资产摘要目录；当前没有 MCU boot、ACK 或位置反馈契约。 | 控制板与固件、执行器反馈、限位/标定、速度/负载、停止和断联安全态、真正停止/到位。 |
| R05 | Server 与 Embedded 独立配置及进程启动入口、REST/CLI 管理/配对说明；历史 Linux ARM64 容器 default release/link、无外设二进制启动/重启演练已归档。 | 当前指定机器的可复现部署清单、干净安装/配对/升级恢复记录和真实长稳测量。 | 当前部署 Mac/Pi 身份与配置、真实音频/动作、至少 24 小时混合负载。容器证据不能替代 Pi。 |

依据：[next-round checklist](../../checklists/next-round.md)、[deferred 台账](../../deferred.md)、[10/06 audio](../2026-10-06/HANDOFF-audio.md)、[10/04 motion](../2026-10-04/motion.md)、[音频时钟设计](../../audio-reference-clock-design.md)。Easy-Turn 继续部署于 Server；音频 mixer 本身不执行回声消除，Easy-Turn 的轮次分类也不代替 AEC。

## R02：最早离线软件子包

建议先做 **有界双流证据导出与离线重放**，直接服务已有“同步记录格式尚未固定”的缺口。在取得实际音频设备资料前，用合成 PCM 和已有 native sink fixture 验证，不选定新 AEC 依赖、不声称已对齐或已消除回声。

具体入口：

- `crates/alia-core/src/audio_clock.rs`：沿用 `AudioTiming`、`CaptureTiming`、stream/boot/clock/continuity、原生与转换后游标、`observed_boot_ns` 和时钟质量。观测时间不是 DAC 呈现时间，转换批次的 native 首样本不等于重采样输出首样本。
- `packages/alia-embedded/src/module/context/audio_reference.rs`：`AudioReferenceBus::try_read` 是单消费者队列；`snapshot` 独立保存最终丢失/关闭信息。已有 64 块、每块 1024 样本、非阻塞生产边界。不得另加竞争消费者以“同时录音”；后续 AEC 与导出须由同一消费所有者安排。
- `packages/alia-embedded/src/module/context` 的 `AudioFrame` 与 `module/speech/duplex.rs::accept_frame`：capture 当前经过连续性检查后直接送 VAD/KWS/上传，未经过 AEC；应在明确的处理边界记录既有格式与转换事实，不能把已转 mono 的 PCM 称为原始多通道采集。
- `packages/alia-embedded/src/module/cpal_audio/playback.rs` 及 `alsa_backend.rs`：复用已有最终 reference 生产点。CPAL 数据是实际 callback buffer 的 native 表示转换结果；ALSA 仅记录 `writei` 已接纳完整帧前缀。`module/audio_debug.rs` 只保存 source PCM，且当前 continuous duplex 没有 `record_capture_frame` 调用，不能用它直接代替同步证据。

子包交付和验收边界：

1. 固定本地证据记录格式，保留两流原有身份、格式、游标、分段、时钟质量、内容类型、loss/close snapshot 和实际样本；所有“未对齐/未知延迟”显式保留。不能用相同采样率或共同 host observation 推导采样时钟相同。
2. 实现有界消费/导出和重放；保持生产 callback 不做磁盘 I/O、不等待消费者，资源满时报告丢失。worker 的停止、join 和最终 snapshot 必须有明确所有者。导出样本格式应忠实保留现有输入表示，不能悄悄变采样率/混声道后仍标为原格式。
3. 用合成输入覆盖完整双源 mix、单源取消、held silence、zero fill、ALSA partial accepted prefix、队列满、最终 close marker 丢失、capture lag/conversion reset、设备流重开与未知/回退时钟；重放必须拒绝把缺口补零或拼接成连续声学素材。
4. 实际录音保持显式启用，并记录所选设备、范围、保留方式；本软件子包可只用合成数据验收。之后在 H02/H03 明确时，将相同记录用于现场基线和实际 AEC 接入。AEC 帧长、格式转换、延迟/漂移算法由选定消费者约束，不能把现有 1024 样本块或上传帧当作 10 ms AEC 帧。

可复用窄测试入口（本分工未执行）：

```sh
cargo test -p alia-embedded module::context::audio_reference::tests --offline --locked
cargo test -p alia-embedded module::cpal_audio::playback::tests --offline --locked
cargo test -p alia-embedded module::speech::duplex::tests --offline --locked
```

已有关键断言包括 reference 队列满和最后 close 丢失仍可见、consumer contention 不阻塞 producer、capture/playback/restart 时钟不同、混音限幅及最终 reference、取消单源不伤另一源、ALSA 短写只结算已接纳 mix。它们是子包基础，不应重新写一套平行音频 backend。Linux ALSA null fixture 只证明软件/libasound 边界，不能证明真实采播时钟或声学效果。

## R03：最早离线软件子包

建议先做 **签名 Animation 绑定受信资产 bytes**。这不依赖控制板通信协议，能关闭当前真实授权缺口。不得将未知 MCU 协议实现为假 ACK，也不提前增加反馈位置字段并据此声称硬件能力。

当前路径：`crates/alia-core/src/body.rs::BodyMotion::Animation { file, fps, loops }` → `packages/alia-server/src/channels/embedded/body.rs::BodyIntent::parse` → 已签名 JSON → `packages/alia-embedded/src/module/motion/plan.rs::prepare/read_animation`。资产读取已有 canonical path confinement、regular file、64 KiB/600 source frames、有限角度与 duration/loops 检查；plan 一次性物化全部动作，在 submit 重新验证权限后交给 writer。这些边界保留。

最小必须同步修改的范围：

| 层 | 必须解决的契约 |
| --- | --- |
| 受信资产来源 | 指定 Server 如何获得经批准资产的内容摘要/版本/长度。可用受控静态清单与本地资产，不引入分发平台。不能直接信任 LLM 生成的摘要，也不能把 Embedded 当前读到的 bytes 自报摘要等同于授权。对未知资产应拒绝。 |
| `alia-core` | Animation 的严格 signed JSON 携带预期内容身份并校验格式；明确 `BODY_CONTRACT_VERSION`、请求/回执版本拒绝和旧命令迁移规则。资产摘要不能 optional 后继续接纳没有绑定的旧 Animation。已有 workspace `sha2` / core 依赖可复用。 |
| Server adapter / expression | 由可信清单归一化 `play_animation` 参数并固定摘要后创建 `BodyCommand`；当前 capabilities 仅声称 `play_animation` 支持，没有资产目录，须如实描述可以选择的受信资产与不兼容版本。原 owner/scope/Execution 批准条件保持。 |
| Embedded preparation | 对同一次有界读取获得的 bytes 先验证所签身份，再从这些 bytes 解析/生成 plan；不能校验一个文件后重新打开另一个文件执行。保持路径、大小、帧数、角度、速度/标定及提交前权威重验。 |
| Envelope 与兼容性 | `crates/proto/proto/body.proto` 当前只是 `SignedBodyPayload { payload, signature }`，内载 strict JSON；不需为资产绑定凭空增加 MCU RPC。需同步检查签名 domain/contract version 的发送与接收，以及旧版本 fail-closed 行为，不能只改一个 Rust enum。 |
| 测试 / 文档 | core serialization/validation、server 参数与能力、embedded 文件替换/不符摘要/缺失/超界、同身份不同资产拒绝、验证失败前零写入、精确取消和 receipt 保真；同步 fixtures、`motion-assets.md` 与协议说明。 |

优先验收：合法已登记资产可形成预期完整 plan；同名改内容、大小或版本不符、路径逃逸、旧未绑定命令不能进入 writer；验证后路径被替换也只能执行已经验证并物化的 plan；错误不得改变原签名身份、执行批准、取消及资源清理保证。仅“给文件算一个 hash”不能完成这项工作。

可复用窄测试入口（本分工未执行）：

```sh
cargo test -p alia-core body::tests --offline --locked
cargo test -p alia-embedded module::motion::plan::tests --offline --locked
cargo test -p alia-embedded module::motion::runtime::tests --offline --locked
cargo test -p alia-embedded grpc::body_service::tests --offline --locked
cargo test -p alia-server channels::embedded::body::tests --offline --locked
```

`packages/alia-server/tests/body_delivery.rs` 还有真实 SQL ledger/adapter 回归，执行需 Root 管理隔离数据库。现有 plan 测试已覆盖文件大小/帧数/loops/duration、symlink/非 regular file、完整轨迹预检；runtime 已覆盖重复身份、部分 native write、迟到 quiescence、取消后仍有原 owner 与 join、标定失败前未构造 native backend。

硬件证据边界：当前 serial 后端只 `write/flush`，PCA9685 只 I²C register write；二者均无读回 MCU ACK/位置。`device_boot_id` 是 Embedded 进程 boot，backend instance UUID 是软件 writer 身份，均不代表 MCU 重启身份。Body v1 的 `position_confirmed_actions` 必须为 `None`，validator 拒绝伪造位置。接受、软件提交、控制板确认、传感器确认到位必须分别展示。

## 最少部署 / 硬件资料

Root 已集中向用户询问，避免重复提问。以下字段缺失只阻塞依赖它的现场动作，不阻塞上述离线工作。

| 台账 | 最少当前资料 | 决定什么 |
| --- | --- | --- |
| H01 / H05 | 最终部署 Mac 是否就是当前工作宿主；OS/CPU/RAM；模型服务地址、运行方式、现有权重/版本和实际 backend；现有配置/资料路径。 | Server 部署与真实模型矩阵、预算和故障边界。历史 M5 Max/64 GiB/macOS 26.6.2 不能自动当本轮部署事实。 |
| H02 | 当前 Pi5 8 GB 的 OS/架构；可信 SSH alias/host 及确认方式；供电、散热、存储；允许的检查窗口。 | 先只读采集 OS/toolchain/资源/ALSA/USB/串口，再确认部署方式。历史 KeQiaopi/Ubuntu 24.04.4 资料不能替代当前目标确认。 |
| H03 | 麦克风/声卡/扬声器或功放的型号与连接；硬件 AEC 是否存在及输入/输出路径；实际采播格式/声道或可读设备信息；麦扬间距、方向、安装方式和测试音量。 | 软件 AEC 放置、参考链、时钟域及现场基线。缺数值可以先记录 unknown，不能编造格式或假定双设备共时钟。 |
| H04 | 控制板与固件/协议资料；执行器型号、接线/供电；是否存在 ACK/MCU boot/位置反馈；通道映射与已校准限位/速度；机构负载、停止/失联/断电安全状态及现场监督条件。 | 是否能反馈实际位置，如何停止、断联恢复与验证。不能猜“全置零/卸力”是安全姿态。 |

仓库 `assets/neck` 和样例配置可用于离线契约 fixture，不是当前机构标定事实。`motion.hardware_enabled=false` 的 mock 成功也不说明控制板在线。

## 可复用的 R05 入口与顺序

1. 先收口本轮有界 Settings 审计，进入 R02-A 与 R03-A；不继续自动扩张为全部 GET 或权限框架审计。两个离线子包可在职责分清时并行，保持默认 shadow、原执行批准及政策。
2. H01—H05 到位后执行可信目标的只读盘点，更新部署事实；先建立实际音频基线，决定硬件/软件 AEC 链路；再按明确控制板协议接入 ACK/boot/反馈。取得现场安全状态后才进行有限机构动作。
3. R02 实测覆盖远端单讲、近端单讲、双讲、附和、明确暂停/继续、噪声、断联/重开，保留音量/布局、参考缺口、用户语音完整性与端到端暂停延迟。不能将 Easy-Turn label 正确当成声学验收；ERLE 只用于条件明确且对齐的远端单讲记录。
4. R03 现场验证迟到命令、断连、MCU/Embedded 分别重启、限位、停止失败与恢复，保留实际反馈证据；open-loop 设备只能报告能够证明的提交层级。
5. 再完成 R05 干净安装、配对、启动、升级/恢复、关闭和至少 24 小时混合负载。阈值由实际设备基线和用户接受条件确定；架构提案里的建议数值不直接当本轮测得门槛。

可复用入口：

- [Server / Embedded operations](../../../operations/server-embedded.md)：分别使用 `ALIA_CONFIG_PATH` 与 `ALIA_EMBEDDED_CONFIG_PATH`；现有 OOBE、独立 PostgreSQL、健康 `probe=false`、`/api/v1/channels`、`/api/v1/device/status?channel_id=…`、需要确认的设备绑定及 Response delivery events。手册是操作入口，不是已完成当前目标部署的证据。
- [Model services](../../../operations/model-services.md)、`services/easy-turn/README.md`：本地 fake contract tests 为 `services/easy-turn/.venv/bin/python -m unittest discover -s services/easy-turn/tests -v`；真实模型 smoke 为 `python -m alia_easy_turn.smoke --sample-dir … --output …`，需已有模型服务与素材且输出新的证据路径。该 runner 记录重采样/混声道、实际标签/响应/时延，超时会停止，仍不是实机自然全双工验收。`prepare` 是显式大体量下载步骤，本轮未执行。
- [历史无外设 release smoke](../2026-10-06/evidence/linux/release-smoke.py)：可复用隔离配置/随机 loopback 端口/独立 schema/两轮进程生命周期及资源清理方式。此档案脚本固定 `/runtime/release-smoke-v3` 和 `/evidence` 且拒绝已存在输出，不能当通用 installer 原样再次运行；其中模型、音频、发现均为 noop，motion/hardware 关闭。后续须针对当前源码和新证据目录重建演练，不重写历史证据。
- `packages/alia-server/tests/duplex_playback.rs` 与 `voice_stream_live.rs`：可复用 transport/duplex 与明确 opt-in 的真实模型测试边界；真实模型需要实际环境。两者均不能替代麦扬声器与机构现场验证。

未发现可直接代表当前 Pi 目标“干净安装 + 24 小时真实混合负载”的已完成报告或通用验收 runner。现有容器演练和软件测试仍有价值，但应保留各自输入、环境与 no-hardware 标签。

## 保持暂缓与本次未验证

Web/iOS/Android、Embedded 通用跨设备适配、其它 Server OS/CPU、Pi 侧 Easy-Turn、同机/同进程产品化继续按台账暂缓。不得因 Cargo feature 或本地 mock 存在声称部署支持已完成。

本分工没有新增运行验证、真实录音或硬件动作。R02-A/R03-A 是有源码依据的建议子包，尚未实施；资产受信摘要来源与现场硬件协议需明确后再形成各自提交范围，R02/R03/R05 保持开放。

Root 当前验证与阶段进度见 [最终交接](root-HANDOFF.md#root-最终收口)，当前结果不回写为该分工独立执行。
