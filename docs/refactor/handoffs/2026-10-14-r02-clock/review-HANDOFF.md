# R02 时钟与 AEC consumer 需求 — 独立审查交接

日期：2026-10-14。Root 指定基线 `38fb689` clean；分工 `settings_auth_review`。本轮审查音频时钟、参考缺口和未选定 consumer 的接入条件，R03 继续暂停。不新增依赖、算法、平台或运行服务，不打开设备、不录放音、不运行模型或 SSH；本分工只读源码/证据并编辑本文，不运行 Cargo、Git、Docker 或外部探测。构建、实际 RED/GREEN、资源和提交由 Root 统一。

## Checklist

- [x] 沿用最新 R02 Mac、field-prep 和硬件确认资料，区分软件证据与硬件事实。
- [x] 核对实际 AEC 状态、固定版本依赖、native/converted/speech/VAD/重采样帧域和消费者路径。
- [x] 提出有界采播对齐、漂移和 future consumer 条件，明确缺失硬件资料的影响。
- [x] 独立评审两位 owner 的具体时钟/回放缺口与 RED 设计，向 Root 反馈文档精度。
- [x] 独立读回最终生产/测试候选，确认单 reader、精确取消和关闭统计边界。
- [x] 独立读取 Root 本轮 RED/GREEN 原始结果与清单，区分历史和本轮验证。
- [x] 汇总最终结论、剩余限制并停写；提交由 Root 完成。

当前 **7/7**，仅为本分工进度，不是 R02 硬件或项目完成比例。三项实际 RED、最终候选和本轮 GREEN 已独立读回，无阻断项；本文停写，源码提交、提交 blob 对照和归档由 Root 收口。

## 已有能力和未选定事项

当前应用没有 AEC processor/module/feature，也没有自动 reference worker。`AudioReferenceBus::try_claim_reader` 的非测试消费路径是显式 `EvidenceSession::start`；Builder 不自动创建该会话。reader 排他、不可 Clone，Drop 只释放消费权，不取消播放、不关闭 native owner、不清空剩余块。原生 tap 在既有 64 块固定内存队列内运行，没有自动文件录音。

Cargo 与本地固定版本源码表明：CPAL 为 0.15.3，Linux ALSA 为 0.9，rubato 精确固定 `=5.0.0` 且关闭默认 features，Sherpa 用于现有 KWS/VAD。没有 WebRTC/Speex AEC 依赖。mixer 负责 voice/cue 混音、音量与精确取消，混音本身不是回声消除。本轮未作算法选型，也未引用外部算法的帧长作为项目要求；不需要为未选型框架引入依赖。

`PcmResampler` 已用 rubato 做 playback 固定比率抗混叠转换，保留跨网络包相位/尾部并修剪库报告的 output delay。构造的最大相对比率为 1.0，项目没有调用 ratio 调节接口。固定版本 rubato 的 `Adjustable` 确实允许由外部反馈控制器改变比率，但此库能力不等于项目已经实现设备时钟漂移补偿；当前 wrapper 的内部块为 `input_rate/100` 整数截断，约 10 ms，不是 AEC 帧要求。

capture 仍先对声道求平均转 mono、按固定名义比例线性插值，再复制到目标声道并施加增益；转换器保留前块尾部和相位。native evidence 在这些转换之前记录。`CaptureTiming.native` 是贡献该转换输出的原生批次，不能把它的首样本时间直接配给转换后的第一样本。相同名义采样率和 sample cursor 对应时长也不构成物理配准。

## 必须区分的帧域

| 粒度 | 仓库实际语义 | 本轮结论 |
| --- | --- | --- |
| native reference block | 最多 1024 个交错样本，按完整声道帧切块；capture/playback 共用 64 块队列 | frames = samples/channels；callback/read 大小可变，存储块不代表固定时长 |
| converted AudioFrame | 转换后格式/游标以及贡献的 native 批次 | 保留转换事实；没有精确首样本映射或跨设备延迟校准 |
| speech upload chunk | 16 kHz mono，最多 1600 个样本；短 callback 尾部不补齐 | 最多 100 ms 的传输块，不是固定算法帧；queue 数量也不是精确音频时长 |
| Sherpa VAD window | pending_samples 按配置 window_size 缓冲，默认 512 | 当前 VAD 推理窗口，与 AEC consumer 帧长分别确定 |
| playback rubato 内部块 | `input_rate/100`、固定比率、同一播放时间线 | 不能由此推出 AEC 必须使用约 10 ms |
| AEC consumer frame | 尚未选择算法或硬件主机分工 | 采样率、声道、帧长、render/capture 调用顺序、延迟上界、尾帧和 reset 条件均待选型后确定 |

