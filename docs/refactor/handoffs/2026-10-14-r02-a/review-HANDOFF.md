# R02-A 独立审查与回归设计 HANDOFF

日期：2026-10-14。分工：`settings_auth_review`。Root 本轮基线 `f372ad3b4a7fc87de2526840fcada461080c20dc`，承接 R03-0 的软件交付；本分工只修改本文，不修改生产及测试、不执行 Cargo、Docker、数据库、SSH、硬件 I/O 或 Git 提交。所有运行结果以 Root 保存的实际日志为依据。

## 审查 checklist

- [x] 读取最新 R03-0 恢复检查点、audio clock/reference 设计及硬件 audio HANDOFF。
- [x] 核对旧 AudioDebugRecorder 的生产入口、资源所有权与证据缺口。
- [x] 核对 CPAL / ALSA capture、post-mix reference、时钟及有界队列的现有事实。
- [x] 向两位实现 owner 交付 single consumer、丢失、精度、切块及关闭边界。
- [x] 独立读取 Root 四项实际 RED 的日志和元数据。
- [x] 审查 native 双流 / reader ownership 的最终实现及回归。
- [x] 审查显式会话 / 导出 / 离线重放的最终实现及回归。
- [x] 审查 Root 旧配置与全部调用方清理，以及文档与默认行为的一致性。
- [x] 核对最终冻结源码和实际测试结果，明确 Linux / 硬件未验证边界。

当前 **9/9**，是独立审查分工进度，不代表 R02 声学、硬件或部署验收通过。两分工及 Root 集成最终静态审查无阻断；Root 冻结后的 macOS 实际运行、离线导出/重放及拒绝覆盖证据已独立核对。本分工已停写，最终归档及提交由 Root 收口。

## 范围及硬件事实

R02-A 是有界软件证据与合成 PCM 离线重放。继续保留既有输出身份、精确取消、observer 与 native owner 边界，不接入 AEC，不新增依赖、平台或后台自动录音。麦阵 R1 当前尚未接入，未来经 USB 连接 Pi；扬声器及功放未选。硬件支持 AEC、USB 枚举与软件参考流均不证明其 AEC 已启用或参考接线已完成。本轮不访问设备、不录制真实音频，也不通过合成测试推导声学效果。

最新 R03-0 已完成软件 stop/start/PCA 清理，硬件未启用。Root 当前只读环境记录为 macOS ARM64，OrbStack 为 Stopped。历史 Linux 通过不是本轮修改后的 Linux 验证。

## 原实现实际缺口

`module/audio_debug.rs` 的 capture / wake 方法没有生产调用，唯一 `direct_response_playback` local-cue 调用把源 PCM 写成 WAV，发生在 native 混音、音量和后端接受之前。它不能证明实际提交的播放数据。

旧 recorder 每次复制完整样本并启动一个没有保留 JoinHandle 的 `spawn_blocking`，任务数量和会话总字节没有上限；capture 累积只依赖 sample rate 和 segment 长度，没有 boot / stream / continuity / loss，关闭也不 flush 剩余数据。文件名使用局部 sample cursor 或 UTC 微秒，写入可能覆盖同名文件。这不是可扩展成双流证据的可靠基础。Root 清除旧 recorder、配置、死接口及唯一源 PCM 调用，以显式同步有界会话替代，属于本轮必要清理。

现有 CPAL / ALSA capture 向 speech `AudioBus` 发布的是转换和 gain 后的 PCM，保留 `CaptureTiming`；原 native 时钟只 observe 整批输入，没有把该 native PCM 放入 reference 队列。post-mix 播放 reference 则已保存最终 native buffer / ALSA 成功接受前缀。R02-A 应复用该固定 64 × 1024 f64 样本队列，在转换/gain 之前新增 native capture 证据，speech 的 PCM 与行为保持原契约。

