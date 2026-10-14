# R02 时钟与参考缺口 — Reference / replay 交接

日期：2026-10-14；基线 `38fb689`；分支 `codex/architecture-refactor`。Owner：`settings_auth_fix`。本分工 **8/8**；真实 RED、同源码 GREEN 和实际 CLI 重放已独立读回，独立 review 无阻断，源码与交接停写，提交由 Root 统一完成。R03 继续暂停。

## Checklist

- [x] 读取上一轮四份 HANDOFF、验证摘要和独立音频时钟设计。
- [x] 追踪 native capture / postmix、共享 reference 队列、单 reader、EvidenceSession 与 Replay。
- [x] 明确已有保障、实际消费入口与时钟回放缺口，报告 Root 并获得有界测试范围。
- [x] 只添加历史时钟异常的综合 RED 测试；候选完整停源，Root 统一格式、冻结与运行。
- [x] Root 实际 RED 失败证据读回。
- [x] RED 后最小回放修复及必要正例；生产 clock 修复由另一 owner 负责。
- [x] Root 统一 GREEN 和独立 review 通过。
- [x] 最终日志/计数/限制回填并停写，提交由 Root 完成。

## 沿用证据与已证明契约

上一轮 R02 Mac 已完成 4 项真实软件链组合测试：capture helper 分块、实际混音后的 CPAL callback、单 EvidenceSession、JSON 往返/replay，以及精确取消、hold/resume、观察取消/drop、队列溢出、lost close 与 reopen。默认 327/0/0、无默认 252/0/0 是历史基线结果，不当作本轮测试。上一轮 closed footer 保守下界也已完成：观察块/帧不超过 attempted−lost 与 cursor−lost；允许 partial/empty，active snapshot 不强行要求原子一致。

当前生产链与边界：

- `cpal_audio/clock.rs:82` 的 `observe_capture_samples` 在 native 完整帧上一次 observe 后按最多 1024 样本分块；capture PCM 位于转换/增益之前，保留输入容器和 normalized f64。`cpal_backend.rs:383` 是实际 CPAL capture 调用；ALSA 读成功前缀使用相同 helper。
- `cpal_backend.rs:468` 的 `fill_output_buffer` 写实际最终输出后调用 `record_samples`；media、held silence、真正 underflow/争用填零及混合分别标记。`playback.rs:1143` 的 ALSA 提交边界只记录实际接纳前缀。参考块丢失不会补零，实际写出的零样本仍保留。
- `context/audio_reference.rs:137` 用 compare_exchange claim 非 Clone reader，只有 reader `try_read(&mut self)` 可 pop；bus Clone 无法偷取。reader Drop 仅释放消费权，不清队列，也不关闭或取消音频 owner。
- 同文件 `publish` 只 try_lock 固定 64 槽队列；成功入队先读 loss 再分配 sequence，失败独立累计 attempt/loss，不阻塞 producer。capture/playback 共用容量，方向饥饿会通过缺口暴露，没有公平性或时间容量承诺。
- `audio_evidence.rs:192` 的 pump 有界检查取消与限额；停止观察后 claim 保留到 finish/Drop。finish 只取得两个方向的最新 snapshot，错误返回同样释放 reader。取消诊断不会取消播放。
- Replay 目前按方向/流身份、marker/continuity、sequence、loss 和 PCM cursor gap 发 reset；保留 marker 与 PCM 的合法交错，不改写旧 timing。旧流被新流覆盖且无本流 close marker/final snapshot 时，最终状态仍 unknown。closed footer 可保留尾部 loss；不能从记录序号差推断精确 producer loss。

现有生产代码没有启动 AEC worker，也没有在正常音频模块中调用 EvidenceSession；`AudioPlaybackBackend::reference_bus` 仅暴露可 claim 的总线。唯一可用消费契约是原始完整帧块与显式诊断会话；1024 是最大样本存储量，不是 AEC 的固定帧长、10/20ms 周期或采播配准要求。本轮不选择 AEC 算法、重采样器或创建消费框架。

## 基线回放缺口

基线 `audio_evidence.rs:709` 的 Replay::next 不检查 clock 字段。若同 stream 的相邻实际 PCM 保持 sequence 连续、frame cursor 连续、continuity 不变，而时钟发生以下变化，旧实现第二步仍返回空 resets：质量 BackendEstimate ↔ Unknown；callback_ns 或 sample_ns 各自 Some ↔ None；callback_ns 或 sample_ns 单独倒退。

这类历史输入可由原 `read_json` 合法导入，且应继续保留原 PCM 与时间证据。它们不需要被判为恶意/非法文件，也不能用强拒绝时间倒退来破坏已有 marker 与旧 PCM 合法交错。问题是消费者缺少“不能延用之前时钟关系”的回放提示。

生产根因另属 `AudioStreamClock::observe`（`audio_reference.rs:374`）：基线只检查 clock_id 与 callback_ns 倒退，不检查 sample anchor 单独倒退；playback 大 batch 逐块 observe 还需由 clock owner 检查是否在同一 batch 提前恢复时钟质量。修复新 producer 不会自动修好已保存文件，因此回放保护仍单独需要。

Root 批准最小 GREEN：新增单一 `ReplayReset::ClockDiscontinuity`，仅比较同一完整 stream 身份的相邻实际 PCM，跳过无 PCM marker；对质量或 callback/sample 可用性变化，以及两个 Some 值的倒退发提示。既有 PCM/clock/continuity 原样，不改 JSON schema 或拒绝历史文件；不比较不同方向/流，不把 queued delay 或 host observed_boot_ns 当共同采样时钟，不增加漂移阈值。稳定相等 callback、负 sample 起点正常递增与 marker 插入应保持合法且不多发时钟 reset。

