# R1 单次手动唤醒观测工具

独立 Rust example，复用 Alia 的 R1 协议、独占串口和 ALSA 采集实现。**必须等主任务明确释放设备后才运行 `run`**；2026-10-24 实机验证已在确认释放后开始，原始证据保存在 `run/r1-manual-capture-20261024/live/`。工具不启动 EmbeddedBuilder、服务、VAD、ASR、TTS 或真实模型。

协议发送一次初始版本查询、必要的握手 ACK，以及一次 `manual_wakeup`。如果冷启动握手晚于初始查询，工具在应答握手后最多补发一次只读版本查询；重复握手不增加查询次数，也不延长版本阶段的 2 秒绝对期限。没有手动唤醒重试、定时唤醒、自动自然唤醒、重开采集、播放、音量控制命令、CDC 原始音频切换、固件写入或电源操作。生产配置及自然唤醒会话逻辑不变。

## 本地离线验证

在仓库根目录执行（Rust 1.96.0；已有依赖缓存）：

```sh
cargo test --offline --locked -p alia-embedded --no-default-features --example r1_manual_capture
cargo run --offline --locked -p alia-embedded --no-default-features --example r1_manual_capture -- --help
```

macOS 可运行纯协议、计划和合成 PCM 测试；Linux 额外测试 PTY 分包、握手、取消写入、串口独占释放及输出保护。测试不访问真实 UART/声卡，不运行 ignored 硬件测试。5 分钟静默使用合成时间推进，无需实际等待 5 分钟。

Linux 构建：

```sh
cargo build --offline --locked -p alia-embedded --no-default-features --example r1_manual_capture
```

输出为 `target/debug/examples/r1_manual_capture`（设置 `CARGO_TARGET_DIR` 时位于相应目录）。构建目标必须与未来设备一致；不要把 macOS 产物部署到 Pi。`--help`、空参数和非法参数均不打开设备；非 Linux 平台拒绝硬件模式。

## 设备释放后的实验

1. 由主任务确认 UART 和 UAC 已释放，核对控制 UART 稳定路径、R1 UAC 名称/序列号及独占使用条件。示例路径仅沿用历史记录，不能替代届时确认。
2. 保持当前固件，真正断开 R1 的全部供电再上电；记录断电/上电时间、设备序列号、说话方向与距离。Pi 或进程重启不等于 R1 冷启动。上电后不要先说自然唤醒词或运行其他唤醒工具。
3. 工具强制匹配以下完整版本，不匹配即停止，不发送手动唤醒：

   ```text
   version: UART_REAL, circle6:1.9.6.f8ef651@master engine:circle6.1004; build date: Sep  3 2026 22:40:02
   ```

4. 在面向已确认 beam 的固定位置，按终端提示完成一次实验。beam 0 是方向波束，不是全向模式。下面命令只供设备释放后使用，替换冷启动说明和输出路径；输出目录的父目录须存在，工具拒绝覆盖已有目录。

   ```sh
   ./r1_manual_capture run \
     /dev/serial/by-id/usb-1a86_USB_Serial-if00-port0 \
     hw:CARD=L6Microphone,DEV=0 \
     0 300 ./r1-manual-cold-01 \
     '填写实际断电及上电时间、设备序列号、beam 0 朝向、距离' \
     --save-pcm
   ```

5. 5 秒基线保持安静；收到匹配 ID 且数值 `code:0` 的手动命令 ACK 后，进入 45 秒说话窗口；随后静默 300 秒；收到下一提示后进入第二个 45 秒窗口。通过聊天提示配合时，操作者看到提示后说约 15 秒，再自行安静，给通知延迟和句尾留出余量。全程不用自然唤醒词，不触发其他软件唤醒。可用“现在开始录音测试，一二三四五，窗外有一棵树”等普通句子。提示只写终端，不通过扬声器播放。
6. 长静默可显式设为 30–3600 秒。若需比较不同静默长度，每轮重新建立真正冷启动条件；不要在已经自然唤醒的状态上补测并称为冷启动结果。

基线出现板端事件则在发送手动命令前中止。手动命令之后出现的所有 `0x04` 帧及可解析唤醒事件都会记录；它可能来自手动命令或自然唤醒，必须结合现场记录复核，不会自动把它解释成自然唤醒或新的声源角度。版本确认后再次收到握手会中止，避免把板端可能重启后的状态接续成同一次实验。版本或命令 ACK 超时、拒绝、迟到均中止；版本阶段超时会明确记录尚未发送 `manual_wakeup`。手动命令部分写入/ACK 超时意味着结果可能未知，工具不重发。

## 证据与判读

- `events.jsonl`：UTC 起始时间、输入参数、随机命令 ID、收发帧、阶段提示、逐秒样本数/非零数/均值/RMS/去直流 RMS/峰值/削波数、原生与转换后采样游标及连续性。阶段标签按主机观测时间划分，不是精确声学切点。
- `report.json`：完成/错误状态、各阶段统计、ACK、是否尝试握手后的只读版本补查、板端事件数、最后采样时序及关闭后的采集状态。
- `capture.s16le`：仅在显式 `--save-pcm` 时创建，16 kHz、单声道、有符号 16 位小端，无文件头；软件增益为 1。新目录权限 0700，文件权限 0600。默认不保存音频；开启后约每秒 32 KB。使用逐阶段 `total_samples` 边界定位试听片段，并检查语句是否跨过阶段边界；必要时保留上下文或完整录音，不能把阶段标签当声学切点。工具自身不播放或上传音频。