原 `AudioReferenceBus` 可以任意 clone 并调用 `try_read`，多个消费者可偷走彼此的块；“只有一个消费者”只是约定。需要非 Clone 的 reader 所有权，claim 失败不得消耗数据，drop 只释放观察权，不停止、取消或重新打开 native stream。

## 时间、丢失与关闭审查标准

- capture 与 playback 是独立 stream / clock；相同 rate、声卡或 boot 不代表采样同步。`observed_boot_ns` 是主机观察时刻，sample cursor 是各流计数，`None` / Unknown 保持未知，不能生成声学偏移、漂移修正或虚构时间。
- capture 的原 `CaptureTiming.native` 描述贡献整批输入；切分证据块只能偏移块 cursor 与可用 sample timestamp，不能再次 observe 导致 cursor 翻倍，也不能宣称 resampling 一一映射。
- capture 和 playback 都包含 zero-length lifecycle marker。marker 的旧 content 值即便为 ZeroFill，也不代表存在零样本；真实 Media 零值、ZeroFill、HeldSilence、Mixed 与 MixedHeld 均要保留。
- 队列争用、满队列、序号缺口、终止 marker 丢失及会话容量封顶是缺失证据，不能补零。unknown xrun 帧数也不能擅自添加到已观察 cursor。
- stream reopen 产生新身份。方向级 snapshot 只保存最新流；若旧流最终 snapshot 已被替换，不能从新流 closed 推断旧流完整关闭。会话提前结束时，仍 running / closing / 未观察到 final 的流必须明确保留其状态。
- normalized f64 保存自身精度及源 sample format，不保证原生 i64/u64 低位无损；非有限样本必须明确处理，不能经 JSON 变成 null 后当作正常 PCM。native capture 还可能已由设备固件处理，不等于 raw ADC。
- 会话同步 drain / finish / export 不创建 detached worker，drop 只释放 reader。native stream 的 stop / join 仍由现有 ModuleCleanup 管理；本轮无需改变其逆序 shutdown supervisor。

本轮导出只包含 native 双流，不另收转换后 capture，也不声称把转换后 PCM 精确配准到 native cursor。原 speech 广播及 CaptureTiming 原样保留。固定队列会在音频运行时观察/cache native PCM；“默认不启用”指没有自动证据会话、文件导出或持久录音，不是默认完全不存在内存中的 native tap。

额外发现的生产并发边界已与两位 owner / Root 对齐：原 sequence 在取得 queue try_lock 之前分配，两个 producer 可先取得序号 1 / 2，再以 2 / 1 的顺序入队。成功记录序号须在持有队列锁时分配；失败尝试仍计入 attempt / loss，不能让 callback 等待。即使如此，PCM timing 在 publish 前已形成，error marker 仍可能先入队，再看到较旧 continuity 的真实 PCM。严格 replay 应保留并 reset 该交错，不以 metadata 必须全局单调为前提。active snapshot 的独立 atomic 字段也不是一致快照；只有 closed 发布后的统计才可作最终约束。

Root 的配置承诺限定现有 `load_config` 和 settings JSON，它们都经过已有 strict `EmbeddedRuntimeSettings`。公有 `EmbeddedConfig` 的直接 serde 路径仍可忽略未知字段，仓内无此读取入口，也已没有任何旧 recorder 可重新启用；本轮不扩展全部配置类型的 strict policy。文档须保持这个准确范围。

## 实际 RED

已读取 `run/validation-20261014-r02-a/red-config.*`、`red-native-capture.*`、`red-single-consumer.*`。三次运行均退出 101、`source_drift=[]`，manifest SHA-256 均为 `e520b96fb716df80ba7c5d5c4c2e9a7975b987495785d95c7bab2d55a96a4a78`（657 输入）。均为预期断言失败，没有用编译失败充当 RED。

| 见证 | 实际结果 |
| --- | --- |
| 旧 `[audio_debug]` TOML | 旧配置被接受，预期拒绝；相关用例失败。 |
| 旧 `audio_debug` JSON 更新 | 更新返回成功，预期拒绝并保持内存/磁盘；相关用例失败。 |
| native capture | 真实生产 helper observe 3 帧，证据 PCM 块为 0，预期 1。 |
| single consumer | 第二个 bus clone 实际偷走已经建立消费者的 PCM，触发断言。 |

