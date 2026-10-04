# R02：音频参考、原生缺口与当前硬件信息

日期：2026-10-04。审查基线为 `8c1742287b75720a2530711ce78c447679369884`（R04 源码 `8f5bb83`）。本轮先查真实音频路径及既有 fixture，只修复已确认的原生采集连续性缺口；没有接入新的 AEC 算法或依赖，没有执行录音、播放、执行器动作或 SSH。最终源码身份与统一验证由本目录总交接记录关联。

## 本轮修复

原 `speech::duplex::CaptureClock::accept` 只检查转换后的 `timestamp_us`。ALSA xrun 的现有处理会增加 native continuity、重置转换器并重新启动采集，但 `emitted_frames` 继续累加；因此原生缺口没有被 server_turn 语音消费者发现，旧会话可能继续使用不连续的音频。

当前 CaptureClock 验证 `AudioFrame.timing` 的采集方向、stream/boot 身份、continuity、native format、clock identity，以及 16 kHz mono 转换格式、帧数和 converted cursor。已经开始的流也拒绝 timing 有无切换。纯软件 fixture 的连续 `timing=None` 仍明确属于没有原生证据的输入。未知 native timestamp 仍合法；实现不把 native/converted cursor 当作精确映射，不直接相减两个设备时钟。

发现缺口后沿原有 damage 分支退休 capture、关闭原输出、清除 pre-roll 和待处理音频，重置 VAD，并要求新唤醒。新增的 `WakeWordEngine::reset` 由现有 Sherpa `spotter.reset` 实现，Runtime 初次启动及损坏会话清理时调用，防止缺口两边的部分唤醒词重新拼接。noop 和既有测试实现同步补齐该方法；没有引入新的音频状态机。

此修复限于当前自然全双工的 `server_turn` 路径。旧 `utterance` 模式的分段/调试机制没有借本轮扩张重写；其行为不能替代 R02 自然双讲验收。

改动范围：`module/speech/duplex.rs`、同目录 `tests.rs`、`module/traits.rs`、`module/sherpa.rs`、`module/noop.rs`、既有 `tests/runtime.rs`。

## 现有音频链与证据边界

| 环节 | 实际实现及已有测试 | 尚未证明的内容 |
| --- | --- | --- |
| 播放转换 | `Renderer::render` → 既有 rubato 固定比 sinc 重采样 → mono 扩声道 → sink 应用增益；单播放保持相位/滤波历史，End 排尾。`resampler.rs` 的 packet-boundaries、impulse/tail、spectral-quality 测试覆盖转换 | 固定比重采样不是两设备漂移补偿 |
| CPAL reference | `fill_output_buffer` 写出最终 native 格式后记录；既有 `reference_matches_post_resample_gain_and_native_i8_quantization`、`native_float_reference_preserves_the_actual_written_float_values`、`software_mute_and_callback_zero_fill_are_actual_samples_with_distinct_content` | 仅证明 callback buffer 内容，不证明 DAC 呈现 |
| ALSA reference | `write_alsa_chunk` / `confirm_alsa_write` 只记录 `writei` 成功接纳的完整帧前缀；缺少 timestamp 不撤销样本事实。既有 `alsa_reference_contains_only_accepted_prefix_even_after_cancel_or_missing_timestamp` | 不是声卡已播放或扬声器已静音 |
| 本地提示与混音 | direct response WAV 与 TTS 使用相同 native renderer/sink；受控 scope 拒绝 identityless media；`priority` mixer 明确未实现、`noop` 不混音 | 不能声明已将本地提示与 TTS 多源混音；reference 覆盖的是实际获准写出的输出 |
| capture → wake/VAD | native 样本转 i16/声道平均 mono → 当前采集转换器 → 目标声道 → capture gain → AudioFrame；server_turn 持续 VAD，再 wake/session 上传；播放事件不关闭麦克风 | 没有在 wake/VAD 前执行 AEC；当前采集线性转换也不是经真机声学验收的处理链 |
| 时钟域 | boot/stream/direction 独立标识；CPAL 提供 backend estimate；ALSA 为 monotonic status/queued delay，sample_ns 未知；`observed_boot_ns` 仅为同主机观察时间。既有 `capture_playback_and_restart_have_distinct_clock_and_sample_domains` / clock regression 测试 | 同采样率不代表共享时钟；跨域 offset/漂移未标定 |
| 队列、欠载与重开 | reference 固定 64 块 × 1024 样本，生产者 try_lock 不等待；snapshot 留最终丢块/丢帧；新 stream 不拼旧 cursor。既有 `full_reference_queue_and_lost_close_remain_visible_without_later_pcm` / `resumed_reference_reports_the_missing_cursor_range_without_inventing_silence` | 没有生产 AEC consumer；未知缺口不能补成真实静音 |
| 暂停/尾音 | hold 保留未提交媒体并记录实际静音；已进入的一帧可以结算，Resume 仅继续剩余部分。既有 `held_reference_is_actual_zero_and_resume_keeps_resample_gain_and_quantization` / `partial_alsa_submission_is_not_replayed_after_explicit_wait` | CPAL 软件清队列、ALSA drop/prepare 或网络 ACK 均不证明设备缓冲与声学尾音消失 |