工具要求原生 16 kHz 单声道且无重采样，持续保留同一采集流；保存的是采集后 i16 PCM。原生流重开、游标缺口、连续性变化、实际 PCM 接收队列丢帧、后端故障或连续 2 秒没有 PCM 都中止并保留错误记录。辅助原生 reference 队列是非阻塞观测副本，丢块单独记录为 `native_reference_loss`，报告的 `native_reference_complete=false` 及累计计数明确表示旁路证据不完整；它不直接等于保存 PCM 缺失，不替代实际 PCM 游标检查。**持续收到全零 PCM 不会触发“没有 PCM”的超时**，它会保留为静默证据。Ctrl-C/SIGTERM 会取消观测、停止并 join 采集线程、关闭 UART，报告不会记为完成。SIGKILL 或磁盘写入失败可能只留下部分 JSONL，应视为未完成。

`observation_completed: true` 只代表有界观测过程结束，**不代表有效语音验证通过**。分别审查：

| 要回答的问题 | 所需证据 |
| --- | --- |
| 命令是否被接受 | 一次发送尝试、匹配 ID、数值 `code:0`；ACK 不是声学效果 |
| 是否连续得到 PCM | 实际 PCM 样本游标与连续性无缺口、无后端/PCM 队列错误、各阶段样本数与观测时长相符；旁路完整性另列 |
| 静默后能否恢复有效语音 | 保存音频中，静默前后普通句子均可听懂、无明显截断；峰值或非零 PCM 不能替代试听 |
| 是否仅靠手动唤醒 | 冷启动现场记录、从上电到实验结束未说唤醒词/未运行其他唤醒工具、全部板端事件复核 |

报告始终保留 `audio_validity: not_established_by_statistics`、`human_audio_review: not_performed` 和 `cold_boot_verified_by_tool: false`。人工结论应另附现场记录，不改写原始证据。没有 `--save-pcm` 时本次只能回答传输及能量问题；要验证可懂度，未来受控实验应显式保存并人工试听。

串口解析沿用现有校验和与重同步逻辑。`uart_partial_discards` 表示超时丢弃的不完整帧，出现时不能声称完整观测了控制通道；`no_board_event_observed` 也不证明上电至打开串口之前没有唤醒。该工具不测 AEC、真实模型效果或超过所选时长的常开能力。

2026-10-24 最新离线结果：macOS example 21 项、无网络 Linux ARM64 example 24 项通过；格式、Linux 构建、strip 与 `--help` 检查通过。初始准备阶段的现有 R1 回归 23 项、ALSA `null` 采集关闭/join 测试 1 项也通过。Clippy 普通检查完成时新 example 无诊断；`-D warnings` 被既有库代码的 43 条告警阻塞，后续 example 修正未重复该检查。版本日期双空格、冷启动握手后有界版本补查、旁路丢块与 PCM 缺口的区分、45 秒说话边界均有回归覆盖。完整记录及各版二进制哈希见 `run/r1-manual-capture-20261024/verification.json`。

实机前两轮原始证据保持不变：cold-01 在握手后的版本查询阶段中止，未发送手动唤醒；cold-02 单次手动唤醒成功并保存 112.28 秒连续 PCM，但因旧版工具将辅助 reference 丢块当作失败而提前结束。cold-02 的第 5–40 秒上下文片段已由操作者确认句子完整、声音清楚；其静默恢复阶段未执行，不能计为 5 分钟恢复通过。新版扩大说话窗口，保留旁路缺失记录且继续严格核对实际 PCM 连续性。

cold-03 完整实机结果：操作者重新断开两根 USB、确认灯灭至少 10 秒，主机核对两个设备消失后再接回；原固件精确匹配，beam 0，仅发送一次 `manual_wakeup` 并收到匹配的数值 `code:0`。保存 6,328,960 样本（395.56 秒，16 kHz 单声道 S16_LE），其中静默阶段 300.02 秒；同一流、采样游标连续、无后端故障、无实际 PCM 队列丢帧。辅助 reference 队列少了 1 块/320 样本（20 毫秒）副本，报告保留 `native_reference_complete=false`，不能宣称旁路证据完整。397 个窗口和 6 个阶段的统计均独立重算匹配，WAV 的 PCM 与原始切片逐字节一致。

操作者试听确认前后两段普通句子均完整清楚，且中间始终安静、未播放音频或说唤醒词。原始自动报告保持“统计不能证明有效语音/未人工试听”，人工结论另存 `live/cold-03-human-review.json`，综合结论见 `live/summary.json`，连续性审计见 `live/cold-03-independent-audit.json`。音频位于 `live/cold-03/analysis/{full,speech-before,speech-after}.wav`。实验后已核对 UART/PCM 无占用且 PCM 状态关闭；未改固件、混音器或生产配置，未连接真实模型。

本次结论限于该设备、当前固件、beam 0 和现场位置的一次完整冷启动/5 分钟静默验证，不推断其他方位、无限时长、真实模型或 AEC 效果。


设计依据：[R1 系统控制拾音调研](../../../../docs/architecture/r1-system-controlled-capture-research-2026-10-23.md)。