耗时分别为 config 20.20 秒、native capture 7.41 秒、single consumer 0.14 秒（包含各自编译时间）。所有输入均为合成或配置 fixture，没有真实录音。

## 最少 GREEN 回归及复用边界

建议沿用现有 CPAL post-mix、ALSA 接受前缀、lost-close、contended queue、clock regression、native owner 和 mixer 精确取消测试，不重新建立假播放后端。新增必要覆盖如下：

1. native 非零 capture 经过实际 helper，源格式/边界正确；超过固定块大小的 stereo 输入只 observe 一次，块 frame cursor、sample timestamp 与整批返回值一致。
2. reader claim 排他、失败 claim 不吞数据，drop 后可重新 claim；reader/session drop 不影响 native running 或已有 cancel identity。
3. 合成双流导出与离线重放精确保留每块的 normalized f64 bits、源格式、边界、时间、content、身份和 loss。重启/continuity/同 rate 异 clock 不合并；无法证明的缺失不补零。
4. 空块 marker、闭流末尾丢失、运行中 finish、旧 snapshot 替换、容量/读取/导出限制均不能产出虚假的完整结果。容量达到后的处理有界且可审查。
5. 格式、帧/样本长度、cursor 溢出、非法 identity / metadata、非有限样本、文件截断、超限与失败导出有明确错误和资源清理；离线载入本身也有输入大小上限。
6. 旧配置在 TOML 与 JSON 路径均明确拒绝；拒绝不改变现有 settings 或文件。默认 node 构造不 claim、导出或创建自动录音任务。

本轮软件验证不能替代真实 R1 USB / ALSA 采集、AEC 参考接线、扬声器/功放、电气条件、声学延迟与漂移测量。导出操作数量和内存有界也不构成 OS 同步文件 I/O 墙钟时间上界。

## Native 双流候选静态审查

已完整读取 agent1 停写后的六个所属文件：`context.rs`、`context/audio_reference.rs`、`cpal_audio/clock.rs`、`cpal_backend.rs`、`alsa_backend.rs`、`playback/tests.rs`。未发现阻断项；下列是源码与测试设计结论，尚未当作测试运行通过。

- CPAL 输入及 ALSA I16/I32 成功读取前缀都在转换/gain 之前调用同一个 capture helper；S32 对齐检测及转 I16 发生在 native 证据复制之后。speech 广播保持原有整批 CaptureTiming 和转换结果。
- helper 一次 observe 整批输入，按通道对齐切成至多 1024 样本的固定块；每块只修改 frame 范围与可用 sample timestamp，不重复推进时钟。2052 个 stereo 样本回归重建全部原输入，并核对 1026 总帧和三块偏移、callback/boot observation 不变。
- reader 是非 Clone 的独占 owner，失败 claim 不读取队列，drop 可重新 claim 且原 stream 保持 running / 既有 PCM 仍在。四方 barrier 用例让胜者持有 reader，直到所有 claim 尝试完成，避免把顺序释放误认为同时多 owner。
- 发布成功的 sequence 在 queue 锁内分配，失败尝试仍记 attempt/loss，producer 仍只 try_lock。并发用例核对最终 attempt、retained、lost 的守恒以及成功序号递增；该测试重用一个 PCM block，只验证队列并发不变量，不作为真实连续音频 replay 的证据。
- 非有限原生 float 在 producer 保持实际值，后续证据会话须显式拒绝，不能生产层改写为零。i64/u64 normalized 精度限制已写入源码契约。
- post-mix / ALSA accepted-prefix、late cancellation、lost-close、contended queue 等旧测试仅迁移为 reader，不改变真实提交或取消逻辑。Linux 专属 ALSA runtime 测试需要当前 Linux 环境执行才可声称通过。

