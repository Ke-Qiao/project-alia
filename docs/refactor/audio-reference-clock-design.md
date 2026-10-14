# 实际播放参考流与独立音频时钟

状态：2026-10-14，R02-A与Mac离线模拟已完成，最新独立时钟审计统一native批次锚点、补历史回放时钟变化提示，并明确未来AEC consumer的帧/对齐契约，见[R02 clock checklist](checklists/r02-clock-2026-10-14.md)。本文末尾2026-09-20平台记录仅为历史，本轮不宣称Linux、声学AEC、双讲或物理播放验收。

## 证据从哪里产生

参考流位于嵌入端实际音频后端。CPAL 在输出回调写完最终原生格式缓冲区后复制样本，因此包括重采样、声道转换、增益以及最终格式量化的结果。浮点后端的实际值通过 f64 保留；整数后端按实际整数表示归一化。不能用待播放的原始 TTS PCM 替代这份参考。

R02-A 将 native capture 加入同一队列：CPAL 记录输入 callback 实际交付的完整帧；ALSA 记录成功 readi 的完整帧前缀。位置在软件转换和增益之前。每批只推进一次原生游标，固定块拆分不二次 observe，也不改变既有 speech 的转换后 CaptureTiming。记录保留 native 样本容器类型和归一化 f64 自身的 bits；宽整数转换可能已损失低位，不能声称 i64/u64 原始字节无损。设备固件可能先做过 AEC/降噪，因此这里也不宣称原始 ADC。

ALSA 只在 `writei` 成功后记录实际接纳的完整帧前缀。部分写不会记录未写入尾部；取消后迟到的成功写入仍是原身份的已接纳事实。读取时间戳失败只降低时钟质量，不丢弃这段前缀。随后 `start` 失败会留下故障记录，也不撤销此前已经接纳的样本。

边界沿用 `CpalCallbackBuffer` 和 `AlsaWriteAccepted`。它们证明后端提交，不证明 DAC 已呈现、扬声器发声、用户听到或取消后声学尾音已经消失。本包没有改写现有 Delivery/Proto 语义。

## API、队列与生命周期

`AudioBus::with_boot` 使用现有 embedded boot ID；每次原生采集或播放启动建立独立 stream UUID。`AudioPlaybackBackend::reference_bus()` 暴露后端参考总线，不支持该证据的实现返回 `None`。`AudioFrame::timing` 为可选的真实采集批次元数据；旧 `timestamp_us` 仍是转换后样本游标对应的时长，不是墙钟或物理采样时间。

`AudioReferenceBus::try_claim_reader()` 排他取得非 Clone 的 `AudioReferenceReader`，只有 reader 的 `try_read(&mut self)` 能取走数据。失败的 claim 不读取队列，reader drop 仅释放读取权，不关闭 native stream、不取消播放、不清空剩余块。队列固定为 64 块，每块固定存储 1024 个 f64 样本并按完整声道帧分块；capture 与 playback 共用原容量。生产者只尝试取得队列锁，不等待消费者，不为每次参考记录分配堆内存。模型推理与文件 I/O 不进入回调路径。

每块包含 boot/stream 身份、格式、完整帧游标、连续性序号、时钟质量、后端边界、参考块序号以及此前丢失的块数和帧数。软件静音仍属于真实媒体；因缓冲不足实际填零和媒体/填零混合分别记录。CPAL 缓冲锁争用时，记录真正写出的静音，并留下断连续性标记；不能把丢失的参考块推断为静音。

`snapshot(direction)` 返回该方向**最新原生流**的独立原子统计。队列溢出、锁争用以及最后一个关闭标记被丢弃时，即使没有后续 PCM，仍可观察该流最终的丢块/丢帧事实。关闭顺序是原生 owner 先停止和析构，再发布最后标记尝试及丢失计数，最后以 Release 发布 `Closed`；读取者先 Acquire 该状态再读取最终计数。清理用的 `Cancelled` 不会覆盖之前的实际故障原因。

重新打开会替换该方向的旧快照。消费者遇到新的 stream ID 必须重置，不得将旧、新 cursor 拼接为连续音频。本接口不保存设备全生命周期的每个历史流或累计诊断账本。

