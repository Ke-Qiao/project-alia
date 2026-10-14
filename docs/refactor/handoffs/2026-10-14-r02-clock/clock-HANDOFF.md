# R02 独立音频时钟 — HANDOFF

日期：2026-10-14；Owner：`settings_auth_reproduce`；基线 `38fb689`，分支 `codex/architecture-refactor`。本轮只审计并补已存在的本地时钟证据边界，不实现 AEC、漂移估算器或新重采样框架，不新增依赖。R03 按用户决定暂停。Root 统一运行 Cargo、冻结证据和 Git；本 agent 未运行 Cargo、设备、SSH、模型或外部服务。

## Checklist 与进度

- [x] C01 读取上一轮恢复检查点及四份 HANDOFF，核对已完成能力和禁止重复范围。
- [x] C02 追踪 Core 类型、Embedded 原生后端、转换后 speech 时钟、所有生产调用者及已有回归。
- [x] C03 汇报两个具体反例；按 Root 授权仅增加 RED，生产保持未改后停写。
- [x] C04 Root 实际确认 RED 后完成有界修复与必要边界正例。
- [x] C05 独立审查与实际 GREEN 结果读回，不以历史结果替代。
- [x] C06 记录剩余漂移/对齐限制并完成分工交接，Root 统一检查点和提交。

当前 **6/6（本分工完成）**。真实 RED、同源默认完整 GREEN、无默认 features 与实际纯文件 replay 结果均已独立读回，静态审查无阻断。源码、测试与本文停写；全包检查点、归档和正常提交由 Root 统一，不预写提交 hash。

## 已核对的时钟域与实际调用链

| 域/字段 | 实际来源与保证 | 不能推导的结论 |
| --- | --- | --- |
| `AudioStreamId { boot_id, stream_id, direction }` | builder 将 Embedded boot ID 交给 AudioBus；每次 capture/playback native open 分配新 UUID；同向重开同样换 ID | 同设备、同采样率或共同主机计时器不等于共享采样域 |
| `AudioTiming.start_frame/frames` | capture 实际完整输入帧；CPAL 最终回调缓冲区与 ALSA writei 成功接纳前缀；xrun 未知缺帧不补入 cursor | 不能证明 DAC 呈现、物理采样时间或缺失段长度 |
| `continuity` | Started/Stopped、故障、取消等 marker 改变分段；native cursor 不因分段回退 | 不能把不同分段当连续信号，marker/PCM 入队顺序也不是全局 timing 顺序 |
| `clock_id` | 严格对应该 native stream UUID；后端时钟原点每次 open 独立 | 不可直接相减两方向的 `callback_ns` 或 `sample_ns` |
| CPAL `callback_ns/sample_ns` | 由该 stream 首个 callback 建原点，读取 Input/Output timestamp；负的 sample offset 合法 | 后端估计不是物理校准，尤其不能把它当 R1 固件/AEC 参考的测量 |
| ALSA `callback_ns/queued_frames` | 成功启用 Monotonic timestamps 后读取 status htstamp/delay；时间戳缺失保留 Unknown；`sample_ns=None` | status + delay 不是某个样本的已证实采集/呈现时刻 |
| `observed_boot_ns` | 同一 AudioBus 的 `Instant` 单调原点，采样时记录主机观察时刻 | 包含回调/线程调度及执行延迟，不能直接作物理采播配准 |
| `CaptureTiming.converted_*` / `AudioFrame.timestamp_us` | capture 转换后独立样本域；`timestamp_us` 仅为 converted cursor 对应时长 | resampler 跨块历史和延迟未建模，不能宣称 native 第一帧与 converted 第一帧精确对应 |

Core 定义位于 `crates/alia-core/src/audio_clock.rs`；本地 owner 与 monotonic 游标在 `module/context/audio_reference.rs`。`module/cpal_audio/clock.rs` 提供 CPAL/ALSA 原点、capture 观察与 playback 记录。CPAL 实际输入在转换/增益前观察，输出在最终混音/格式量化后记录；ALSA 捕获读取成功前缀并在转换前记录，播放仅在 writei 成功后记录接纳帧。

`module/speech/duplex.rs::CaptureClock` 验证 converted 16 kHz mono 的样本数量、游标/时间戳及 native identity/continuity；重开或 native continuity 变化会结束当前 capture session，外层按既有清理/重建流程处理。它不对齐 playback，也不把 native cursor 强行要求连续，因为某些 native 批次可能尚未产出 converted PCM。native 证据导出/replay 的唯一 consumer 与 speech 的 converted broadcast 是不同用途，当前未新增第二个 reference reader。

只读查看了 Cargo.lock 对应的本地 CPAL 0.15.3 源：`InputStreamTimestamp.capture` 是输入批次采集时间，`OutputStreamTimestamp.playback` 是预测播放时间；macOS 实现实际以 callback 与本批 buffer duration 的加减形成估计。该细节进一步说明 sample 锚点变化仅是时间证据分段，不应宣称设备物理故障或由此虚构 PCM 丢失。本轮未为此访问外部服务或更新依赖。