Root 另指出：若锁内先分配 sequence 再读 loss，抢不到锁的其他 producer 仍可让 loss 领先该成功块序号。最终补丁已经先读取两个 loss atomic，再分配 sequence，最后入队，保证成功记录的 `lost_blocks_before < sequence`；并发回归新增逐块断言。两个 loss 字段仍是独立观察，只可能落后，不宣称彼此具有精确同一时点关系。

## 显式会话与导出候选静态审查

已完整读取 `audio_evidence.rs`、13 项测试及 `examples/audio_evidence_replay.rs`。会话上限为 512 records、524288 normalized samples、16 MiB JSON；调用方可降低上限，不能放大硬上限。pump 的本次读取次数也有独立上限，空队列不表示 stream 停止。

会话仅持 reader 和独立观察取消 token，没有 native 控制、关设备或取消播放的能力。finish 消费自己并读取两个方向最新 snapshot，Drop 释放 reader；session 满额/取消/invalid 时仍保留终止理由和既有合法 records，已取出但未保留的一个块另计 discard，不假装它已成功导出。没有后台 task 或 recorder worker。

所有 record 保留 f64 bits、原容器、边界、boot/stream、时钟、sequence、loss 与 content。验证拒绝混 boot / nil / clock mismatch / 不匹配的 frames 与样本、溢出、NaN/Inf/out-of-range、同 stream 容器或后端改变，以及不符合当前 CPAL/ALSA 的组合。capture 只能 Media；ZeroFill/HeldSilence 实际样本必须全零，marker 必须无 PCM、无边界/容器且 Unknown 时间。BackendEstimate 至少有 callback 观察；Unknown 仍允许真实的部分时间证据，不因缺一项而补造或抹除另一项。

关闭 snapshot 严格核对最终计数、cursor、continuity 不早于同一 stream 已观察记录；active snapshot 仅作为独立观察，不要求原子一致。replay 每方向独立判断 stream/continuity/sequence/frame/loss reset，仅产出已有 records，不插入任何补零，也不播放。文档要求 replay 后继续检查 footer：只在 footer 可见的 tail loss / lost close 不能漏掉，reopen 替换掉的旧 stream 最终状态仍未知。

导入先检查总字节 cap，再通过 capped Visitor 限制 records / 每块 samples 的 Vec，避免任意长数组先完成分配后才拒绝。上限检查会额外读取一个有界元素以发现超限，未保留第 513 个 record / 第 1025 个 sample。严格 nested unknown 字段检查局限于本地证据格式，不改变 core 其他 wire 类型。导出先构造有界 JSON，再按 8192-byte chunk 写入 caller-owned sink；取消或写入失败明确返回错误，已写前缀由调用方负责，不能宣称文件原子性或系统调用时间上界。

13 项测试设计包含 bits 完整往返、reader/session cancel/drop、不关闭 native owner、records/samples/bytes 三种限额、非有限值拒绝、满队列末尾丢失、reopen 未知终态、marker/PCM 交错和 gap reset、严格格式与身份、同流容器和伪静音拒绝、active/closed 差异、解析期数组上限、truncated JSON、预先取消、写入失败。partial sink 用例真实保留前 17 bytes 后失败；mid-export cancel 用例在第一块 8192 bytes 写完后取消，下一块不写。

离线 example 使用明确 `SyntheticFixture`，先把有界证据写到内存，再临时文件写入/sync，最后 `persist_noclobber` 发布指定新路径。它不打开设备、不覆盖已有目的文件。重新读入后迭代 replay，并同时报告 footer 的已知 lost tail、latest capture running、旧 capture final unknown 与 acoustic alignment unproven。它验证人工构造的格式/重放，不代替生产 native capture helper 的回归。

## Root 集成清理静态审查