## 实际 RED

唯一新增测试：`module::audio_evidence::tests::historical_pcm_clock_changes_require_replay_reset_without_rejecting_samples`，位于本分工 `audio_evidence/tests.rs`。

它从既有 fixture 取真实记录形状，构造 8 类时钟变化的连续双 PCM，清空不相关 latest snapshots，使用真实 `AudioEvidence::read_json` 导入，再检查原 timing 与样本 bits 保留。第二步若没有任何 reset 则收集标签；没有引用尚不存在的 enum，也没有修改生产来造编译失败。

已独立读回 `run/validation-20261014-r02-clock/red-clock-contract.log/json`：Root 的无默认 lib 三个定向用例退出 101，11.08 秒，**0 passed / 3 failed / 0 ignored / 247 filtered**。本用例实际失败列出 `quality lost`、`quality restored`、`callback missing`、`callback restored`、`sample missing`、`sample restored`、`callback regressed`、`sample regressed` 全部 8 类；另两项是 clock owner 的 producer 反例。RED manifest SHA256 为 `f492372c783673beac85b356b3e0d01fdd1e4b165ceed22d679a77d5d9816272`，`source_drift=[]`；不是编译失败或本轮 GREEN。

## GREEN 实施

只修改 owned `audio_evidence.rs` 与 `audio_evidence/tests.rs`。在已有 FrameGap 的同 stream 前一 PCM 查找中追加 clock 比较，不增加遍历或框架；新 enum 注释明确这只是相邻 PCM 时钟元数据关系不再可延用，不是设备故障、漂移测量或采播对齐证明。特别是上一块 sample 时间可能为大 batch 内的名义偏移，回放不会将此时钟提示升级为物理结论。

原 RED 改为精确检查 `ClockDiscontinuity`；新增两项正例：

- `replay_clock_checks_skip_markers_and_allow_equal_callback_with_negative_sample_origin`：已知 PCM → Unknown marker → 已知 PCM，callback 相等、负 sample 正常递增、host observation 变化；只保留 marker 的既有 Discontinuity，不误发时钟 reset，原样本和 timing 不变。
- `replay_clock_checks_stay_per_stream_and_ignore_alsa_queue_delay_changes`：capture/playback 和 capture 新旧 stream 各自时钟独立；ALSA 的 sample=None、相同 callback、queued delay 的 None→负数→正数→None 不独立触发时钟 reset，只有 FirstObservation / StreamChanged 的原语义。

本模块现共 18 项测试（原 15 + RED 1 + 正例 2）。两份 owned 文件使用 `rustfmt --edition 2024 --config skip_children=true` 格式化，exit 0；完整停源后交 Root 统一执行，未自行运行 Cargo。

独立 reviewer 已完整读回生产补丁和三项本轮用例，确认 schema/validator/PCM 未改、时钟比较按完整 stream 隔离、marker 与各正常时间/延迟边界无误，静态审查无阻断。

Producer 与 replay 的边界不同：producer 只比较原始 native batch anchor；例如新 batch sample=201ms 大于上一批原始200ms，虽小于上一批末块名义推导221.3ms，也不能判为 native sample clock 回退。Replay 只有实际保存的相邻 PCM 元数据，没有可信 batch 身份，看到221.3→201ms时仍可保守提示 ClockDiscontinuity；这不代表丢 PCM、物理时钟故障或测得漂移，不应为消除此提示而弱化既定回放比较。

## 最终验证独立读回

已只读核对 `run/validation-20261014-r02-clock/` 中日志、运行 JSON、manifest、smoke 与 static：

- 首次 `final-embedded` 退出 101，**212 passed / 43 failed / 0 ignored**。`sandbox-failure-analysis.json` 及原日志明确全部 43 项在三处既有 fixture 的 `TcpListener::bind(127.0.0.1:0)` 遭 EPERM；原失败保留，不计生产业务 RED，重跑未改源码。
- 同源码 `final-embedded-loopback`：默认 all-targets 退出 0，68.75 秒，**335 passed / 0 failed / 0 ignored**，包含255 lib +75 integration +5 example。本模块18项、三项本轮新增测试均逐项 `ok`。
- `final-no-default`：无默认 features 的 lib + example 退出 0，39.55 秒，**260 passed / 0 failed / 0 ignored**，为重复255 lib +5 example，不累加独立数量。本模块18项同样全部通过。
- `final-cli-build` 退出0，1.49秒；`final-replay-smoke` 退出0，0.60秒。实际重放合成三记录证据给出 **ClockDiscontinuity 2 次 + FirstObservation 1 次**，历史7 records / 9 resets保持不变；`replay-smoke.json` 两场景 `input_unchanged=true`、`all_assertions_passed=true`。
- 最终 manifest 实际读回 **666 项**，SHA256为 `fb4a641051ef77125d788d7013487dcc7fda488e43c74044a0690c5767144fe7`。运行 JSON 均记录同一 final manifest 和 `source_drift=[]`。
- 变更文件 rustfmt 与 diff-check通过；全仓fmt仍仅未修改的 `packages/alia-server/tests/server_runtime/delivery.rs` 历史import顺序失败，不宣称全仓格式通过。

本 agent 仅只读核验结果并更新本交接，未运行 Cargo、Git、SSH、设备、模型、服务或新增依赖。当前生产、测试及本交接停写；Root 统一证据收口与提交。离线软件结果不替代真实录放音、设备漂移/延迟、AEC、双讲或 R03 物理验收。