## 已有行为与两项可复现缺口

已存在 callback 时间倒退、ALSA 缺失时间、分块 offset、native 重开、转换器 reset、尾部 close loss 和单 reader 回归。上一轮新增 closed 统计自洽与实际 capture/postmix/session/export/replay 串联；本轮复用，不重复构造基础 fixture。

**缺口 1：sample 锚点单独倒退未被识别。** `AudioStreamClock::observe` 只保留 `last_callback_ns`。先读 `callback=100, sample=-10`，再读 `callback=200, sample=-20` 时，sample 已倒退，但旧实现仍保持 BackendEstimate 和原 continuity。Root 确认按 native batch 比较 callback 与 sample 两个可用锚点，任何一者回退都应推进时间分段并降低该批读数；不回退 cursor、不丢弃或补造 PCM。相等 callback、负 sample 起点和 ALSA sample 缺失均合法。

**缺口 2：同一播放 batch 的回退降级只作用于首块。** `record_samples` 原先每拆一个 1024-sample 块都再次 `observe`。同一大回调读数发生倒退时，第一块 Unknown，后两块用相同 callback 再比较时不再小于 last，便重新恢复 BackendEstimate。capture 已按整批 observe 一次，playback 应对齐该契约；派生的 chunk offset 不应更新下一次 native batch 的原始锚点历史。

Root 已批准仅在 `module/context/audio_reference.rs` 与 `module/cpal_audio/clock.rs` 增加 RED：

- `module::context::audio_reference::tests::backwards_sample_time_changes_continuity_without_rewinding_frames`：sample 单独从 -10 回退到 -20，应分段并保持帧游标；下一批增加至 -15 后可恢复新段的后端估计。
- `module::cpal_audio::clock::tests::playback_batch_clock_regression_downgrades_every_split_block`：2052 个 stereo 样本经真实 helper 拆成三块，应全部 Unknown；旧实现预期为 `[Unknown, BackendEstimate, BackendEstimate]`。另核对完整游标 `(1,512)/(513,512)/(1025,2)` 和总 cursor 1027。

Root 先冻结上述 RED 与另一 agent 的 historical replay RED 后实际运行。独立读回 `run/validation-20261014-r02-clock/red-clock-contract.log/json`：退出 101、11.08 秒，**0 passed / 3 failed / 0 ignored / 247 filtered**；我的两个 producer 测试分别真实报告 sample 回退未分段与 `[Unknown, BackendEstimate, BackendEstimate]`。第三项为 reference owner 的 replay reset 反例，不计入本 owner 的新增测试。666 输入 RED manifest SHA-256 为 `f492372c783673beac85b356b3e0d01fdd1e4b165ceed22d679a77d5d9816272`，`source_drift=[]`。这是可编译真实业务失败，没有把缺符号或环境错误当 RED。

## GREEN 实施与边界矩阵

Root 确认 RED 后授权修复。`AudioStreamClock` 增加 `last_sample_ns`，以 `<` 独立检查 callback/sample 两个可用原始 batch 锚点；正常缺失保留之前可比较的读数。发生回退时以本批原始锚点开启新段，当前整批返回 Unknown；下一批递增读数可恢复后端估计，游标始终继续推进。

采集和播放共用同文件私有 `publish_native_samples`，只接收已观察的 batch timing 与不可变 probe，统一完整帧分块、offset、归一化和 publish。helper 不持可变 clock，也不能调用 observe；没有新公开 API 或通用框架。playback 完整帧数为零时继续提前返回，不产生 observe/Started；不完整 frame 原有 BackendError 标记保留。capture 的返回值仍是完整 native batch timing。

本 owner 新增测试共 **5 项**，原两项 RED 断言保留并补必要后续状态：

| 测试 | 证明边界 |
| --- | --- |
| `backwards_sample_time_changes_continuity_without_rewinding_frames` | callback 前进而 sample 回退推进时间段，PCM cursor 不回退，后续正常 sample 恢复估计 |
| `playback_batch_clock_regression_downgrades_every_split_block` | 三块使用同一次 Unknown 读数、同 continuity/host observation；仅一次 ClockRegression，保留全部帧；下一 batch 正常恢复 |
| `equal_negative_and_missing_clock_anchors_do_not_invent_gaps` | 负 sample 起点、相等 callback/sample、缺失后正常恢复、ALSA callback/delay 有值但 sample None 均合法；缺失不抹掉旧锚点，新流不继承旧历史 |
| `playback_split_offsets_do_not_replace_the_next_native_batch_anchor` | 块内按 nominal sample rate 推导 offset；下一真实锚点可早于上一批末块推导时间、但晚于上一批真实锚点，不误判 producer 回退 |
| `split_sample_offset_overflow_keeps_the_available_callback_estimate` | capture/playback 两方向 sample offset 超过 i64 表示范围时明确 None，保留实际可用 callback 和 BackendEstimate；不造缺帧或反复分段 |