已经读取 Root 的 config/types、路径处理、builder、SpeechPipeline、duplex、local cue、对应 fixtures 与 example/config 文档。它们只删除旧 debug 参数、字段及唯一路径；原 lease/events、Ready scope、cue 精确取消与 join guard 保持。`module/mod.rs` 从 audio_debug 替换为 audio_evidence，生产搜索没有遗留 AudioDebugRecorder / AudioDebugConfig / audio_debug 调用。

旧表的 TOML load 与 JSON settings 更新拒绝覆盖 enabled=true/false，JSON 被拒后核对内存和磁盘不变；默认序列化无旧字段、shared clock selector 默认 false。新会话没有加入 builder/startup，因此默认没有 claim、自动导出或 detached 录音任务。Root 文档承诺限定现有配置入口，与实现一致。

## 最终运行与同源证据

两次独立重新计算 `green-source-manifest.json` 及其中全部 659 个输入，SHA-256 均为 `676d2e76fccd23ac78003a6d6cbc8258a1c1faeba18e8c8cf1d02da8b246bda3`，当前无漂移。以下证据位于 `run/validation-20261014-r02-a/`，Root 负责归档到本轮 evidence 目录；没有以编译成功代替实际测试。

| 实际检查 | 结果 | 独立核对与边界 |
| --- | --- | --- |
| 首次 `green-embedded` | exit 101，57.91 秒；lib 176 passed / 42 failed / 0 ignored | 42 条错误全部是 `127.0.0.1:0` bind 的 EPERM，仅来自 body/playback/speech 三个既有 fixture 入口；audio evidence 13 项已通过。保留为沙箱环境失败，不能算整体 GREEN，也不是新增业务断言失败。 |
| 允许本地 loopback 后，同源 `green-embedded-loopback` | exit 0，58.43 秒；默认 features `cargo test -p alia-embedded --all-targets --offline --locked` | 独立从各 suite 结果求和：lib 218 + integration 66 = **284 passed / 0 failed / 0 ignored**。新 evidence 13、native clock 5、reference/reader 10 逐项 ok；原 mixer、精确取消、native owner、speech/gRPC、配置与资源回归同轮通过。 |
| Embedded scoped fmt | exit 0，0.20 秒 | 所属 package 格式通过，无源漂移。 |
| workspace fmt | exit 1，0.82 秒 | 仅未修改的 Server `tests/server_runtime/delivery.rs` import 顺序。`preexisting-format.json` 记录与本轮基线完全同字节，SHA-256 `f1c3bbabf0a575161e8a1cadd043d6cb94fb0b57b19fe788cd3a52a6e78d6709`，不顺带修改无关文件。 |
| `green-synthetic-replay` | exit 0，1.90 秒 | 明确 SyntheticFixture；导出后重新载入并 replay，实际 7 records / 16 samples / 9 reset signals；footer 报告 playback closed 且 tail loss 2 blocks / 2 frames，latest capture running、旧 capture final unknown，physical completeness=false。 |
| 同一路径 `green-synthetic-no-clobber` 再执行 | 预期 exit 1，0.64 秒，File exists | `synthetic-no-clobber-verification.json` 证明原文件字节未变、无新增隐藏临时文件；独立核对文件 5137 bytes、SHA-256 `374d84a56124ab88aecf40aaaa8225c2df6f813b3c860e9061efa953a38d1b49`。 |

上述实际运行均使用同一冻结源码且 `source_drift=[]`；第一轮沙箱失败、最终成功和拒绝覆盖的预期失败分别保留，不混合计数。本次构建配置为 macOS 默认 features，没有声称 no-default、Linux 或 Pi 实跑。当前 OrbStack 为 Stopped，未启动容器/未知机器，没有真实录音、打开音频设备、连接 Pi、操作执行器、部署或外部模型调用。测试只用合成 PCM、临时 SQLite / 文件与本地 loopback；本分工未自行执行 Cargo。

最终未发现当前软件范围的阻断问题。还需后续硬件条件确认及实际采播/AEC参考接线/声学测量，才能推进 R02 的硬件验收；本轮结果不能提升为 AEC 已工作、全双工声学合格或真实用户已经听到输出。
