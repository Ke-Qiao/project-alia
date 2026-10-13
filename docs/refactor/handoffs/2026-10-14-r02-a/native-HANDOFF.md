# R02-A — Native 双流与消费所有权 HANDOFF

日期：2026-10-14。Owner：`settings_auth_fix`。本文件覆盖 native capture/reference 与单消费者；显式会话、文件格式和离线重放见同目录 [evidence-HANDOFF.md](evidence-HANDOFF.md)，统一验证、归档及提交由 Root 完成。

## 当前进度

- [x] 读取恢复检查点、audio reference clock design、硬件确认与实际调用路径。
- [x] 在原实现上编写可执行 RED，并由 Root 冻结输入、实际复现。
- [x] 完成独占 reader、native capture 接入和必要调用方迁移。
- [x] 补充拆块、源格式、并发消费/发布与丢失守恒回归设计。
- [x] 仅对所属 6 个文件执行 rustfmt，使用 `skip_children=true`，未修改 Root 所有的 `playback/tests/control.rs`。
- [x] Root 对冻结后的 GREEN 运行统一 Embedded 验证并回填结果。
- [x] 独立 reviewer 完成最终源码读回，无阻断项；统一运行证据见下一节。
- [x] Root 提交源码 `e557ceb`，总 checklist、恢复检查点和证据归档随交接记录提交。

本分工 checklist 为 **8/8**，Root 已完成源码提交与最终归档准备。生产和测试源已停写；本 agent 未自行运行 Cargo、Git 提交或硬件 I/O。已读取 Root 的实际 GREEN 日志和 JSON，不把软件通过当作硬件验收。

## 已执行的 GREEN

Root 冻结 659 个输入，manifest SHA-256 `676d2e76fccd23ac78003a6d6cbc8258a1c1faeba18e8c8cf1d02da8b246bda3`。在同源 macOS ARM64 上运行 `cargo test -p alia-embedded --all-targets --offline --locked`：`green-embedded-loopback.log/json` 记录退出 0、58.43 秒、`source_drift=[]`；lib **218** 加 integration **66**，合计 **284 passed / 0 failed / 0 ignored**。

本分工的 reader/reference queue 10 项、clock 5 项均实际通过，包括原两项 RED、新增 2052 stereo samples 拆块、S32 与非有限样本保留、同时 claim 和并发发布守恒。原 playback/control、motion/PCA、configuration 等回归也包含在同一次 all-targets 运行中。

首轮 `green-embedded.log/json` 保留沙箱本地回环绑定 EPERM 导致的 42 项失败，不据此声称产品测试通过；Root 在同一冻结源码上完成允许本地 loopback 的重跑，才得到上述通过结果。`cargo fmt -p alia-embedded -- --check` 也退出 0、无输入漂移。workspace fmt 存在历史 Server 导入顺序问题，由 Root 记录，不扩展本分工修改范围。

同源合成证据离线 example 退出 0；详见 evidence HANDOFF。环境记录显示 OrbStack 为 Stopped，本轮没有启动 Linux daemon，Linux 专属 ALSA null runtime 测试未在本轮执行；无真实录音/播放或声学验收。

## 已复现的旧实现缺口

RED 清单共 657 个输入，SHA-256 `e520b96fb716df80ba7c5d5c4c2e9a7975b987495785d95c7bab2d55a96a4a78`；日志位于 `run/validation-20261014-r02-a/`。Root 保存 `red-source.patch`，每次 RED 结果的 `source_drift=[]`。

1. `module::cpal_audio::clock::tests::native_capture_entry_publishes_actual_pcm_in_the_bounded_dual_stream_bus`：通过行为等价抽取的真实 capture 入口提交 6 个非零/零 i16 stereo 样本，native cursor 为 3，但队列 PCM 块数为 0，期望 1；`red-native-capture.log/json`，退出 101。这是业务断言失败，不是编译失败。等价抽取 seam 已包含在 RED patch 中。
2. `module::context::audio_reference::tests::another_bus_clone_cannot_steal_the_established_consumers_pcm`：旧 bus clone 直接 `try_read` 取走已建立消费者的 PCM；`red-single-consumer.log/json`，退出 101。GREEN 将旧读取 API 迁移到 reader，并验证第二次 claim 失败、drop 后可重领且不清空队列。

旧 debug 配置的两个 RED 由 Root 负责，见 root HANDOFF。

## 生产结果与所有权

修改范围为 `packages/alia-embedded/src/module/` 下：