现有 speech 仍订阅独立 converted `AudioBus`，随后执行 VAD/KWS 和上传。仅取得 reference reader 不会自动把该输入变成 AEC 输出。未来 consumer 确定后才应决定 native/converted 的插入点、下游唯一有效输入和转换延迟，而不是本轮预先添加空框架。若硬件 R1 已完成适用的 AEC，也不能无条件再叠加主机软件算法。

## 采播对齐与漂移的条件契约

每个原生流分别持有 boot/stream/direction、format、cursor、continuity 和局部 clock origin。CPAL capture/playback 的时间戳原点分别建立；ALSA 只有状态观察和 queued delay，不产生 sample_ns。`observed_boot_ns` 是主机观察时刻，包含调度与路径位置差异。不得直接相减独立流的局部时间戳，也不能把 host 观察时间当作 ADC/DAC 采样时刻。

未来映射只适用于固定的一对连续片段，至少区分名义采样率比、实际斜率/偏移及其不确定性。Root 文档用 `p=a*c+b` 和相对名义比率的 ppm 作为条件定义是合理的；当前没有可供输出这些实测值的跨流锚点、误差界或校准证明。块内 sample offset 按名义采样率推算，不能作为额外独立测量来拟合真实漂移。重开、continuity 变化、原生参考缺口、转换 reset、时钟质量或可用性变化均使相关映射失效；缺证据保持 unknown，不猜阈值或延迟。

future consumer 只能在有效片段内累积其要求的完整帧，缺块不能补成零、跨 reset 拼接或拉伸。真实 HeldSilence/ZeroFill 与丢失是不同事实。必须分别处理 sequence gap、known producer loss、frame gap、未知 xrun 帧、marker 和最新快照；尾部没有下一条记录时仍需读取 latest snapshot，被新流替换且没有 close 证据的旧流终态保持 unknown。

消费者及其有界组帧/映射/算法状态由一个 owner 管理；诊断不得竞争第二个 native reader。取消观察只清理观察状态，不代替按身份精确取消播放或关闭设备。native 回调中不加入模型/文件 I/O；现有捕获转换和 broadcast 仍有分配/锁，不能把 reference tap 的不等待保证泛化成整条链路硬实时。

## 两个具体缺口的独立评审

基线生产 `AudioStreamClock::observe` 仅检测 callback_ns 倒退，sample_ns 单独倒退仍保留原质量/continuity。playback `record_samples` 又对每个存储块重复 observe 同一 callback reading：首块检测倒退并降为 Unknown，后续块因为 callback 相等而错误恢复 BackendEstimate。owner 的 sample 独立回退及 2052 stereo 样本三块 RED 都直接对应生产入口，适合作为有界修补依据。

建议与现有 capture 路径一致，每个实际 batch/成功接纳前缀只 observe 一次，然后复制该 timing 切块并推导 offset；native 原始 callback/sample 锚点才进入上一观测缓存。比较严格小于，保留合法负 sample 起点和相等 callback；ALSA sample=None 不补造。offset checked_add 溢出可仅丢弃该 sample 估计，保留仍有效的 callback 观察；这属于部分证据，不要求全字段强制 None。

整批 observe 会先把 cursor 推进到批次末尾，但与既有 capture 相同。并发 marker 可以带末尾 cursor 先入队，原 validator 已允许 marker/PCM 的合法交错；不能因此重写 PCM 的原连续性或丢掉它。CPAL runtime 字段先 drop Stream 再 drop clock owner，ALSA owner 在处理线程/PCM 结束后发布 Closed；closed 成功记录/帧数下界不依赖每块推进时机。只要保持现有停止后发布 final 的所有权，新增整批观测不会使 loss 被误作实际静音。

基线离线 replay 只依赖 stream/sequence/frame/continuity/loss；历史合法文件中即使 quality、callback/sample 可用性改变，或其值倒退，同 stream 连续 PCM 仍可能得到空 resets。owner 提议新增 `ClockDiscontinuity`，只比较同 stream 上一条实际 PCM、跳过无时钟的 lifecycle marker，不拒绝历史文件、不改证据 schema、不更改 PCM。独立判断此范围合适：这是消费侧不能沿用对齐状态的提示，不是已经证实设备时钟故障、声学中断或丢帧。