依赖静态读取自 Cargo.lock：cpal 0.15.3、alsa 0.9.1、rubato 5.0.0、sherpa-onnx 1.13.1。没有 AEC 依赖。详细既有契约见[实际播放参考与独立音频时钟](../../audio-reference-clock-design.md)。

## 本轮测试映射

以下三项新增回归复用既有纯软件音频 / 实际 loopback gRPC fixture，没有声卡、模型或硬件动作。方法中的 xrun/restart 为明确注入的原生元数据变化，不是实物断连。

| 完整 Rust 测试名（alia-embedded lib） | 检验内容 |
| --- | --- |
| `module::speech::duplex::tests::native_xrun_or_restart_retires_active_output_before_fresh_wake` | 转换时间保持连续，分别改变 native continuity 和 stream；旧输出退休、损坏帧不进 VAD/上传；新 wake 建新 session，sequence/offset 从 0 开始 |
| `module::speech::duplex::tests::native_capture_loss_clears_partial_wake_before_accepting_new_audio` | 缺口前 prefix、缺口后 suffix 不能触发；新的完整两部分 wake 仍可连接并上传；使用 fixture 阶段计数同步，无放宽生产期限 |
| `module::speech::duplex::tests::native_capture_timing_validates_format_cursor_and_evidence_without_aligning_clocks` | format、converted count/cursor、direction、clock identity、resampling flag、元数据丢失/突然出现均拒绝；未知时钟和原生空转换批次不被伪判为跨域对齐失败 |

准备阶段各改动文件的 `rustfmt --edition 2024 --config skip_children=true` 与 `git diff --check` 均退出 0。本次 Cargo 由 Root 唯一验证窗口实际执行 `cargo test --workspace --locked --offline`，保留默认 features、默认测试并行度，build jobs 为 2，`RUST_MIN_STACK` / `DOCS_RS` 均未设置。退出码 **0**，完整运行 **541.76 秒**；外层结果 **1062 passed / 0 failed / 5 ignored**，已排除嵌套子进程摘要，不能额外累计本节测试。

| 本轮独立 macOS 实测目标 | 结果 | 证据位置 |
| --- | --- | --- |
| `alia-embedded` lib，日志中的 `alia_embedded-e7e0f3f5ffe0d9c6` | **167 / 0 / 0**，26.16 秒；三项新增测试分别为日志第 181、185、195 行，均 `ok` | [workspace-v1.log](../../../../run/validation-20261004/workspace-v1.log)、[计数原件](../../../../run/validation-20261004/workspace-v1-counts.json) |
| Embedded `tests/runtime.rs`，`runtime-dd520e0a2ab4d669` | **10 / 0 / 0**，0.08 秒 | 同一日志第 293—307 行 |
| 完整 workspace 调用 | exit 0，`source_drift=[]` | [命令/退出码原件](../../../../run/validation-20261004/workspace-v1-result.json) |

macOS 编译目标中既有 reference / resampler / 软件 native 提交与 hold 测试也在上述 lib 目标通过，默认 features 实际涵盖 Sherpa reset 编译链接。Linux 专属 ALSA 原生测试不在 macOS 的编译条件内；本节不将它们计作本轮 macOS 执行，Linux 当前 Embedded lib 174/0/0、runtime 10/0/0 已通过；包含平台专属 ALSA 模拟/null 边界，具体命令与身份见 [Linux HANDOFF](linux.md)，不等于打开真实声卡。

