# R02 Mac 离线软件链模拟交接

日期：2026-10-14。基线 `ea3f0e4`；分支 `codex/architecture-refactor`。本分工 **8/8**，独立静态复核无阻断，Root 统一执行的默认和无默认 features 验证均通过并已独立读回；提交由 Root 统一处理。R03 保持暂停。

## 已有实现与本轮缺口

已读 `examples/audio_evidence_replay.rs`、R02-A native/evidence 交接、field-prep 检查点及合成演练证据。原示例的 7 条记录直接构造 EvidenceRecord 与 latest snapshots，确实经过 JSON 导出/读取/replay；其 16 个样本值及 9 个 reset 提示只证明手填格式演练，没有经过实际 capture helper、renderer、mixer、CPAL 输出填充、reference bus 或 EvidenceSession。

原有 clock、reference、mixer、playback/control 与 evidence 单测已经分别验证局部行为，本轮不把缺少组合测试包装为已复现生产缺陷。补齐同一个软件链上的真实 PCM 与生命周期交接；库 footer 缺口和示例 CLI 由其他 owner/Root 负责。

## 实施与所有权

仅改 `packages/alia-embedded/src/module/cpal_audio/playback/tests.rs` 的子模块声明，新增 `tests/offline_evidence.rs`。复用私有生产 `observe_capture_samples`、`Renderer`、`PlaybackBuffer`、`fill_output_buffer`、`PlaybackSender` 和公开 `EvidenceSession` / JSON / replay。未扩展生产 API 可见性、未新增依赖、模拟 backend 或服务；未修改 R03。

四项测试的统一 filter 为 `module::cpal_audio::playback::tests::offline_evidence`：

1. `capture_mix_exact_cancel_and_single_observer_round_trip_the_actual_pcm`：同一 reference bus/session 接收 2052 个真实生成的 i16 capture 样本，经生产 helper 分成 512/512/2 个立体声帧；voice/cue 实际混音得到 `[11000,11000]`，精确取消 cue 后 voice 尾部仍为 `[20000,20000,30000,30000]`。检查 owner 分别提交 3/1 源帧、完成/取消，单 reader 排他，JSON 往返与 replay 全样本相等；两个 stream clock 独立且时间未知不补造。
2. `explicit_hold_records_real_silence_and_resume_retains_only_voice_tail`：显式暂停通过真实控制与 callback 写出两个 0，证据为 HeldSilence 且 voice/cue 尚未提交；继续后只提交原 voice 尾部，cue 为取消且未重播。导出/读回保留实际静音，而不是把缺失 PCM 解释成静音。
3. `cancelling_or_dropping_observer_leaves_actual_playback_owner_and_pending_pcm_live`：取消并完成观察、以及新观察者直接 Drop，都释放消费权，不终止实际排队/已渲染的 playback owner；重新 claim 后同一 owner 正常提交原 PCM，后续 capture 与 callback 输出进入新 evidence。取消时双向 stream 仍 running，source frames 仍 0，之后才正常结束。
4. `producer_overflow_lost_close_and_reopen_keep_missing_audio_and_old_final_state_unknown`：实际 capture helper 连续发布填满 64 槽队列，旧 capture close marker 丢失；此时实际播放 10 帧也全部无法进入 reference 队列，closed playback footer 记录 3 个丢失块/10 个丢失帧。排空后 capture 重开使用新 ID，bundle 不继承旧流的关闭状态；replay 只返回 63 个已记录旧 capture 帧及一个新帧，不补丢失的 capture 或 playback 样本。

每项有 playback 的测试都检查 shutdown 后 queue/source-byte reservation、rendered buffer、resampler 状态清理，以及 owner output quiescence、presentation 仍未知；导出观察者结束后重新 claim 可排空。测试不启动线程、设备、模型、网络或执行器；没有超时睡眠。操作为有限同步次序，真实 stream UUID 与 observed_boot_ns 不强求跨运行字节相同。

## 证据边界

使用生产边界 helper 不等于打开 native 设备。测试导出明确置 `SyntheticFixture`；所有 PCM 为生成数据，sample/container 信息仅为软件输入类型。`unobserved_history_possible=true` 保持；不是 R1 录放音、macOS 驱动调度、实际设备线程 join、物理时间对齐、AEC 或自然全双工验收。

精确 loss 数仅在同步生产完毕且流 closed 后核对。本轮不引入活跃 atomic snapshot 的跨字段同步假设。旧 capture 的已关闭 snapshot 在重开前只由测试观察，最终 bundle 的 latest 已换成新流，不能把测试掌握的旧状态凭空写进 evidence。观察者取消只取消诊断消费，不撤销输出 owner。

## Checklist

- [x] 读回既有示例与 R02-A / field-prep 证据。
- [x] 确认缺口为跨组件软件链，复用真实私有 helper。
- [x] 同 bus/session 的 capture 分块、混音、精确取消与导出回放断言。
- [x] 显式 hold / resume 的 PCM 和 owner 账本断言。
- [x] 观察者取消/Drop、重新 claim 与实际 owner 继续工作的断言。
- [x] 实际 queue overflow、lost close、reopen 与清理断言；源码交 Root 冻结。
- [x] Root 实际编译/测试通过并保存冻结输入、日志与计数；本 agent 独立读回如下。
- [x] 独立 review 已完整读回四项测试，无阻断；本分工交接完成，提交仍由 Root 统一处理。

## 实际验证

证据目录为 `run/validation-20261014-r02-mac/`。独立读回 `final-embedded.log/json` 与 `final-no-default.log/json`，四项 `offline_evidence` 测试在两种 features 组合中均逐项 `ok`。

- 默认 `cargo test -p alia-embedded --all-targets --offline --locked`：exit 0，84.00 秒；**327 passed / 0 failed / 0 ignored**，包含 247 lib、75 integration、5 example。
- 无默认 features 的 lib 与 `audio_evidence_replay` example：exit 0，35.63 秒；**252 passed / 0 failed / 0 ignored**，为同一批 247 lib 与 5 example 的重复验证，不另计新增测试数量。
- `final-cli-build`：exit 0，1.43 秒；`final-cli-smoke`：exit 0，0.70 秒。`cli-smoke.json` 的九项场景全部符合预期，包括历史重放、拒绝覆盖文件、拒绝矛盾 closed footer；`all_assertions_passed=true`、`temporary_directory_removed=true`。这些 CLI 结果属于 Root/其他 owner 的组合交付，并非本文件新增的四项测试。
- 最终 `final-source-manifest.json` 实际读回为 **666** 项，文件 SHA256 为 `ef33fcd714968f0236f42a065783c9967f23f7b0ac22deee6f120b376efef7f8`；上述四次运行 JSON 均记录同一 manifest 和 `source_drift=[]`。

本 agent 仅只读核验日志/manifest 并更新本交接，未运行 Cargo、Git、数据库或设备动作；源码继续冻结，交接更新后停写。