显式证据会话可以持有此 reader，并在本地诊断上限内拉取、结束和导出。没有自动会话、落盘或分离的写文件任务；正在运行的 native 后端仍会在上述固定内存队列中交接观测。停止观测不构成设备停止，最新快照不能补回已被换流覆盖的旧 final 状态。具体格式、重放和合成示例见 [音频使用说明](../../packages/alia-embedded/docs/audio-and-voice.md#debugging-and-limits)。

成功块在队列临界区读取此前 loss 快照，再分配 sequence，确保已入队记录顺序单调，同时避免并发失败计数超前于该块序号。失败的 try_lock/满队列仍计 attempt 与 loss，不等待消费者。错误标记的 timing 可能先于已观察的 PCM 入队，因此 replay 保留原 timing 并输出不连续信号，不改写时间或丢掉合法 PCM。活动流快照的原子字段不是同一时刻的事务，只有 Closed 的最终统计可按关闭协议校验。

## R02-A 会话与离线使用

`module::audio_evidence::EvidenceSession::start` 显式取得 reader，`pump(max_records)` 每次拉取有界数量，`finish` 返回 `AudioEvidence` 并释放读取权。会话 drop 同样释放读取权，持有的取消 token 只用于停止观测，不调用 native cancel/close。没有后台线程、定时任务或自动录音配置。旧 `[audio_debug]` 表须从运行配置移除；现有文件加载和 settings 更新会拒绝它，包括 `enabled=false`。

软件诊断硬上限为 512 个记录、每记录 1024 个样本、总计 524288 个样本、JSON 16 MiB；调用者可进一步收紧 `EvidenceLimits`。达到限额、取消或遇到非法记录有明确终止原因，被弹出但未保留的块另记 discarded；未读队列和之后产生的数据仍属未知。每个记录保留格式、native 样本容器、归一化 f64 bits、边界、boot/stream、sequence、continuity、loss、原生游标、clock quality、可用 delay 和主机观察时间。f64 bits 包括负零，不经小数 JSON 舍入。

`write_json`、`read_json` 与 `replay` 均为同步调用者所有；读写逐块检查取消，但不能中断已进入的 OS 阻塞调用。导入先限字节和数组大小，再验证 v1 身份、格式、边界、样本长度、有限值、时钟/静音组合、序号及关闭快照；未知字段、截断或不合法组合明确失败。最新校验对Closed再要求已观察成功块数不超过attempted−lost_blocks、已观察PCM帧数不超过cursor−lost_frames；使用checked运算且只取下界，允许缺失前缀/未读尾部/marker错序，不强化活动快照。通用 writer 出错时部分目标由调用者处置，不声称完整导出。合成示例使用临时文件、同步和不覆盖的发布方式，不会覆盖已有文件。

从仓库根目录执行以下命令可生成并重放一份**纯合成**证据；输出父目录必须已存在，文件必须是新路径：

```sh
cargo run -p alia-embedded --no-default-features --example audio_evidence_replay --offline --locked -- generate /tmp/alia-synthetic-evidence.json
cargo run -p alia-embedded --no-default-features --example audio_evidence_replay --offline --locked -- replay /tmp/alia-synthetic-evidence.json
```

`replay` 只返回独立双流记录和 reset/gap 信号，不播放音频、不消费真实设备、不补零、不建立采播配准。遍历结束后仍必须检查 `observed_latest`、观测终止原因、discarded 和 `unobserved_history_possible`：最后 close 块可能丢失，而方向快照可能已替换旧流。工具严格区分生成与只读重放，拒绝旧隐式参数及额外参数；摘要按实际输入给出各stream记录/帧/样本/reset计数、是否观察到关闭标记及匹配的最新快照，不再硬编码固定fixture状态。空记录、单方向、仅footer都有效；没有自己的关闭标记或匹配closed快照的旧流final保持未知。输入要求普通文件并拒绝末级symlink/特殊文件，沿用有界read_json；输出仍不覆盖。`ObservedNative` / `SyntheticFixture` 是来源声明，不是文件认证或硬件证明；格式合法也不意味着内容可信。

## 时钟和缺口的解释

采集与播放各自拥有原生帧域和时钟 ID。即使它们碰巧使用同一设备、相同采样率或同一主机时钟，本包也不据此宣称共享采样域。共同的主机单调观察时间只用于关联诊断，不是校准后的样本对齐关系。

CPAL 使用其 `InputCallbackInfo`/`OutputCallbackInfo` 的实际时间戳 API，质量为后端估计。ALSA 显式申请单调时间戳，仅记录状态观察和 queued delay；不会据此生成某个样本的物理呈现时间。时间戳缺失时为 `Unknown`，倒退时推进连续性并降低该次质量，但不回退实际帧游标。

转换后的采集批次保留其实际贡献的原生输入批次及转换后游标。发生重采样时，这不等于已经建立“第一个原生样本到第一个转换后样本”的精确映射。重采样器跨块尾部、滤波延迟和两端设备漂移仍需后续显式建模和测量。

ALSA `wait` 和 `readi` 采用同一错误路径。`EAGAIN`/`EINTR` 不伪造缺口；采集 `EPIPE` 标为 `Overrun`，播放 `EPIPE` 标为 `Underrun`。采集缺口先清除转换器历史，再执行一次 `prepare/start`；失败结束该 owner。该路径避免 `snd_pcm_recover` 在挂起恢复中无限循环而阻住停止/等待退出，其恢复循环见 [ALSA 官方实现](https://github.com/alsa-project/alsa-lib/blob/master/src/pcm/pcm.c#L8271)。不可恢复的等待、读取、启动和停止错误保留 `BackendError`。

## 实现范围与后续主线

本包新增的播放参考交接采用固定存储和不等待的锁尝试。**既有采集转换器和 broadcast 路径仍会分配及加锁**，不能由此声称所有生产音频回调已经满足硬实时要求。显式 ALSA 与 Linux 回退路径共用同一采集实现，避免两份实现产生不同的证据语义。

后续 AEC 主线需要实际接入一个拥有参考消费者的音频工作线程，定义丢块/重开时的 reset，建立重采样延迟及采集/播放漂移映射，测量物理输出延迟与设备取消尾音，并完成树莓派 5 上的回声和双讲实验。应使用已核验的依赖和真实硬件结果决定抑制、双讲保真及重收敛策略。这些尚未实现，不能把本包的后端接纳或软件空设备测试作为完成证据。

本包没有引入 AEC 依赖、完整 WAIT 恢复或 Companion，也不推进 Web/iOS/Android 和通用嵌入适配的延期范围。

## R02 时钟、缺口与 consumer 契约（2026-10-14）

本轮先审计既有实现与证据，再修复可复现缺口，进度见[R02 clock checklist](checklists/r02-clock-2026-10-14.md)。以下区分现有数据能证明的事实与未来AEC接入必须满足的条件；没有选择或启用AEC算法、重采样漂移控制器或硬件路径。

### 时钟域和锚点

- 一个native流的身份为boot/stream/direction；采集和播放拥有独立stream、clock origin和帧游标。相同设备名、采样率或数值相等的callback/sample时间均不能证明跨流对齐。重开建立新身份，旧模型不能自动继承。
- `start_frame/frames`数的是该边界实际观察或接纳的完整声道帧。一个帧含`channels`个交错样本；xrun造成的未知物理帧不补进cursor。转换后的capture游标与native游标分属不同域，converter尾部及滤波延迟没有校准映射。
- CPAL的callback时间与本批首样本时间是后端估计，允许首样本相对本流origin为负；ALSA只保留status观察与queued delay，`sample_ns=None`不补造。单次队列延迟不是固定声学延迟，也不得沿用为后续读数的已知值。
- 一个native输入批次或成功接纳的输出前缀必须只观察一次时间并推进一次总cursor；切成1024样本存储块只复制批次质量/连续性并按名义采样率平移sample offset。拆块不构成新的独立测量，不可用这些派生点反推设备的真实采样率或漂移。
- callback或sample锚点倒退表示时间证据需要分段；该批质量降为Unknown，实际PCM和帧游标保留。callback相等、合法负sample offset、ALSA缺少sample锚点不自动构成倒退。Unknown不等于零时间，也不证明PCM丢失。
- `observed_boot_ns`是同一总线起点的主机观察时间，包含调度/回调/采样位置差异；用于关联诊断，不能直接作为采集或DAC呈现时间。

### 漂移和对齐的条件

未来需要对一对固定的连续capture/playback片段建立可失效的映射。可用`p = a*c + b`描述捕获帧c对应的播放参考帧p：a表示两个采样域的帧率比例，b表示该片段的偏移；正的名义比例为`Fp/Fc`，并非实测a。相对名义比例的漂移可写为`(a/(Fp/Fc)-1)*1e6 ppm`，但当前不输出该估计值，也不以相同名义频率代替测量。

求a需要分离独立的原始锚点与按名义速率生成的块内offset，并约束观测误差；求b还需要已确认的参考路由、采样/呈现时刻含义、转换延迟和物理延迟依据。v1证据没有已校准的跨流锚点、精度界或算法收敛证明。当前不设置猜测的ppm阈值、等待毫秒数或声学延迟，不因为数学可拟合就宣称对齐有效。

映射只在对应身份、format和continuity片段内有效。时间质量/可用性丢失或恢复、锚点倒退、reference缺口、流重开、转换reset均要求consumer使相关映射失效；恢复数据只允许重新建立证据，不能静默复用旧系数。缺失或不可比时明确保持unknown。硬件AEC由设备内部对齐时，也不能借此给主机两个独立软件流追加共享时钟声明。

### 缺口、重放与帧需求

现有队列的sequence gap、producer loss、PCM frame gap、continuity变化和显式marker分别保留原始含义；它们不是可互换的丢失样本数量。队列尾部无后续记录时还须看latest closed快照；被换流覆盖且缺少关闭标记的旧终态未知。真实HeldSilence/ZeroFill保留其实际零样本，缺失记录不补零、不拉伸、不拼接为连续帧。

回放应明确给出时钟证据变化提示，保留合法历史记录的原时间与PCM，不通过拒绝整个旧文件掩盖异常。时钟提示比较同stream相邻实际PCM；生命周期marker本来没有时钟，不把每个marker的Unknown误当独立测量。时钟提示说明对齐状态不可沿用，不自动声明声学中断或丢帧；consumer结合原有gap/reset决定其算法状态。逐条sample时间可能含前一批名义分块offset，所以回放的保守提示也不等于生产端原始batch锚点倒退。

| 现有粒度 | 实际用途 | 对AEC的约束 |
| --- | --- | --- |
| 最多1024个交错样本/块，64块采播共用 | 有界native参考交接；帧数为样本数除声道数 | 不是固定时间窗；例如48kHz双声道满块只有512帧 |
| 16kHz mono、每上传块最多1600样本 | speech网络发送；回调短尾不补齐 | 是最多100ms的传输块，不是已选AEC帧 |
| Silero `window_size` | 当前VAD推理缓冲 | 与回声处理帧长分别确定 |
| playback rubato `input_rate/100`内部块 | 既有固定比率重采样 | 约10ms（整数截断）只属于该wrapper，不能据此宣称AEC要求10ms |

仓库目前没有AEC processor/feature或自动reference worker；R1尚未接入，内部AEC参考接法与格式未确认，是否还需要主机软件AEC也未决定。既有rubato固定比例wrapper未启用动态ratio调节，不能算作漂移补偿。实际consumer确定后，必须明确支持的采样率/声道、帧时长、输入/输出含义、render先后次序、可接受延迟/等待上界、尾帧处理及gap后的reset/重收敛规则。只在同一有效连续片段内累积完整帧；不足一帧时按已选consumer规则保留或结束，禁止用未知PCM补齐。重采样转换需有其延迟/尾部与映射证明，不能直接把不同采样率的数组配成一对。

现有speech仍从独立的converted AudioBus进入VAD/KWS/upload；只取得reference reader不会让该路径自动经过AEC。实际consumer确定后才选择native或converted接入点及其输出/延迟契约。

consumer取得现有唯一reader，拥有自己的有界组帧、映射和算法状态；未来诊断分支如有需要应从该owner的有界输出取得，不能竞争第二个native reader。取消/释放消费权只停止消费并清理自身缓存，不能替代精确播放取消或关闭native owner；外部模块、模型与文件I/O仍不得进入原生回调。该接入条件不是本轮新增运行服务或预留框架的授权。

## 历史验证依据：2026-09-20

恢复记录保存在 `run/recovery-20260920/audio/`，来自本 agent 已执行的纯文件修改；没有重放原 shell 的联网、Docker、删除或 Git 动作。恢复后重新测试，历史通过次数不作为本包最终验收。

最终 macOS 无默认 features 的 `cargo check --all-targets` 和库测试已通过，库测试 124 项。默认 features 的库测试也已实际编译、链接并通过 124 项，使用官方 build.rs 支持的 `SHERPA_ONNX_ARCHIVE_DIR` 复用本地 v1.13.1 arm64 原生库归档；不是 `DOCS_RS` 跳过链接。Linux ARM 无默认 features 的 `cargo check --all-targets` 及库测试通过，库测试 130 项，其中 6 项验证 ALSA 原生调用/明确注入的错误边界。

Linux 使用固定 ARM64 镜像 `rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1`。只读构建源码清单包含 597 个文件，SHA-256 为 `503f32ba40b55419ac5f7bc320b6900d85bd19071d6e426426018d99a2ee8d53`；本验证说明在冻结构建源码后新增，不属于该构建清单。Cargo.lock SHA-256 为 `0736653b5c57a936dc71d2b18a82b927536acb7bb8c49341f845717c87fde2a2`。依赖 registry 只读，target/cache 为该任务私有目录。

最终 Linux 日志与编译器、系统包版本位于 `run/recovery-20260920/audio/validation/evidence-v2/`，`result.txt` 为 `check=0`、`test=0`。macOS 对应 `macos-check-final.log`、`macos-lib-final.log` 和 `macos-default-lib-final.log`。前期缺失 Protobuf 头文件及不恰当软件 null fixture 的失败日志也保留，最终测试未跳过任何用例。本包只新增一次有界的软件验证，不宣称执行了新的 Linux 默认 features/Sherpa 运行时验收。

测试覆盖最终格式量化、静音/填零、部分写及取消、参考队列溢出/争用、最后关闭统计、流重启、时间倒退与转换器缺口重置。ALSA 的软件 `null` PCM 测试调用真实 libasound，验证写入前缀、写后启动失败与采集停止；等待失败及 xrun/suspend 的错误身份显式注入同一生产处理入口，实际 `prepare/start` 和 native owner 关闭使用软件设备。软件 null 没有硬件节奏，也不能用于证明物理断连时的 wait 行为。没有树莓派、DAC、声学 AEC 或双讲实测结论。
