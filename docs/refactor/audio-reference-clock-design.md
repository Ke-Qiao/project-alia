# 实际播放参考流与独立音频时钟

状态：2026-10-14，R02-A 在原软件基础上推进 native 双流有界证据与离线重放；本轮最终验证见 [R02-A checklist](checklists/r02-a-2026-10-14.md)。本文末尾 2026-09-20 的平台记录保留为历史证据，不代表本轮 Linux 验证。该接口不声明已经完成声学回声消除、双讲质量或物理播放验收。

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

`write_json`、`read_json` 与 `replay` 均为同步调用者所有；读写逐块检查取消，但不能中断已进入的 OS 阻塞调用。导入先限字节和数组大小，再验证 v1 身份、格式、边界、样本长度、有限值、时钟/静音组合、序号及关闭快照；未知字段、截断或不合法组合明确失败。通用 writer 出错时部分目标由调用者处置，不声称完整导出。合成示例使用临时文件、同步和不覆盖的发布方式，不会覆盖已有文件。

从仓库根目录执行以下命令可生成并重放一份**纯合成**证据；输出父目录必须已存在，文件必须是新路径：

```sh
cargo run -p alia-embedded --example audio_evidence_replay --offline --locked -- /tmp/alia-synthetic-evidence.json
```

`replay` 只返回独立双流记录和 reset/gap 信号，不播放音频、不消费真实设备、不补零、不建立采播配准。遍历结束后仍必须检查 `observed_latest`、观测终止原因、discarded 和 `unobserved_history_possible`：最后 close 块可能丢失，而方向快照可能已替换旧流。示例报告同时读取 footer，明确尾部损失与被替换流的 final 未知。`ObservedNative` / `SyntheticFixture` 是来源声明，不是文件认证或硬件证明；格式合法也不意味着内容可信。

## 时钟和缺口的解释

采集与播放各自拥有原生帧域和时钟 ID。即使它们碰巧使用同一设备、相同采样率或同一主机时钟，本包也不据此宣称共享采样域。共同的主机单调观察时间只用于关联诊断，不是校准后的样本对齐关系。

CPAL 使用其 `InputCallbackInfo`/`OutputCallbackInfo` 的实际时间戳 API，质量为后端估计。ALSA 显式申请单调时间戳，仅记录状态观察和 queued delay；不会据此生成某个样本的物理呈现时间。时间戳缺失时为 `Unknown`，倒退时推进连续性并降低该次质量，但不回退实际帧游标。

转换后的采集批次保留其实际贡献的原生输入批次及转换后游标。发生重采样时，这不等于已经建立“第一个原生样本到第一个转换后样本”的精确映射。重采样器跨块尾部、滤波延迟和两端设备漂移仍需后续显式建模和测量。

ALSA `wait` 和 `readi` 采用同一错误路径。`EAGAIN`/`EINTR` 不伪造缺口；采集 `EPIPE` 标为 `Overrun`，播放 `EPIPE` 标为 `Underrun`。采集缺口先清除转换器历史，再执行一次 `prepare/start`；失败结束该 owner。该路径避免 `snd_pcm_recover` 在挂起恢复中无限循环而阻住停止/等待退出，其恢复循环见 [ALSA 官方实现](https://github.com/alsa-project/alsa-lib/blob/master/src/pcm/pcm.c#L8271)。不可恢复的等待、读取、启动和停止错误保留 `BackendError`。

## 实现范围与后续主线

本包新增的播放参考交接采用固定存储和不等待的锁尝试。**既有采集转换器和 broadcast 路径仍会分配及加锁**，不能由此声称所有生产音频回调已经满足硬实时要求。显式 ALSA 与 Linux 回退路径共用同一采集实现，避免两份实现产生不同的证据语义。

后续 AEC 主线需要实际接入一个拥有参考消费者的音频工作线程，定义丢块/重开时的 reset，建立重采样延迟及采集/播放漂移映射，测量物理输出延迟与设备取消尾音，并完成树莓派 5 上的回声和双讲实验。应使用已核验的依赖和真实硬件结果决定抑制、双讲保真及重收敛策略。这些尚未实现，不能把本包的后端接纳或软件空设备测试作为完成证据。

本包没有引入 AEC 依赖、完整 WAIT 恢复或 Companion，也不推进 Web/iOS/Android 和通用嵌入适配的延期范围。

## 历史验证依据：2026-09-20

恢复记录保存在 `run/recovery-20260920/audio/`，来自本 agent 已执行的纯文件修改；没有重放原 shell 的联网、Docker、删除或 Git 动作。恢复后重新测试，历史通过次数不作为本包最终验收。

最终 macOS 无默认 features 的 `cargo check --all-targets` 和库测试已通过，库测试 124 项。默认 features 的库测试也已实际编译、链接并通过 124 项，使用官方 build.rs 支持的 `SHERPA_ONNX_ARCHIVE_DIR` 复用本地 v1.13.1 arm64 原生库归档；不是 `DOCS_RS` 跳过链接。Linux ARM 无默认 features 的 `cargo check --all-targets` 及库测试通过，库测试 130 项，其中 6 项验证 ALSA 原生调用/明确注入的错误边界。

Linux 使用固定 ARM64 镜像 `rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1`。只读构建源码清单包含 597 个文件，SHA-256 为 `503f32ba40b55419ac5f7bc320b6900d85bd19071d6e426426018d99a2ee8d53`；本验证说明在冻结构建源码后新增，不属于该构建清单。Cargo.lock SHA-256 为 `0736653b5c57a936dc71d2b18a82b927536acb7bb8c49341f845717c87fde2a2`。依赖 registry 只读，target/cache 为该任务私有目录。

最终 Linux 日志与编译器、系统包版本位于 `run/recovery-20260920/audio/validation/evidence-v2/`，`result.txt` 为 `check=0`、`test=0`。macOS 对应 `macos-check-final.log`、`macos-lib-final.log` 和 `macos-default-lib-final.log`。前期缺失 Protobuf 头文件及不恰当软件 null fixture 的失败日志也保留，最终测试未跳过任何用例。本包只新增一次有界的软件验证，不宣称执行了新的 Linux 默认 features/Sherpa 运行时验收。

测试覆盖最终格式量化、静音/填零、部分写及取消、参考队列溢出/争用、最后关闭统计、流重启、时间倒退与转换器缺口重置。ALSA 的软件 `null` PCM 测试调用真实 libasound，验证写入前缀、写后启动失败与采集停止；等待失败及 xrun/suspend 的错误身份显式注入同一生产处理入口，实际 `prepare/start` 和 native owner 关闭使用软件设备。软件 null 没有硬件节奏，也不能用于证明物理断连时的 wait 行为。没有树莓派、DAC、声学 AEC 或双讲实测结论。