上述两份源码已 scoped rustfmt 并停写；Root 直接统一最终整包验证，不由本 agent 运行 Cargo 或反复局部 GREEN。现有 native queue、取消、owner、reference/evidence 及包外源码未由本分工修改。

## 本轮实际验证与审查

本 agent 只读核对 `run/validation-20261014-r02-clock/` 原始日志、JSON、static 和 sandbox 分析，没有另跑 Cargo。最终 **666** 输入 manifest SHA-256 为 `fb4a641051ef77125d788d7013487dcc7fda488e43c74044a0690c5767144fe7`；以下各最终运行均 `source_drift=[]`。

- 首轮 `final-embedded.log/json`：退出 101，51.80 秒，lib **212 passed / 43 failed / 0 ignored**；我的五项新 clock 测试均通过。43 项失败均为三个既有 fixture 的 `TcpListener::bind(127.0.0.1:0)` 返回 EPERM：playback 21、body 9、duplex 13。原日志及 `sandbox-failure-analysis.json` 保留；没有将其记作产品 RED 或完整 GREEN，也没有改源码绕过失败。
- `final-embedded-loopback.log/json`：Root 在允许本地回环的环境按相同 frozen manifest、默认并行与完整 targets 重跑，退出 0，68.75 秒；lib 255 + integration 75 + example 5 = **335 passed / 0 failed / 0 ignored / 0 filtered**。五项 owned 新回归逐项通过，既有 owner、取消、清理、PCM/loss 和重开测试没有跳过。
- `final-no-default.log/json`：无默认 features 的 lib + `audio_evidence_replay` example，退出 0，39.55 秒；lib 255 + example 5 = **260 passed / 0 failed / 0 ignored / 0 filtered**。这是同一批测试的另一 feature 运行，不累加为独立测试数量；五项新 clock 回归同样全部通过。
- `final-cli-build.json`：最终无默认 features example 构建退出 0，1.49 秒。`final-replay-smoke.json`：实际纯文件 replay 场景退出 0，0.60 秒；`replay-smoke.json` 确认 clock-transition 为 3 records / 3 resets，历史 fixture 为 7 records / 9 resets，两份输入 SHA-256 前后不变，`all_assertions_passed=true`。此部分由 Root/reference owner 交付，未运行 native 音频或模型。
- `static.json`：四份本轮变更 Rust 文件格式检查与 diff check 均退出 0。全仓 format 退出 1，仅为未改动 Server `tests/server_runtime/delivery.rs` 的既有 import 排序；Root 已核对相对 HEAD 未变，本轮未扩张修复。

独立 reviewer 已完整读回 producer 最终实现、五项回归及与 reference replay 的协作边界，结论无阻断：私有 helper 不 observe、不增加堆分配或新 owner；仅真实 batch 锚点进入历史；正例保留负/相等/缺失、ALSA None、重开和表示溢出语义。R03 没有推进；本轮没有新平台、服务、设备调用、算法依赖或实机声学结论。

## 保留的时间可用性与漂移边界

`sample_ns` 在分块时 checked-add 失败可以为 None，而 callback 仍是有效后端观察；当前 `BackendEstimate` 不要求所有可选字段齐备，因此不能无依据把这种部分可用估计一律降为 Unknown 或伪造硬件中断。replay 可以提示可用性改变，但不能把它计为 producer PCM loss。缺失读数也不重置先前原点或捏造时间。

producer 比较原始 native batch 锚点；文件 replay 只能看到逐条 PCM 记录。若下一批原始 sample 锚点高于上一批原始锚点、却低于上一批末块的 nominal 派生 offset，producer 不应误记 ClockRegression，而 replay 可以保守提示逐条时间不连续。两者不能混称：提示不认证真实后端发生回退，也不代表 PCM 已丢失。没有增加缺乏 consumer 需求的 batch ID 或漂移映射。

当前数据允许同一 stream、同一连续段内观察帧数与后端时间增量；已有 host 观察时间可用于诊断关联。实际采样频率漂移、两方向相位/延迟映射及可信区间仍未建立：callback 调度、后端延迟估计、ALSA queue delay、重采样器历史和真实设备时钟都可能影响这些关系。本轮不从有限合成数据估算 ppm、不增加共同时间轴、校准参数、AEC 帧聚合或自动 reset 策略。

双向 native timing 不进入现有 Voice 网络契约作为物理对齐证明；本轮保留 Proto/API、默认 shadow、权限、精确取消与 owner 清理。R1 仍未接入，物理参考路径、AEC、双讲与尾音验收未完成；R03 后续暂停，不用软件离线通过关闭现场项。