已只读核对原始 manifest/result/counts/log：冻结输入 **633** 项，manifest SHA-256 `a1c7dc3d6787cf2b86144b60c17b5aaaaad65025d1695a1326194c826c8ba0d0`；日志 SHA-256 `beb8172f600b72fdc923552809e19683f20c277be06c3b8eaef853052915ab47`。本次复核时对应测试二进制仍在原日志路径，SHA-256 分别为 `a64fb1808d13925f7e819a0df55eac792a059df20f9acbd7c52b68909f4d7237`（Embedded lib）和 `e8851e9de96424703996f558d8ad3be385be95efddc5aea459a985f8d41f99c9`（runtime）。本次默认 native archive 摘要为 `ca7baf61f331ea6b848e3630e566647302c26960c7303932bc33125074871eeb`。当前冻结修复已提交为 `010f7aa5ae2c39a5c90c7f22979391a6b1bbbf23`；[逐输入核对](committed-source-verification.json) 确认 633 项与 Git 对象及工作区一致。

## R02 剩余前置与测量出口

- [x] 核对实际 render/reference/capture/wake/VAD 顺序及已有依赖，复用离线 fixture。
- [x] 修复 native continuity 没有到达 server_turn 消费者的缺口，并补 KWS reset 与确定性回归源码。
- [x] 在最终冻结输入上独立运行上述新增及 macOS 条件下既有音频测试，核对原始命令、退出码、输入及日志；Linux 与真机证据分开。
- [ ] 固定真实硬件、采样格式、物理布局和是否已有硬件 AEC，再选择并接入成熟 AEC；不叠加未知回声消除器。
- [ ] 单个 audio worker 独占现有 reference consumer，在 wake/VAD/上传之前处理 capture；按所选库要求重分帧。现有 1024 样本 reference 块和至多 100 ms upload 块不能误称 10 ms AEC 帧。[WebRTC 官方 AudioProcessing 接口](https://webrtc.googlesource.com/src/+/refs/heads/main/api/audio/audio_processing.h) 将 capture/render 分别送入正向/反向处理，约 10 ms 分块并考虑两路延迟（2026-10-04 读取，blob `a4f3f52fe133d4c1b14b76f1b03d3b87a711d19a`）；这是后续接口参考，不是当前仓库已集成 WebRTC APM 的声明。
- [ ] 测量工具明确保留 stream/continuity/sequence/lost counters、采样格式、原生游标、clock quality、可用 queue delay 与观测时钟域。现有 AudioDebugRecorder 记录源 PCM，不是同步 native reference；server_turn 也没有调用它的 capture recorder。尚未新增录音工具或启用录音。
- [ ] 对参考丢块、原生 xrun、设备重开做实际 AEC reset/重收敛；不把缺失参考解释为静音。取消前已提交的 reference 与声学尾音窗口需按硬件标定保留。
- [ ] 使用获同意的素材和保留/删除策略分别测试 Alia 单讲、用户单讲、双讲；覆盖音量、距离/朝向、长时运行、重连。报告样本量与失败分母、自唤醒/自转写、用户语音完整性、插话到实际停声延迟、xrun/drop、CPU/RSS/队列趋势。
- [ ] ERLE 只解释适用的远端单讲、已对齐输入/输出；样本不足不报告高分位数。软件 timestamp、模型标签或 null PCM 不关闭声学质量验收。

R02 整轮保持开放。软件缺口修复不等于 AEC、双讲、实际停声、真 Pi 或长期运行已验收。

## 当前设备信息与停止后的资源状态

| 项目 | 分类 | 本轮依据 |
| --- | --- | --- |
| Pi5 / 8 GB、Ubuntu 24.04.4 aarch64、历史主机名及 SSH 用户 | 历史已知 | 来自本次附件；不提升为当前实测 |
| 当前可用 SSH 目标 | 待用户确认 | 本机 SSH config 及唯一 OrbStack include 无可识别 Pi 候选；对历史主机名及 `.local` 的四种写法，本机 known_hosts 均无匹配。没有据此猜旧 IP 或绕过 host-key 验证 |
| 当前 kernel/toolchain/ALSA/USB/串口/磁盘/供电散热 | 未实测 | 没有确认接入目标，未运行 SSH 或扫描网络 |
| 麦克风/声卡/扬声器/放大器、连接主机、共享时钟/布局/硬件 AEC | 待现场信息 | 不从 Mac 开发设备或旧 Voice Doll 配置推断 |
| 控制板/固件/执行器/反馈/限位/安全姿态 | 待现场信息 | 见 R03 交接，未发真实动作 |

本子任务未创建或改变常驻服务、设备绑定、网络、部署政策或凭证；未打开音频设备、未采集声学录音。读取 SSH 文件时只提取匹配结论，未记录密钥或完整配置。需要用户确认当前可信 SSH host/alias 后，才可继续只读环境采集。