- `context/audio_reference.rs`：`AudioReferenceBus` 保留 Clone 供 producer 和 snapshot 使用，但不再提供读队列方法。`try_claim_reader` 通过 AtomicBool 取得不可 Clone 的 `AudioReferenceReader`；`try_read(&mut self)`、`snapshot(direction)` 和 `boot_id()` 由 reader 提供。Drop 只释放消费资格，不关闭 stream、不取消播放、不丢弃队列。
- `context.rs`：导出 reader 和本地 metadata 类型。`AudioSampleBoundary` 区分 CPAL capture callback、ALSA capture read 与原有两个 playback 边界，不修改 Delivery 契约。`NativeSampleFormat` 记录 CPAL 10 种 native 容器；marker 为 None。两枚举、ReferenceContent 与 AudioStreamSnapshot 复用已有 serde 依赖。
- `cpal_audio/clock.rs`：`observe_capture_samples` 对完整 native batch 只推进一次采集游标，返回原整批 AudioTiming 供现有 CaptureTiming 使用；同时按 frame 对齐复制到既有固定块。块的 sample time 从整批 sample time 按 frame 偏移，callback time、observed boot time、continuity 与原批保持一致。Unknown/None 不补造时间。Playback 的 `record_samples` 调用签名保持 SubmissionBoundary，内部转本地 metadata 并记录 native format。
- `cpal_audio/cpal_backend.rs`：CPAL native input slice 在 converter、resampling、capture gain 之前进入固定队列；所有原 native 容器均通过已有泛型路径。
- `cpal_audio/alsa_backend.rs`：仅成功 readi 返回的 S16/S32 prefix 入队；S32 在位移探测、转换成 i16、converter 和 gain 之前取样，不改 read/recovery/control loop 语义。
- `cpal_audio/playback/tests.rs`：原 post-mix/native reference 测试改用独占 reader，本地 boundary 断言更新；生产播放精确取消与资源清理路径未重写。

队列仍是最多 64 块，每块 1024 个 f64 样本；producer 使用原 try_lock，不等待消费者、不做文件 I/O，也不为每块引入堆分配。成功入队的 sequence 在取得 queue try_lock 后分配，避免独立错误 callback 抢先入队造成成功记录序号倒退；拿不到锁或队列满的尝试仍累计 attempted/lost blocks/lost frames。多个 loss atomic 不宣称属于同一原子快照。

CPAL error callback 可和 PCM callback 并发。marker 的 cursor/continuity 生成和入队之间可能穿插整批 PCM；capture 不为了排序改写原始 timing。证据层须保留标记与样本，并将歧义表示为 gap/reset/unknown，不能把 marker cursor 当作下一 PCM 必须越过的游标。

## 回归覆盖及验证入口

默认 Embedded `--all-targets` 会覆盖 macOS 可编译路径；最窄 filter 是 `module::context::audio_reference::tests` 和 `module::cpal_audio::clock::tests`。

- 独占读取、bus clone 第二次 claim 拒绝、释放后重新 claim、待读 PCM 保留、stream 状态不被 reader 生命周期改变；4 个同步线程同时领取恰好一位成功。
- 4 个线程并发发布 PCM/故障 marker：保留下来的 sequence 严格递增，join 后 attempted = retained + lost，known PCM frames = retained + lost；不以 sleep/时间猜测线程次序。
- 保留原 queue 满、try_lock 争用、丢失 close marker、最终 owner drop 顺序、restart 独立身份/时钟、回退 backend time 等回归。
- native i16 非零 PCM 的 RED 变为内容、boundary、format 与整批 timing 断言。
- 单批 2052 个 stereo 样本拆成 512/512/2 frames，native cursor 仅推进 1026；拼回样本等于原 normalized PCM，sample time 偏移准确，callback/boot time 和整批 CaptureTiming 不变。
- 原 S32 容器低位在转换前证据中可见；未知时间与 delay 仍为 None。native f64 NaN/Inf 和负零原样交给证据层，验证器须显式拒绝非有限值，producer 不伪造静音。
- Linux ALSA null 测试的 reader API 已迁移；是否在本轮环境实际运行以 Root 日志为准，不能以此文档或历史运行代替。

## 保留限制与后续

这里保存的是 native 后端交付/接受边界的 normalized f64 观察，源容器并非已测 ADC 有效位深。i64/u64 转 f64 可能丢失低位，不能宣称 raw bit-exact；文件层保持的 bits 是归一化后的 f64 bits。硬件固件是否已处理音频、采集/播放声学对齐、真实 DAC 输出均未知。

采集与播放保留独立时钟，等采样率不代表同一时钟。流快照仍仅保存每个方向最新 stream，reopen 后未观察到的旧 stream 最终状态不得补成 closed/complete。零长度 lifecycle marker 使用历史 ZeroFill content 但没有任何 PCM，不能当作实际静音帧。

本轮未触发真实录音/播放、设备连接、外部付费调用、AEC、平台扩张或依赖变更。软件 GREEN 与源码审查已完成；总文档归档和最终恢复检查点已由 Root 收口，交接记录另行提交。

Root 收口：源码 `e557ceb962610004287f38e66b50abca3e44b661`，659 个 Git blob 与 GREEN 清单全部一致；归档入口见 [Root HANDOFF](root-HANDOFF.md)。