必要正例包括相等 callback、负 sample 起点、同批分块、marker 的 Unknown 不额外引发 clock reset、质量/可用性恢复、独立方向/重开；queued_frames 变化或其可用性、host 观察时间不应单独生成 clock reset。即使下批原始 sample 时间早于上块按名义率推导的时间，该提示也只能描述元数据不连续，不能宣称测得设备漂移。RED、候选与实际结果由两 owner/Root 后续提供。

## 硬件资料是否阻塞本轮

现有用户事实仍是 R1 尚未接入，计划 USB；支持硬件 AEC 的陈述不证明固件配置已启用或参考电气已接入。扬声器/功放未定，板卡/固件版本、稳定设备 ID、原生格式/声道含义、参考输入路由和真实延迟仍未验证。软件 postmix tap 的存在也不证明 R1 已收到参考。

这些缺失不阻塞本轮本地源码审计、条件契约或合成回归，不必重复要求用户提供同一资料才完成当前工作；它们阻塞真实硬件 AEC 模式、是否需要主机算法、实际帧/通道选择、漂移阈值调优及声学验收。保留 Pi5 8G + macOS server 当前范围，R03 暂停；不因此扩展通用设备平台或 R05 长稳。

## 已读回的本轮 RED 与回放候选

已独立读取 `run/validation-20261014-r02-clock/red-clock-contract.json` 和完整原始日志。Root 运行 no-default library 三项定向用例，exit 101、11.08 秒，**0/3/0**，247 filtered out；清单 SHA-256 为 `f492372c783673beac85b356b3e0d01fdd1e4b165ceed22d679a77d5d9816272`，`source_drift=[]`。三项都是预期运行期断言，非编译失败：sample 回退未改变 continuity；三块质量实际为 `[Unknown, BackendEstimate, BackendEstimate]`；历史文件八类 quality/可用性/时间值变化均未给出 reset。Root 据此批准两 owner 修补。

已读回 replay GREEN：`ClockDiscontinuity` 在原 FrameGap 的同 stream 前一 PCM 查找内追加，独立比较 quality、callback/sample 是否存在和严格倒退；marker 不作为时钟观测。没有修改 validator、JSON schema 或原 PCM/时间。RED 现在精确要求该 reset 而非任意非空 resets。两项正例涵盖交错 marker 的 Unknown/末尾 cursor、相等 callback、合法负 sample、host 时间回看、ALSA delay 数值及可用性变化、双向交错和同向换流；保留既有 StreamChanged/Discontinuity，未额外制造 clock reset。静态审查无阻断，尚不声称 Cargo 通过。

Root 的完整条件契约节已独立读回。建议将 rubato 内部时长表述为“约 10 ms”，并明确现有 converted speech 不自动经过 reference consumer、硬件 AEC 是否需要叠加软件算法尚未决；Root 已回填。仿射映射和 ppm 均明确是条件定义，未输出伪实测结果、未猜阈值或建立 consumer 框架。

随后完整读回 producer 最终 GREEN。`last_sample_ns` 与 callback 一起按真实 batch 锚点比较；正常缺失保留历史，回退后以该批原始读数建立新段后续比较基准。私有 `publish_native_samples` 只接受已观察 timing 与不可变 probe，统一 capture/playback 的帧对齐、offset、归一化和 publish；不二次 observe、不新增堆分配，不触及 native owner 或取消入口。playback 零完整帧仍直接返回，残帧错误标记仍保留。

五项新增 clock 回归（RED 两项、正例三项）覆盖三块一致的 Unknown/continuity/host observation、单个 ClockRegression marker、下一批恢复；负/相等/缺失恢复、ALSA sample=None、重开独立历史；下一真实 sample 锚点早于前批派生末块时间时不误用派生点；capture/playback 两向 offset 溢出只失去 sample 估计，保留 callback/quality。结合 replay 三项新增测试，最终源码静态审查无阻断。reader、queue publish sequence/loss、精确取消、owner 关闭和导出资源代码保持原有实现；本分工未替 owner 编辑。

## 最终运行证据独立核对

已读取本轮 `run/validation-20261014-r02-clock/` 的最终清单、五次 final wrapper JSON、相应原始日志、static、sandbox failure analysis、replay smoke 脚本/摘要及两份实际 stdout/stderr。用纯文件读取独立重算 `final-source-manifest.json` 原字节 SHA-256 为 **`fb4a641051ef77125d788d7013487dcc7fda488e43c74044a0690c5767144fe7`**，**666 个输入**逐项 hash 均匹配；最终各 wrapper 使用同一清单，`source_drift=[]`，当前输入亦无漂移。本分工没有再次执行测试或二进制。

