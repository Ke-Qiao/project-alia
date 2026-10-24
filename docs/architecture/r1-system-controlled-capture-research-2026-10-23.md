# R1 由 Alia 控制采集、取消固定口令的可行性调研

调研日期：2026-10-23；实测补充：2026-10-24，Asia/Shanghai。对象：Yahboom 销售的聆构 ListenGo R1 六麦环形阵列，现场固件 `UART_REAL, circle6:1.9.6.f8ef651@master engine:circle6.1004`，构建时间原文为 `Sep  3 2026 22:40:02`（`Sep` 后有两个空格）。

10 月 23 日原始调研只做资料、代码和旧镜像提取件的离线核对；10 月 24 日在独立授权窗口完成冷启动、单次手动唤醒及五分钟静默后的音频验证，结果见第 7.1 节。用户已授权按“冷启动后单次 `manual_wakeup`、固定 beam 0、随后持续采集”推进纯 Rust 生产接入，软件接入和离线回归已完成，见[生产接入记录](../operations/software-release.md#r1-系统控制采集软件接入2026-10-24)。用户随后明确不重复插拔、beam 0 讲话和试听，因此直接复用已有硬件基线。系统 Start／Stop 的真实 Pi→Server 传输与同身份普通进程重启已现场通过（第 7.2 节），见[最新现场记录](../operations/software-release.md#r1-系统控制传输与普通重启现场验收2026-10-24)。

**R1 当前软件接入与分阶段验证已收口，回到 Alia 主线整理和交付。** 生产单次初始化与真人语音等同轮组合缺口，统一留到真实模型方案确定后，集中执行一次“生产启动 → 讲话 → 模型处理 → 回复播放 → 打断／恢复”完整验收，不再安排零散硬件补测。固件改造继续暂缓；AEC 等商家确认后单独验证。阶段收口不代表真实模型或全部声学验收通过。

## 结论与建议

**当前固件在本次冷启动、固定 beam 0 和同一讲话位置下，只需一次 `manual_wakeup`，五分钟静默后仍能取得完整清楚的处理后 UAC 人声，无需“小飞小飞”。这支持先完成纯 Rust 主机接入；任意方向、任意静默间隔、全天运行或异常重启尚未由本次验证覆盖。** [F4]、[F5]

1. **采用“确认冷启动后单次初始化＋现有 UAC 持续采集”，并单独控制 Alia 会话。** `cold-03` 只发送一次 beam 0 的手动唤醒，收到匹配的成功 ACK，未观察到有效自然唤醒事件；静默前后音频均获用户评听确认。beam 0 是固定方向，不是全向或自动定位。不得在每次会话、重连、进程重启时自动重发，更不能因 ACK 超时盲目补发。[P1]、[P3]、[F4]、[F5]
2. **USB PCM 常读已经存在；系统会话触发是独立的软件改动。** 10 月 23 日核对的启动流程打开采集并持续读 AudioBus，只有 `wakeword.detect(pcm)` 才建立新 VoiceSession；当时没有可直接调用的 `TriggerCapture`／`StartListening` RPC。改成 `wakeword="noop"` 本身不会自动变成常听模式。现已通过独立 `speech.trigger=system` 与签名 CaptureControl 复用会话 owner，并完成第 7.2 节的传输／重启验收；原始代码调查与后续实现分开记录，不伪造自然唤醒或方位。[C1]、[C2]、[C3]、[C4]
3. **原始六路音频是有公开依据的另一条无刷机候选。** `get_original_audio` 控制独立 CDC 音频流，不控制处理后 UAC。它有机会绕开处理输出的唤醒策略，但 1.9.6 的冷启动无唤醒行为、完整帧格式和持续性尚未实测；不能直接继承原有降噪、波束及 AEC 结果。[P1]、[P2]、[S1]、[F1]
4. **全向／自动跟踪／常开处理输出应优先询问受支持配置，尚不必索要全部 DSP 源码。** 旧 1.9.3 镜像包含与全向 VAD 输出、关闭唤醒后端有关的配置线索，说明“现成例程只演示唤醒”并不等于“引擎物理上只能这样工作”；但这些线索不是 1.9.6 的公开控制合同，不能直接写配置或猜命令。[I1]、[I2]
5. **如果产品硬要求不接受固定波束或不明超时，采用有明确连续音频合同的 UAC 前端，比反复补发手动唤醒更合适。** 可先用普通 UAC 麦克风解耦 Alia 会话逻辑，再按远场、方向与 AEC 要求评估处理型阵列。替代硬件仍需现场验收，不能用其宣传指标替换当前实测。

历史自然唤醒验收保持原范围和“不发送 beam”的记录；本次单次手动唤醒是另一独立授权与验证。两类证据不能拼成任意方向、长稳或生产对话已通过的结论。

## 1. 证据等级与本次核对范围

| 级别 | 本报告的用法 | 能证明的范围 |
| --- | --- | --- |
| 现场记录 | FYP 已有实测、Alia 10 月 21／23 日记录，以及 10 月 24 日独立 `cold-03` 与系统控制传输／重启验收 | 各次设备、版本、命令与音频现象分别记录；不能跨轮拼成整链验收 |
| 厂商文档 | 所提供资料包内协议、规格书、教程、白皮书 | 文档明确描述的接口／设计能力；没有标注适用版本的说明不能直接升级为 1.9.6 保证 |
| 供应代码 | 本地资料包里的 Python／ROS2 源码 | 主机如何发送和解析；不等于板端实现，也未声称与厂商最新在线仓库逐字节一致 |
| 镜像静态证据 | 既有 1.9.3 提取件及提取清单 | 旧镜像中的文件、注释和符号；不是当前 1.9.6 的运行配置 |
| 本次推论／建议 | 从上述材料推导的方案与验证步骤 | 需要试验或厂商确认，不能标为已支持／已通过 |

10 月 23 日调研重新计算了资料包中八份 PDF 的 SHA-256，并重新执行文本提取；均与既有 [PDF 清单][M1]及其提取文本一致。另渲染核对了协议第 7 页、波束教程第 4 页、录音教程第 5 页、规格书第 6 页、白皮书物理第 4 页。页码以 PDF 物理页为准。白皮书文件名含 V1.0，但内部修订页写有 V1.1／2026-04-02，引用时不以文件名推断版本。

Alia 代码核对基于本工作区内容，核对时 HEAD 为 `d2b7b6f12000535a1896dfcf7a2f4be4a2866e97`；工作区已有大量未提交变更，HEAD 不代表所读全部文件。关键文件哈希列于末尾。FYP 旧交接中关于 AEC 的故障措辞不作为本报告定性，采用 Alia 后续用户更正：模块不列为故障，AEC 等商家确认。[F3] 主任务仍在推进，其后续现场结果应以原记录为准，本报告不改写其验收结论。

## 2. 必须分开的四层，以及“方向”的含义

| 层次 | 当前能控制／观察什么 | 不能据此声称什么 |
| --- | --- | --- |
| 主机 USB／ALSA 采集 | 打开 UAC、连续读到帧、检查格式／时钟／xrun；Alia 启动时已做 | 读到 16000 样本不保证样本里有可懂的人声 |
| 板端处理与输出策略 | R1 先处理六麦，UAC 提供处理后单声道；现有记录显示唤醒、方向会影响有效人声 | 主机把 PCM 流保持打开不等于关闭板端输出门控；“门控”也不一定是明确的全零开关 |
| 板端唤醒与方向 | 自然唤醒事件给出当时 angle／beam；手动命令可指定 0–5 波束 | 手动命令不证明测出了新角度；一次 angle 不是连续跟踪；beam 0 不是全向 |
| Alia 会话与上传 | 是否建立 VoiceSession、消费预卷、连续上传、停止／重连，由主机逻辑决定 | 建会话或发布一个软件事件不会使板端恢复已经抑制的人声 |

```mermaid
flowchart LR
    M[六个麦克风] --> D[R1 处理与方向／输出策略]
    D --> U[UAC 处理后 mono PCM]
    U --> A[Alia ALSA 与 AudioBus 常读]
    A --> S[Alia 会话准入与连续上传]
    D --> W[自然唤醒 UART 事件]
    W --> S
    H[系统会话触发，CaptureControl Start／Stop] -. 单独建立会话 .-> S
    I[确认冷启动后单次初始化] -. UART manual_wakeup，固定 beam 0 .-> D
    M -. 原始音频开关控制，1.9.6 行为待测 .-> R[独立 CDC 六路原始音频]
```

本报告将“门控”用作**有效语音输出受状态影响**的工作假设，不声称已经定位到具体 DSP 模块或固定静默超时。FYP 记录过未自然唤醒时基本无声；Alia 10 月 23 日记录中，再次自然唤醒后后续短句恢复，支持这一解释，但方向抑制、降噪和其他链路状态仍需隔离，不能唯一归因。[F1]、[F2]

白皮书描述 360°声场覆盖与自适应跟踪，波束教程却明确说明选中方向增强、其他方向削弱；现场也观察到说话人走到对侧变弱、返回恢复。这几条可以同时成立：环形阵列能覆盖各方向，不等于当前输出同时以相同灵敏度拾取所有方向。公开协议没有给出连续 DOA 订阅、自动跟踪模式切换或当前波束只读查询，当前 Alia 也只消费唤醒时方位。[P3]、[P4]、[F1]

“全向”至少要明确是全部方向同时可听、单一目标自动跟随，还是选择一个固定扇区；三者不能互换。手动轮询六个 beam 也不等于同时全向：会切换输出、丢失其他扇区的时间片，并可能打断前端状态，现有协议没有稳定时间或无缝切换保证。

## 3. 现固件可用的控制合同

### 3.1 端口与消息

现场身份来自既有记录：UAC USB 为 `2208:0001`、`EII ListenGo Circular 6-Microphone`；处理后采集为 `hw:CARD=L6Microphone,DEV=0`，16 kHz／S16_LE／mono。CH340 控制为 `1a86:7523`，历史稳定路径 `/dev/serial/by-id/usb-1a86_USB_Serial-if00-port0`，115200、8N1、无流控。UAC 同一 USB 设备还暴露 CDC ACM；历史名 `/dev/ttyACM0` 只是枚举名，未来须按 R1 的设备身份／USB 拓扑确认，不能按第一个 ttyACM 打开。[F1]

控制帧为 `A5 | 01 | type:u8 | length:u16le | id:u16le | payload | checksum`，总长 `payload + 8`，完整帧各字节之和模 256 为 0。`0x01` 握手的 `A5 00 00 00` payload 以 `0xFF`、相同 ID 应答；命令／ACK 为 `0x05`，自然事件通常为 `0x04`，原始音频为 `0x06`。命令成功必须核对相同 ID、数值 `code=0` 和响应内容，不能只看 write 成功。协议 payload 上限 10232 字节；Alia 现控制解析器有意限制为 1024，不能直接把它当原始音频解析器。[P1]、[C5]

以下仅列协议 payload，不是串口逐行 JSON；实际执行命令与次数以各轮事件证据为准：

| 请求 | 文档支持的用途 | 当前证据／缺口 |
| --- | --- | --- |
| `{"type":"version"}` | 查询固件 | 当前 Alia 已实现匹配 ID 的版本确认 |
| `{"type":"manual_wakeup","content":{"beam":0}}` | 手动唤醒／设置波束；beam 数值 0–5，教程说立即生效 | `cold-03` 单次成功 ACK 后取得有效人声，五分钟静默后仍清楚，未观察到有效 `0x04` 唤醒事件；更长保持、其他方向及重启后人声未验；普通主机进程重启的传输验收见第 7.2 节 |
| `{"type":"get_original_audio","content":{"audio":1}}` | 打开原始音频流；0 为关闭 | 开关对象是原始音频传输；不是 processed UAC start/stop，更不是物理麦克风断电或 AEC 开关 |
| `{"type":"wakeup_keywords","content":{"keyword":"小爱同学","threshold":"900"}}` | 替换一个中文关键词 | 仍然需要声学口令，不满足系统直接控制；文档要求 threshold 为字符串、500–1500，配置需断电重启生效；本次不改 |

未在本次审阅的合同中发现受支持的 `always_on`、`disable_kws`、`start_uac`、`stop_uac`、`sleep`、全向或自动跟踪切换命令。这个结论只限定“已审阅合同”，不能推出厂商私有 SDK 或定制固件没有对应能力。不能把通用 AIUI SDK 命令编号直接封进 R1 串口后声称支持。也不采用空关键词或越界阈值来尝试关闭唤醒；关键词教程明确警告 threshold 传成数值可能使固件卡死。[P6]

### 3.2 手动唤醒的证据到底到哪里

[协议第 7 页][P1]把 `manual_wakeup` 示例的内部步骤写为“调用底层接口设置波束”，[波束教程][P3]也用同一个 payload；供应 Python 的 `send_manual_wakeup()` 与 `send_set_beam()` 只是固定 beam 0 和传入 beam 的区别。[供应 ROS 服务][S2]的 `set_beam` 同样发送该命令并等待 ACK。因此“手动唤醒”和“设置波束”不是已证明相互独立的两个板端 API。

FYP 会话 `d3a2c11f15bf4fd395f64b1831824aab` 记录了 beam 0 成功应答，**随后自然唤醒**报告 25°／beam 0，之后 73 个完整 UAC／ROS PCM 块一致且录音获接受。它证明正确波束可解决该方向弱声，不能排除真正打开／刷新输出的是随后自然唤醒。该记录中的 raw 指主机未改动的 UAC 字节，不是六路 CDC 原始麦克风音频。[F1]

10 月 24 日 `cold-03` 补上单次 manual 的独立证据：确认断电重连后，不说唤醒词、不播放音频，仅一次 beam 0 命令；同一 UAC 流在 300.02 秒静默前后均保留了完整清楚的人声。该轮没有自然唤醒对照、方向扫描或门控机制测量；不能由此断言固件内部如何保持状态，也不能认定进程重启等价于板端冷启动。[F4]、[F5]、[F6]

若采用系统触发，不能等待一个没有被文档保证的自然 `ivw.keyword` 事件来开始会话；手动命令即使返回额外事件，也不能标为新测得的声学方位。命令发生超时且缺乏状态读回时，结果应记为未知，不自动高频重发。

### 3.3 原始音频端点

[规格书第 6 页][P5]说明 UAC 数据线同时带标准单通道音频与虚拟串口。[录音教程][P2]明确区分串口六声道原始数据和 UAC 处理后单声道；六路教程只要求启用原始音频，未写必须先说口令。后一点是**验证无唤醒原始路径的依据**，并非 1.9.6 的冷启动保证。

本地供应代码明确分开控制口和原始音频口：控制默认 ttyUSB，音频默认 ttyACM，控制口上的音频帧被忽略。控制口发送开关，CDC 接收有协议封装的 `0x06`，每 16 字节样本组取前 12 字节，按六路、16 位、16 kHz 解释，后 4 字节被文档／示例称为控制信息。[S1]

有四个必须保留的边界：

- 按文档计算，六路音频净数据为 `6 × 16000 × 2 = 192000 B/s`；含 16 字节组为 `256000 B/s`，还未加消息封装。物理 115200／8N1 UART 约只能承载 `11520 B/s`，因此不能通过 CH340 控制 UART 搬运该流；CDC 的波特率设置不能等同物理 UART 带宽。
- 后 4 字节没有足够语义，**不能猜成两路 AEC 参考、时间戳或校验**。旧镜像内部另有六麦＋双参考 ALSA 组合，也不能据此改写 CDC 的外部格式。
- 示例在每个 payload 内按完整 16 字节组循环，尾部不足一组就丢弃。生产接入必须明确跨消息对齐、丢帧／重同步和停启边界；有界缓存、校验、真实样本计数不可省。协议 ID 可能与其他消息共享，未确认计数域前不能把所有跳号都定性为丢样。
- 六路原始输入不等于原有处理后 mono。必须明确选取哪个物理通道、或使用何种经验证的处理；直接平均六路不能保证获得全向增强，且不提供原 DSP 的 AEC／降噪。当前 Alia 的 converter 会对 native 多声道取平均，多声道设备不可未经选择就接进去。[C6]

## 4. Alia 的系统触发现状与最小 Rust 改动边界

下表保留 10 月 23 日调研时的代码现状；10 月 24 日用户已授权生产接入，生产软件与回归现已完成，系统传输／普通重启现场结果见第 7.2 节，历史代码表不回写；独立工具和静音传输均不能代替生产初始化与真人语音的同轮组合验收。

| 已核对入口 | 实际行为 | 对系统触发的影响 |
| --- | --- | --- |
| `EmbeddedModuleRegistry::start` | 启动 audio capture，调用 `start_capture(&AudioBus)`，再启动 speech | 主机 PCM 常读不需要“小飞小飞”；限制发生在后续音频语义和会话准入 |
| `Runtime::accept_frame` | 先跑 VAD；有会话就上传，没有会话则等待 `wakeword.detect()` | VAD active 本身不会从 idle 建立会话 |
| `ModuleEvent::WakeWordDetected` | 当前路径在检测成功后发布；运行循环对其他普通事件忽略 | 往 EventBus 伪造该事件不是现成的系统触发接口 |
| `R1WakeWord` | 单 UART owner，只发送版本查询和握手 ACK；`reset()` 清理主机代际／待处理数据 | reset 不唤醒、不使板休眠、不改变 beam；版本健康不等于声学可用 |
| `WakeWordSource`／noop | 只有 Local 和 R1；noop 的 detect 返回 None | 当前没有 System 来源；noop 不是 always-on 开关 |
| `VoiceSession.Stream` | Embedded 发起 OpenSession 后传 PCM；InitiativeAdmitted 带已有 session 与采集水位 | 主动输出准入不是从无会话状态启动 R1 麦克风的反向指令 |
| HTTP `/voice/chat`、`/voice/transcribe` 等 | 处理请求已带来的音频／文本 | 不打开远端 R1，也不发送板端控制命令 |

证据：[启动流程][C1]、[会话分支][C2]、[speech 生命周期][C7]、[事件来源][C3]、[noop][C4]、[UART owner][C8]、[VoiceSession 协议][C9]、[HTTP voice 路由][C10]。

基于第 7.1 节的固定方向结果，接入保持现有 ALSA → AudioBus → Silero → VoiceSession：

1. 在已有语音 runtime 增加一个明确的系统开始／结束请求入口，来源可为应用按钮或已有可信本地决策。由同一会话 owner 建立／关闭会话，复用当前 start、取消、scope、队列、预卷与重连隔离；已有会话中的重复开始不另建会话。不要另起一套音频上传器。
2. 通过现有 R1 UART owner 提交有界命令，匹配 ID／数值 ACK；不要用第二个工具进程打开同一串口。仅在已确认的板端冷启动初始化阶段发送一次 beam 0 命令，随后会话开始、主机重启和连接恢复均不能自动再次初始化。命令等待不进入 native 音频 callback；超时或部分写入可表示结果未知，不自动重发或加入周期性保活。版本查询允许复用已验证的冷握手处理：握手 ACK 后最多补一次只读版本查询，沿用原 ID 和原始两秒绝对期限。
3. 把系统请求、板端命令接受、PCM 流连续、实际语音可用分开记录。正常静默时没有人声是合法状态，不能用“峰值非零”宣告声学就绪；也不能把一个 ACK 当作整条输入验收。
4. 系统触发使用真实的来源语义，不伪造“小飞小飞”或 R1 角度。停止窗口首先保证 Alia 停上传／清理会话；若需求还包括物理停止采集或停止 UAC，当前板端协议并未提供同等保证，应另定合同。外部远程入口若实际需要，沿用既有绑定／签名认证与请求去重，不开放通用串口命令透传。

最小影响面为 `speech.rs`／`speech/duplex.rs` 的控制入口与会话生命周期、`r1/mod.rs`／`r1/protocol.rs` 的命令事务；需要公开系统来源时再调整 `traits.rs` 和相关调用方。仅当选定模式确实需要配置或远程控制时，才扩展 builder／factory、配置示例或 proto／gRPC。应复用已有 R1 PTY、协议、external-wake 和 duplex fixture，覆盖冷连接、迟到 ACK、重复系统请求、自然唤醒竞争、停止／重连与旧音频隔离；这里列的是后续验证要求，不是本次已运行结果。

这些控制逻辑可用现有 Rust、libc／ALSA 和 serde 实现，无需迁入供应 Python／ROS 或新建守护进程。“纯 Rust 主机路径”不等于板端闭源音频算法也是 Rust；若以后替换板端应用并调用 `libvtn`，那会是 Rust 应用＋厂商 C ABI 的组合。

## 5. 方案比较

| 方案 | 无需刷机 | 是否满足不说“小飞小飞” | 保留处理后 UAC | 可靠性／方向边界 | 建议 |
| --- | --- | --- | --- | --- | --- |
| 只常开 ALSA／改 Alia 系统触发 | 是 | 软件会话可取消口令，但板端未知 | 是 | 现记录不足以保证有效人声；主机无法重建已被抑制的输入 | 不作为完整解决方案 |
| 系统触发＋冷启动后一次 manual_wakeup＋已知 beam | 是 | `cold-03` 在固定 beam 0 与五分钟静默范围通过 | 是 | 固定定向；其他静默时长、重开后人声／自然唤醒覆盖行为未验 | **Rust 接入及系统传输／普通重启现场通过；初始化＋真人语音同轮待验** |
| 周期性 manual_wakeup／扫描六 beam | 是 | 可能表面免口令 | 是 | 无公开重入、超时、保持及无缝切换保证，可能切断语句或夺走方向 | 不作为默认或可靠常开设计 |
| 改关键词／先自然唤醒一次 | 是或按文档需重启 | 改词仍有口令；先唤醒也不满足冷启动无口令 | 是 | 当前自然唤醒路径已验证；长静默后连续对话未由此保证 | 可保留现状，不能算目标完成 |
| Alia 软件 KWS／VAD 接现 UAC | 是 | 软件 KWS仍有口令；VAD可无口令 | 是 | 依赖首先拿到有效 PCM，不能绕过上游门控 | 只在输入可用性解决后选策略 |
| CDC 六路原始音频＋Alia 采集／会话控制 | 是，若 1.9.6 已支持其无唤醒输出 | 候选，需冷启动验证 | 使用另一输出路径 | 格式／丢包／通道／处理待确认；不继承处理后 mono 音质 | 固定方向方案失败后的独立可行性分支 |
| 厂商确认现 1.9.6 的常开／全向配置 | 可能无需换镜像；配置改动仍是板端变更 | 厂商须给出明确模式和示例 | 目标是保留 | 必须注明版本、静默策略、方向策略和退出语义 | 通用需求的优先厂商路线 |
| 厂商适配应用／新版固件或 Rust 替换板端应用 | 否或需要受支持的应用更新 | 理论可实现 | 取决于真实音频 API | 需要 1.9.6 恢复包、SDK／ABI、校准与验收 | 继续暂缓，本报告不恢复该工作 |
| 连续 UAC 麦克风／另一处理型阵列 | 不改 R1，但更换采集设备 | 可按新设备的合同实现 | 采用新设备输出 | 新的拾音、方向、声道及 AEC 验收 | 对无口令／移动场景硬要求的备选边界 |

固定安装、已知讲话方向、显式开始监听的场景，采用已验证的单次初始化路线。若要求机器人在未知方向发现人声，且不允许先用自然唤醒选方向，就不能把 beam 固定为 0 作为通用解法。可用视觉方向也仍需坐标标定与目标选择政策，当前唤醒角度不会自动提供这一能力。

## 6. 常开与全向的厂商线索，及替代硬件边界

### 6.1 旧镜像已经提供了更具体的询问方向

以下均来自已有 **1.9.3** 提取件，未在 1.9.6 上读取或改动。原镜像为资料包烧录 ZIP 内的 `v1.9.3_real_uart.img`，SHA-256 `3499abdc3e3d0b38904680c23d40112ab0b789a9193a8da228888e5e2debcdc5`；镜像配置不等于当前板端运行状态：

| 位置／线索 | 精确证据 | 不能推出什么 |
| --- | --- | --- |
| `cae_init_param.ini` | `bSPVadOn=false`；前一行注释明确把启用与“输出的 VAD 音频是全向降噪”联系起来 | 不能保证该输出就是当前 UAC；不能保证开关不影响音质、延迟或其他算法 |
| `vtn.ini` frontend | 注释有 `sp_vad_on`、`oneshot_type`，后者列出关闭、normal oneshot、含唤醒词输出等值 | 注释不是可直接发送的串口参数，也不是当前运行值 |
| `vtn.ini` backend | 注释示例 `[backend] on=0`；`rewake_mode` 区分全通道唤醒实例与当前波束实例 | 关闭后端是否保留前端处理回调、UAC输出未知；rewake 选择不是连续 DOA 跟踪开关 |
| `cae_init_param.ini` | `nChiefBeam=-1`、`nOutNum=3`；板内另有六麦＋双参考 ALSA 路径 | 内部 chief 索引不是公开 beam 0–5；不得向串口发送 -1 试图获得全向，也不能猜 CDC 后四字节为参考音频 |
| 板端程序／库 | 旧镜像由 `vtn_example` 使用 `libvtn.so`；有 init／interact／destroy／get_version 符号 | 符号不构成正确 Rust FFI 所需的参数、回调、线程与缓冲区生命周期合同 |

证据：[VTN 配置][I1]、[CAE 配置][I2]、[旧镜像审阅][I3]。这是值得向厂商提出的具体线索，不能把它们写成“把 bSPVadOn 改成 true 就解决”。板端配置替换／重启也会改变设备状态，并不因“没有重刷整张镜像”而属于本次授权。

后续若用户恢复厂商沟通，优先确认这五项即可，不先要求重写 DSP：

1. 对 **1.9.6.f8ef651／circle6.1004**，`manual_wakeup` 是仅设置波束，还是也打开处理后 UAC？静默、VAD结束、播放、自然再唤醒后分别如何变化？是否有状态读回和受支持的保持／退出方式？
2. 是否已有无需关键词的 processed UAC 常开模式，能从冷启动输出并在长静默后恢复正常说话？请给正式命令／配置、版本范围和可运行示例。
3. `bSPVadOn`／`sp_vad_on` 所指音频能否送到 UAC？全向、自动跟踪、固定波束的实际区别、切换与当前状态如何读取？关闭 backend／oneshot 后是否仍保留处理输出？
4. 当前固件 CDC 原始流能否冷启动直接开关？六路顺序、采样格式、16字节组后四字节、跨帧对齐、ID计数域、丢包／溢出和 UAC 共存规则是什么？
5. 只有当现版本配置无法满足时，再补充匹配本板 revision 的 1.9.6 恢复包及校验、支持的应用替换路径、VTN 头文件／回调约定和资源配置，沿用现有[厂商资料草稿][V1]。

本报告没有发送消息，也不将这份补充问题写入已有草稿。AEC 模式／电气／性能仍由现有商家沟通负责，不据 `aec_on=true` 判定现场 AEC 已有效。

### 6.2 硬件替代要替换的是合同

如果核心目标只是“Alia 决定何时开始／停止监听”，普通连续输出的 UAC 麦克风可作为会话机制的独立验证输入；选择的具体设备仍须确认无关键词门控。它不自动具备 R1 的远场增强、DOA 或 AEC，不能拿近讲成功验收远场机器人。

若仍需要阵列处理，代表性候选是 **reSpeaker XVF3800 USB 4-Mic Array 的 USB 版本**。Seeed 官方文档直接提供 Linux 录音流程及 DOA／VAD／处理参数控制；USB 与 I2S 变体及固件配置不同。该文档支持把它列为连续主机采集的候选，未构成本项目的冷启动、长稳或 AEC 验收。[Seeed 官方说明](https://wiki.seeedstudio.com/respeaker_xvf3800_introduction/)、[官方固件与主机控制仓库](https://github.com/respeaker/reSpeaker_XVF3800_USB_4MIC_ARRAY)。

评估替代设备时，先取得出厂固件与明确的通道表：哪一路是 ASR、哪一路是其他处理输出、哪几路是 raw，采样格式以实际描述符为准。当前 Alia 的平均降混不能直接混合这些语义不同的通道。单独确认冷启动无需关键词、静默后输入、移动说话的策略、设备身份和重连；AEC 则单独确认参考来自何处、播放路由与采播时钟。不能假定现 C-Media 模拟参考线可直接搬到另一阵列。

这是替代边界说明，不是购买、刷写或新增依赖决定。官方示例使用 Python 不限制 Rust 实现；只有必要的控制协议完整且可验证时，才决定是否需要额外 USB 控制接入。

## 7. 可复核的验证顺序

以下为 10 月 23 日制定的验证矩阵。10 月 24 日已在独立窗口执行其中单次 manual 与五分钟静默部分（第 7.1 节），另完成系统传输及普通主机进程重启（第 7.2 节）；其余未覆盖步骤保留为历史待测设计，不作为当前待执行清单。后续完整验收按本页顶部的模型方案条件集中安排。执行时须确认 UART／ALSA owner 已释放，不与现有 owner 并行。只改变一个变量，保留固件／设备身份／原始结果，不以修改生产超时、提高主机增益或重复说口令掩盖差异。冷启动与重开主机进程必须分别记录；停止主机采集不等于冷启动，实测断电步骤同时断开 UAC 与 CH340 USB。

独立 Rust 验证入口已经准备并用于第 7.1 节，复用现有采集与 UART 代码。验证记录单调时间、固件、请求／ACK ID、自然事件是否出现、PCM 样本数／峰值／RMS／全零比例、连续性和 xrun；本轮用户另行授权保留音频并评听。噪声非零和 VAD 判断均不能单独证明人声清楚；语音可懂性依赖人工确认。该验证不依赖真实 STT／LLM，也不追加 AEC 试验。

| 步骤 | 单一变量与操作 | 需要观察的结果／判定 |
| --- | --- | --- |
| A：冷启动对照 | 已知讲话方向，真正冷启动；仅现有握手／版本与 UAC 采集，无关键词、无 manual_wakeup | 确认帧是否连续、固定测试语句是否可用；没有人声时记录为板端路径未知，不只凭 PCM 有字节判通过 |
| B：仅系统命令 | 从同等冷启动条件开始，发送一次目标 beam 的 manual_wakeup；在整个窗口中不说任何唤醒词 | 同 ID 成功 ACK；对比 A 的人声可用性和首字／尾字；记录是否出现额外事件；若仍须自然唤醒，则该最小候选不成立 |
| C：静默与持续性 | B 成功后不重发命令；讲话、停顿、再讲话 | 原计划包括 5、30、60、300 秒静默及一次 20–25 秒语句。第 7.1 节只覆盖其中五分钟静默前后讲话，不代表其他间隔或长句均通过；需分别判断门控恢复与持续讲话截断 |
| D：方向隔离 | 同等距离与声级，先在已选扇区讲话，再移动到邻侧／对侧；不改 beam、不说口令 | 区分正常定向抑制与时间门控；若需移动场景，必须按该场景验收，不能用正前方成功代替全向 |
| E：恢复与竞争 | 分开验证主机关闭／重开、USB重连、完整断电；最后另测一次自然唤醒对人工波束的影响 | 每轮重新核对版本和状态；重复系统请求不建多会话；旧 ACK／事件／预卷不得污染新会话。自然唤醒竞争测试不能混入 B 的无口令证据 |
| F：raw 分支 | 若需要该路线，在冷启动、尚未 manual／自然唤醒时打开独立 CDC，再用 get_original_audio=1；结束用 0 | 验证六路实际波形／通道顺序／速率／尾部处理／缺口，以及关闭后新音频确实停止；同时判断是否影响 UAC。不能用 ACK 或缓冲旧数据判“已开／已关” |
| G：Alia 整链 | 选定且通过的输入路径接回正常认证和 VoiceSession，由明确系统入口启动／结束 | 从无会话开始，不依赖伪造 UART 事件；每次触发最多一个会话；取消、断线、重新触发、长句及静默后短句均使用真实 PCM；保持主任务的生产期限和原验收口径 |

验收应分别出结果：①接口接受；②无口令音频可用；③方向范围；④静默／重连后的恢复；⑤Alia 会话触发与结束。建议多个冷启动轮次分别记录成功／总次数，不能拿一次成功外推永久常开。若 B 只能短时间有效或 D 只支持一个扇区，只能发布该受限能力；若通用需求必须覆盖未知方向／移动／长静默，则转向厂商常开模式或硬件替代，不通过定时伪唤醒宣告通用化完成。

### 7.1 2026-10-24：独立冷启动 `cold-03` 实测

本节同步已完成的独立验证记录，没有重新访问 Pi。验收证据由[现场汇总][F4]、[人工评听][F5]、[独立重算][F6]、[原始审计][F7]和[设备释放检查][F8]共同组成；其中审计只判断协议与 PCM，不代替用户听感。R1 序列号为 `2c000c70f5c2c761891`，UART 为 `/dev/serial/by-id/usb-1a86_USB_Serial-if00-port0`，采集为 `hw:CARD=L6Microphone,DEV=0`。

冷启动依据为用户确认同时拔下 UAC 与 CH340 两根 USB、LED 熄灭至少 10 秒，加上工具观察两设备消失；再接回原端口、等待至少 15 秒并核对重新枚举。工具没有直接测量电压，因此准确表述是“操作确认与 USB 消失／重现支持冷启动”，不能说软件直接证明断电。完整版本回复为：

```text
version: UART_REAL, circle6:1.9.6.f8ef651@master engine:circle6.1004; build date: Sep  3 2026 22:40:02
```

| 检查 | `cold-03` 实际结果 | 解释边界 |
| --- | --- | --- |
| 命令 | `manual_wakeup` 仅一次，beam 0、ID `38653`；同 ID 数值 `code:0` ACK | beam 0 为定向选择；不是全向、不是本次自然声学定位 |
| 其他发送 | 首次版本查询、握手 ACK、同 ID 的一次只读版本重查 | 版本原始两秒期限未延长；没有周期命令或 manual 重试 |
| 自然唤醒 | 未观察到有效 `0x04` 唤醒事件；用户确认未说唤醒词 | UART 审计只包含成功解码帧，不等于证明每个接收字节都有效 |
| 保存 PCM | 单流、原生 16 kHz／mono／S16_LE，无重采样；6,328,960 样本，共 395.56 秒 | 保存区间样本游标连续；无广播丢失、后端故障或无 PCM 超时 |
| 分段 | 讲话前窗口 44.98 秒、静默窗口 300.02 秒、讲话后窗口 45.06 秒 | 45 秒是给提示延迟预留的窗口，不是声称连续说话 45 秒；分界基于保存样本，非校准声学时刻 |
| 人工评听 | 用户确认“两段都完整清楚；中间一直安静，未播放音频或说唤醒词” | 声学可用结论依赖此确认，不由 RMS、非零比例或阶段标签推出 |
| 独立重算 | 397 个窗口与 6 个阶段的全部统计重算一致；所有 WAV PCM 与原始切片逐字节相同，下载文件与远端哈希／长度一致 | 支持证据完整性；不扩展已测试方向、时长或环境 |
| 退出 | UART／PCM 无占用，ALSA 状态均 `closed`；关闭无后端故障 | 停止期间另读的 1,920 样本尾部在保存范围之外，不填补或解释为内部缺口 |

**辅助原生观测不完整：** `native_reference_complete=false`，累计缺失 1 块、320 样本，即 20 毫秒。该计数来自使用 `try_lock` 和有界队列的 native-reference 旁路副本；本次证据无法区分锁竞争或容量不足。它不表示实际 `AudioFrame` 或保存 PCM 丢了 20 毫秒。保存 PCM 的游标、流身份、连续性、广播丢失、后端错误及无 PCM 超时仍独立严格检查，不能因旁路允许不完整而放宽真正的采集故障。独立审计保留该缺失，没有将旁路标为完整。

音频按用户授权留存于同一证据目录，均保持原始幅度：

- [完整 395.56 秒 WAV][A1]：含版本、基线、ACK、前讲话、静默与后讲话六阶段。
- [静默前讲话窗口][A2]：44.98 秒，主要语音评听范围约为文件内 9–24 秒。
- [静默后讲话窗口][A3]：45.06 秒，主要语音评听范围约为文件内 15–36 秒。
- [原始 S16_LE PCM][A4]、[UART／采集事件][A5]、[原始运行报告][A6]用于复核。完整 PCM SHA-256 为 `ac40999d8abb54c64f85cb3f82d7f6ee55aa78e76367c20a8d4d9672935a5a4a`。

历史未完成轮次保留：`cold-01` 在初始握手后版本查询超时，manual 发送数为 0；`cold-02` 发送一次 manual，旧工具将辅助观测缺失当作致命错误，于 112.28 秒结束，未验证静默恢复。不能把两轮与 `cold-03` 拼接成一个成功过程，也不能记为多轮完整通过。

独立工具的[验证记录][F9]显示 macOS 例程测试 21 项、Linux ARM64 例程测试 24 项通过；既有 R1 回归 23 项和 ALSA null 停止／join 1 项保留各自执行范围。Rustfmt、Linux 构建与 strip 后 help 通过。普通 Clippy 曾完成、例程无诊断，库内已有 43 个警告导致 `-D warnings` 未通过；后续例程调整没有重复 Clippy。最终验证 ELF SHA-256 为 `5bef80a9f6cd2884e45d0af6a6565a111ec3510115fb85efeba0eacc31d8074f`。这些是独立工具检查，不能代替正在进行的生产软件回归。

本次支持**单设备、当前固件、固定 beam 0／当前位置、一次冷启动和五分钟静默**后的免口令有效输入。没有验证其他方向、其他静默时长、全天稳定性、异常重启、自然唤醒覆盖人工方向、生产对话或 AEC；没有播放、接入真实模型、刷写固件或修改 mixer／生产配置。后续接入必须把冷启动初始化与 Alia 会话触发分开，停止会话不等于板端停止采集，主机进程重启也不等于新的板端冷启动。

### 7.2 2026-10-24：系统 Start／Stop 传输与普通重启

本节同步 [`run/r1-system-auto-20261024/summary.json`](../../run/r1-system-auto-20261024/summary.json) 的增量现场结果。
生产源码本轮未改，复用已冻结的 Embedded release；新增真实 Server 所有权回归后，Linux ARM64 完整 duplex **40/0/4**，
最终 `r1_live::remote_r1_system_control_transport` **1/0/0，29.00 秒**。
三轮系统 Start／Stop 取得真实 16 kHz mono PCM，Stop 收束样本分别为 **193600／17600／17600**；
鉴权拒绝、旧 generation／boot 拒绝、idle 不上传和任务／forwarder 清理通过。
同配置／状态的普通无参重启 gate 为 **3.417 秒**，身份相同、boot 更新；它不等于 R1 板端冷启动。

三轮 PCM 峰值为 **1／0／0**，没有语音；本轮没有运行初始化、打开 UART、保存音频或播放，
不证明重启后人声、声学质量或 Server 语义处理。生产单次初始化与真人语音仍缺同轮组合证据，
该缺口统一纳入模型方案确定后的完整验收。继续复用 cold-03 有限硬件结果，不自动安排重复插拔、讲话或试听，也不拼接成新整链通过。

首轮 live 因错过 60 秒重启 gate 为 **0/1，79.77 秒**，一次重试准备因 ANSI 公钥日志解析失败而未启动 Server；
修正编排后通过，未改变 Rust 时限或重新构建。共四个 Pi 进程均未打开 UART、clean exit 0，无强杀或 watchdog 超时。
最终 UART／PCM 与验证端口已释放，自有容器／网络／隧道清理完毕，数据库无非默认 schema／其他连接；mixer 保持 −14 dB。
完整产物身份、过程与清理证据见[发布记录](../operations/software-release.md#r1-系统控制传输与普通重启现场验收2026-10-24)。
当前阶段在上述范围内收口；固件改造继续暂缓，AEC 等商家确认后单独验证。本节不扩展到其他方位、全天稳定性或通用全向能力。

## 8. 来源与复核定位

### 本地主要来源

| ID | 来源 | 本报告使用的部分 |
| --- | --- | --- |
| P1 | [串口协议 PDF][P1] | 物理第 1–10 页；帧、命令、ACK、长度、可能丢数据；第 7 页的设置波束流程已渲染核对 |
| P2 | [音频采集与录制 PDF][P2] | 第 3、5、6 页；六路 raw／UAC mono、先在发声方向唤醒的说明、16字节组 |
| P3 | [控制拾音波束方向 PDF][P3] | 第 2–4 页；六扇区与命令立即生效，不是全向 |
| P4 | [聆构 R1 白皮书][P4] | 物理第 4–6 页；设计能力、跟踪宣传、默认词性能限定 |
| P5 | [聆构 R1 规格书][P5] | 第 6–7 页；独立 UART、UAC＋CDC、模拟参考接口 |
| P6 | [自定义唤醒词 PDF][P6] | 单个中文词、threshold 字符串、范围及重启要求 |
| F1 | [FYP 实测交接][F1] | §2、§4、§5；现场版本、UAC、方向、手动 beam 后又自然唤醒的证据边界 |
| F2 | [Alia 软件发布记录][F2] | 10 月 21／23 日已有输入与会话现场记录；不代表本调研重新验证 |
| F3 | [硬件部署就绪记录][F3] | 当前固件暂缓、自然唤醒方向和 AEC 待商家确认的范围 |
| F4–F9 | [独立现场汇总][F4]、[人工评听][F5]、[独立重算][F6]、[原始审计][F7]、[设备释放][F8]、[工具验证][F9] | 10 月 24 日 `cold-03` 的单次手动唤醒、音频、有限静默恢复与各自审计边界 |
| S1 | [供应 Python 串口工具][S1] | 25–26 行端口、371 行起原始组处理、520／575／658 行起开关与手动命令 |
| S2 | [供应 ROS 串口服务][S2] | 88 行起 SetBeam、202 行起 SetOriginalAudio；主机包装与 ACK |
| S3 | [供应 ALSA adapter][S3] | 90 行起开流、136 行起常读；持续读不等于关闭板端策略 |
| I1／I2 | [旧镜像 VTN 配置][I1]／[旧镜像 CAE 配置][I2] | 仅 1.9.3 静态线索，特别是 backend、oneshot、bSPVadOn |
| I3 | [旧镜像来源与提取报告][I3] | 1.9.3 镜像哈希、BSP／应用边界、内部 ALSA 路径与 ABI 缺口 |
| V1 | [现有厂商资料草稿][V1] | 保留的最小 SDK／恢复资料请求，未发送／未改动 |

在线补查日期为 2026-10-23。历史[亚博 R1 页面](https://www.yahboom.com/build.html?id=16444&cid=743)本次只显示站点框架，没有取得可补充的协议正文；不能把空页面视为产品能力否定。生产商 [EIINSIDE 官方公司页](https://www.eiinside.com/about.html)检索结果确认聆构 R1 属于其产品，直接读取曾返回 502，未取得公开 1.9.6 常开接口说明。替代硬件只引用第 6.2 节的 Seeed 官方文档与其直接链接仓库；第三方论坛、其他名为 R1 的产品及通用 AIUI 资料没有作为本机能力证据。

### 内容身份

八份 PDF 的全路径与哈希见 [既有 PDF 清单][M1]，10 月 23 日调研重算均相同。以下 SHA-256 标识当时所读关键代码／配置；它们用于将来复核，不是新发布产物：

| 文件 | SHA-256 |
| --- | --- |
| Alia R1 `mod.rs` | `f5c9416546a97af67d8f1d88b735c83dd82a25ca119c434b9fd153a9795fa462` |
| Alia R1 `protocol.rs` | `939c219f2ae0b02060d011d22ce8fc091501df7476fca4e70e0c736f6548917f` |
| Alia `speech/duplex.rs` | `ed02439c70524977261796a36212df7f74241c1df825eacdd10626c3e6dc9cb9` |
| Alia `module/registry.rs` | `c3b3654bef959c9f8e9f01d19a211d44ccfcdb53a8cacdb66d7ce13b6f719066` |
| 供应 `serial_reader.py` | `a9c176d7d545a9397631bbb75d3ddfa3eb884b92d72970b7c61ed3d1162b1ab1` |
| 旧镜像 `vtn.ini` | `f9130b06f2dd51c3eb9a31b7b7ba678b77bfc7adbcb45ecac2cb264a4599964e` |
| 旧镜像 `cae_init_param.ini` | `def5baf7da4bee467ec2858a2b20f690a3c20e03b1c6d0db9d3507d4002edc52` |

原始调研完成证据核对与方案评估；10 月 24 日补充的独立实测覆盖第 7.1 节范围，并已授权主线生产接入。仍未知的关键点是更长静默／重启／方向变化下的行为、CDC 冷启动无唤醒输出及常开模式的 1.9.6 适用性；AEC 不在本报告的验证范围。上表代码哈希继续标识 10 月 23 日调研快照，不代表后来验证工具或完成接入后的生产代码。

[P1]: </Users/ke-qiao/Developer/FYP/R1麦克风阵列模块/新资料（20264.22及之后下单的客户看这里）/6.附件/串口协议/串口协议.pdf>
[P2]: </Users/ke-qiao/Developer/FYP/R1麦克风阵列模块/新资料（20264.22及之后下单的客户看这里）/3.模块基础使用/2.音频采集与录制/音频采集与录制.pdf>
[P3]: </Users/ke-qiao/Developer/FYP/R1麦克风阵列模块/新资料（20264.22及之后下单的客户看这里）/3.模块基础使用/4.控制拾音波束方向/控制拾音波束方向.pdf>
[P4]: </Users/ke-qiao/Developer/FYP/R1麦克风阵列模块/新资料（20264.22及之后下单的客户看这里）/6.附件/厂商资料/聆构R1产品白皮书V1.0.pdf>
[P5]: </Users/ke-qiao/Developer/FYP/R1麦克风阵列模块/新资料（20264.22及之后下单的客户看这里）/6.附件/厂商资料/产品规格书-聆构R1.pdf>
[P6]: </Users/ke-qiao/Developer/FYP/R1麦克风阵列模块/新资料（20264.22及之后下单的客户看这里）/3.模块基础使用/3.自定义唤醒词/自定义唤醒词.pdf>
[F1]: </Users/ke-qiao/Developer/FYP/docs/r1-integration.md>
[F2]: </Users/ke-qiao/Developer/project-alia/docs/operations/software-release.md>
[F3]: </Users/ke-qiao/Developer/project-alia/docs/operations/hardware-deployment-readiness.md>
[F4]: </Users/ke-qiao/Developer/project-alia/run/r1-manual-capture-20261024/live/summary.json>
[F5]: </Users/ke-qiao/Developer/project-alia/run/r1-manual-capture-20261024/live/cold-03-human-review.json>
[F6]: </Users/ke-qiao/Developer/project-alia/run/r1-manual-capture-20261024/live/cold-03-independent-audit.json>
[F7]: </Users/ke-qiao/Developer/project-alia/run/r1-manual-capture-20261024/live/cold-03/analysis/audit.json>
[F8]: </Users/ke-qiao/Developer/project-alia/run/r1-manual-capture-20261024/live/cold-03-postflight.json>
[F9]: </Users/ke-qiao/Developer/project-alia/run/r1-manual-capture-20261024/verification.json>
[A1]: </Users/ke-qiao/Developer/project-alia/run/r1-manual-capture-20261024/live/cold-03/analysis/full.wav>
[A2]: </Users/ke-qiao/Developer/project-alia/run/r1-manual-capture-20261024/live/cold-03/analysis/speech-before.wav>
[A3]: </Users/ke-qiao/Developer/project-alia/run/r1-manual-capture-20261024/live/cold-03/analysis/speech-after.wav>
[A4]: </Users/ke-qiao/Developer/project-alia/run/r1-manual-capture-20261024/live/cold-03/capture.s16le>
[A5]: </Users/ke-qiao/Developer/project-alia/run/r1-manual-capture-20261024/live/cold-03/events.jsonl>
[A6]: </Users/ke-qiao/Developer/project-alia/run/r1-manual-capture-20261024/live/cold-03/report.json>
[S1]: </Users/ke-qiao/Developer/FYP/R1麦克风阵列模块/新资料（20264.22及之后下单的客户看这里）/源码汇总/Python/serial_reader.py:371>
[S2]: </Users/ke-qiao/Developer/FYP/R1麦克风阵列模块/新资料（20264.22及之后下单的客户看这里）/源码汇总/ROS2/xf_mic/src/mic_serial_services.cpp:88>
[S3]: </Users/ke-qiao/Developer/FYP/R1麦克风阵列模块/新资料（20264.22及之后下单的客户看这里）/源码汇总/ROS2/xf_mic/scripts/alsa_adapter.py:90>
[C1]: </Users/ke-qiao/Developer/project-alia/packages/alia-embedded/src/module/registry.rs:48>
[C2]: </Users/ke-qiao/Developer/project-alia/packages/alia-embedded/src/module/speech/duplex.rs:145>
[C3]: </Users/ke-qiao/Developer/project-alia/packages/alia-embedded/src/module/traits.rs:20>
[C4]: </Users/ke-qiao/Developer/project-alia/packages/alia-embedded/src/module/noop.rs:95>
[C5]: </Users/ke-qiao/Developer/project-alia/packages/alia-embedded/src/module/r1/protocol.rs:4>
[C6]: </Users/ke-qiao/Developer/project-alia/packages/alia-embedded/src/module/cpal_audio.rs:874>
[C7]: </Users/ke-qiao/Developer/project-alia/packages/alia-embedded/src/module/speech.rs:71>
[C8]: </Users/ke-qiao/Developer/project-alia/packages/alia-embedded/src/module/r1/mod.rs:315>
[C9]: </Users/ke-qiao/Developer/project-alia/crates/proto/proto/voice_session.proto:64>
[C10]: </Users/ke-qiao/Developer/project-alia/packages/alia-server/src/api/voice.rs:356>
[I1]: </Users/ke-qiao/Developer/project-alia/run/r1-firmware-20261021/sources/image-contents/selected/etc/init.d/resource/vtn/res/vtn/vtn.ini>
[I2]: </Users/ke-qiao/Developer/project-alia/run/r1-firmware-20261021/sources/image-contents/selected/etc/init.d/resource/vtn/res/cae_circle6mic_std/cae_init_param.ini>
[I3]: </Users/ke-qiao/Developer/project-alia/run/r1-firmware-20261021/sources/report.md>
[V1]: </Users/ke-qiao/Developer/project-alia/docs/operations/r1-firmware-vendor-inputs.md>
[M1]: </Users/ke-qiao/Developer/project-alia/run/r1-firmware-20261021/contract/pdf-sources.json>