首次 `final-embedded` 在沙箱内 exit 101、51.80 秒，lib **212/43/0**；43 项均在三处已有 fixture 的 `TcpListener::bind(127.0.0.1:0)` 遭 EPERM，分别 playback21、body9、speech13。新增八项已通过。原始失败日志和分类保留，不把它当作产品 RED，也不累计为完整通过。Root 随后在允许本机 loopback 的上下文中同源完整重跑，没有改源码或跳过用例。

| 实际执行 | 独立核对结果 | 范围说明 |
| --- | --- | --- |
| `final-embedded-loopback` 默认 all-targets | exit 0、68.75 秒；lib255 + integration75 + example5 = **335/0/0** | 14 个目标结果行，包含 main0；新增八项与原 offline_evidence 四项逐项通过 |
| `final-no-default` lib + example | exit 0、39.55 秒；255+5 = **260/0/0** | 同一批在另一 feature 配置下重复验证，不加成260个独立新增测试 |
| `final-cli-build` no-default example | exit 0、1.49 秒 | 真实构建，随后 smoke 使用该二进制 |
| `final-replay-smoke` | exit 0、0.60 秒；两项输入均保持原字节 | 合成 clock transition 和已归档历史证据只读 replay |

两种 feature 的日志均逐项核对 evidence18、native clock8、reference12 全部通过；这些是完整测试集合的子集，不重复相加。精确取消、显式 hold、observer cancel/drop、overflow/旧流 unknown 的四项已有组合回归仍通过。没有由 Mac 测试声称 Linux/ALSA 实机、Pi、R1 或硬件 AEC 已验证。

独立核对 smoke 二进制 SHA-256 为 `95d85a4024657f97eeab630d8e9285df923bca04b265b6960a519062917dccd2`，与记录一致。新 clock-transition fixture SHA-256 为 `d65f4e96880c22428405fde6f680b751e9b6c8eb5d647b65f2889cf789f68264`，实际摘要为 **3 records /12 samples /3 resets**：FirstObservation1、ClockDiscontinuity2，覆盖估计丢失与恢复；无 latest snapshot 被补造。历史 fixture SHA-256 仍为 `374d84a56124ab88aecf40aaaa8225c2df6f813b3c860e9061efa953a38d1b49`，实际摘要保持 **7/16/9**。两份输入当前 hash 与执行前记录一致，stderr 均为空，`all_assertions_passed=true`；摘要保持 `source_authenticated=false`、`physical_stream_complete=false`。新合成输入有意保留以便复现，不属于未清理临时资源。

`static.json` 与日志显示四份变更 Rust scoped fmt 和 diff check 均 exit0。workspace fmt exit1 仅为历史未修改的 Server `tests/server_runtime/delivery.rs` import 顺序，Root 的 baseline 比对为 unchanged_from_HEAD=true；该日志单独保留，本轮没有借机修改 Server。该项不冒充 workspace fmt 全过。

## 最终结论与后续边界

独立审查无阻断项。三个已实证的时钟证据反例完成修补，真实采样事实、原 PCM、证据 v1、默认不自动落盘、单 reader、精确取消和 native owner 资源边界保持。当前产物是更明确的时间证据分段与消费 reset；不提供采播已校准映射、ppm 实测、AEC 质量、物理停声或硬件启动结论。

R1 尚未接入与参考路由/扬声器/固件/原生格式未确认等现场事项仍保留，R03 继续暂停。没有新增 consumer worker、组帧服务、依赖、模型、设备动作、外部付费探测或平台验证。Root 后续正常提交并完成证据归档与提交 blob 核对；本分工已完成7/7并停止编辑。

## 资料与执行边界

仓库依据：`crates/alia-core/src/audio_clock.rs`、Embedded `module/context/audio_reference.rs`、`cpal_audio/clock.rs`、CPAL/ALSA backend、capture converter、playback resampler、speech duplex、Sherpa VAD、audio_evidence、Cargo.toml/Cargo.lock；现行 `audio-reference-clock-design.md`、`hardware-deployment-readiness.md`、硬件 audio HANDOFF 和 R02 Mac checkpoint。

固定依赖依据：本机 Cargo registry 中 rubato 5.0.0 的 `src/lib.rs`（output_delay/Adjustable）和 `src/asynchro.rs`（new_sinc 参数），只读核对，不安装/更新依赖。未引用未选 WebRTC/Speex 算法的具体帧要求，没有 Web 请求。此前 R02 Mac 已通过的 327/0/0 和 252/0/0 只作为历史恢复证据，不作为本轮测试结果。
