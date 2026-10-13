# R02-A — 有界会话、导出与离线重放 HANDOFF

日期：2026-10-14。Owner：`settings_auth_reproduce`。本文件只覆盖软件证据链；native 双流与独占消费见 [native-HANDOFF.md](native-HANDOFF.md)。Root 统一执行 Cargo、冻结证据与提交；本 agent 未执行硬件、Cargo、Docker 或 Git 操作。

## 当前 checklist 与进度

- [x] 01 检查恢复检查点、硬件确认、原 recorder 与所有调用路径。
- [x] 02 与 Root/native owner/reviewer 明确 native 双流、独占 reader、字段与 RED 边界。
- [x] 03 删除旧 `audio_debug.rs`，新增显式同步 `audio_evidence` 模块；无新依赖、配置或自动后台任务。
- [x] 04 完成有限会话、v1 严格文件契约、原样保留 f64 bits、导出/读取与独立流重放。
- [x] 05 完成 13 项离线回归和合成 fixture 示例；scoped rustfmt 完成，生产/测试源码停写。
- [x] 06 Root 冻结后实际运行 GREEN 与示例，已独立读取日志和统计。
- [x] 07 独立 reviewer 最终静态与 GREEN 读回无阻断。
- [x] Root 已提交源码 `e557ceb`；总 checklist、恢复入口及证据随交接记录提交。

本 owner 进度为 **8/8**；Root 已完成源码提交，最终交接记录另行提交。R02 声学/实机验收未完成，整体 R01—R06 仍为 2/6。

## 检查出的旧边界

旧 `AudioDebugRecorder` 唯一生产调用是 local cue 的源 PCM WAV，不是混音后的 native reference；capture/wake 记录方法没有生产调用。旧 recorder 以分离的 `spawn_blocking` 写文件，未保留任务所有权，也没有 boot/stream/continuity/loss/clock 导出或离线 replay。

Root 已运行并归档 4 个真实 RED（旧配置两项、native capture PCM 缺失、clone 消费偷读），见 `run/validation-20261014-r02-a/red-*.log/json` 和 `red-source.patch`。新的导出/replay 原先不存在，本 owner 没有把缺符号编译错误伪装成 RED，也没有给将删除的 WAV 路径添加无用测试。Root 负责旧 `AudioDebugConfig`、builder/speech/direct-response 参数和旧文档迁移；本 owner 只删除 recorder 文件并在 `module/mod.rs` 替换模块声明。

## 交付契约

源码位于 `packages/alia-embedded/src/module/audio_evidence.rs`，测试位于相邻 `audio_evidence/tests.rs`。

- `EvidenceSession::start` 从 bus 领取唯一 reader，固定声明来源 `ObservedNative`；`pump(max_records)` 同步、有界读取；`finish(self)` 返回已观察记录与最新方向快照。观察取消、finish/drop 都不持有 native close 或 playback cancel handle，因此只释放消费资格，不关闭采播。
- 诊断硬上限为 512 条记录、每条 1024 个 normalized f64 样本、总计 524288 个样本、16 MiB JSON。调用者 `EvidenceLimits` 只能收紧，JSON 最小预算 8192 bytes；会话为 header/footer 保守预留 8192 bytes。它们是软件内存边界，不是音频质量或声学验收阈值。
- 达到上限返回 `LimitReached`；实际从队列取出但无法保留的记录/样本单独计数。尚未取出的块数以及后续生产量未知，不伪造精确丢失数量。非法 native 样本返回错误并保留此前有效记录，以 `InvalidRecord` 终止观察。取消状态为 `Cancelled`。
- v1 header 带 boot ID 与 `ObservedNative`/`SyntheticFixture` 来源声明；同一文件拒绝 nil ID、混合 boot、同 UUID 跨方向/格式复用。来源是可检查的一致性声明，不是硬件认证、签名或权限证明。
- 记录保存 `AudioTiming`、本地 native boundary、native sample format、content、marker、sequence 和已观察 producer loss。采集是转换/增益前 native input，播放是最终 native output 边界；不另存无法精确配准的转换后 i16。
- `samples_f64_bits` 保存 normalized f64 的 IEEE bits，保留负零和微小有限值。不是量化回 i16，也不声称 i64/u64 转 f64 前的低位可恢复。读取拒绝 NaN/Inf、超出 [-1,1]、不完整 frame、长度/格式/边界不匹配。
- JSON 在 parse 前先限制输入 bytes，反序列化数组时限制保留记录/样本数，再做身份、算术、实际 producer 组合与序列校验。v1 自身和嵌套 metadata 的未知字段都会被拒绝，未修改 shared core serde 契约。真正的 ZeroFill/HeldSilence 必须全零；marker 为零帧、无 PCM、无 boundary/sample format、Unknown clock。
- 成功入队的 sequence 必须递增；同一 native stream 的数据格式/表示不变，PCM cursor 不重叠。并发 error marker 可以先于已经观察到的旧 continuity PCM 入队，因此不把 marker cursor/continuity 当作全局顺序门槛。保留原 timing，在 replay 中显式发出 reset/gap 信号。
- `ObservedLatest` 只保留每个方向最新流。活动快照的 atomics 不是同一时刻的事务；不强加 active counter/cursor 一致性。已 `closed` 的快照才校验最终 counter、cursor 和 continuity 不早于已观察记录。reopen 后旧流缺少 close marker 和自身快照时，最终状态保持未知。
- replay 只遍历实际记录，按方向提供 FirstObservation/StreamChanged/Discontinuity/SequenceGap/FrameGap/ProducerLoss；不补零、不播放、不调用模型、不推导共享采样时钟。调用者必须另外读取 footer，因为尾部溢出/丢失 close marker 可能只在那里可见。`unobserved_history_possible` 固定为 true，`Finished` 只表示观察结束。
- export/read 是同步、调用者所有的 I/O。token 在有界操作之间检查；不能中断调用者实现内部的阻塞 Read/Write。失败或取消后部分 sink 属于调用者，不宣称文件已完整发布；模块没有 detach task 或默认路径。

## 离线测试与示例

最窄测试 filter：`module::audio_evidence::tests`。13 项覆盖：双流 bit-exact 往返与独立 clock、取消/finish/drop 释放 claim 且 native 继续、三类 budget、非法 native sample、队列溢出与 lost-close footer、reopen 旧流最终未知、marker/PCM 并发交错、schema/identity/format/sample/sequence 拒绝矩阵、native 表示不可中途变更与真正静音、active 非原子快照边界、parse 数组上限、输入截断/字节上限/取消/错误、写入前缀后失败与中途取消。

运行示例（Root 统一执行，输出路径必须尚不存在）：

```sh
cargo run -p alia-embedded --example audio_evidence_replay --offline --locked -- run/validation-20261014-r02-a/synthetic-evidence.json
```

`examples/audio_evidence_replay.rs` 创建明确标为 SyntheticFixture 的双流数据，先完成验证和有界 serialize，再写临时文件、sync、`persist_noclobber` 发布；不会覆盖已有文件。随后真实读取文件并 replay。summary 明确输出 tail loss、latest playback closed、latest capture running、被替换 capture 最终未知以及物理对齐未证明。fixture 和 summary 不代表 native 硬件采集的实证。

## 已读取的验证结果

GREEN 冻结 659 个输入，manifest SHA-256 为 `676d2e76fccd23ac78003a6d6cbc8258a1c1faeba18e8c8cf1d02da8b246bda3`，下列每次结果均 `source_drift=[]`。

- `green-embedded-loopback.log/json`：`cargo test -p alia-embedded --all-targets --offline --locked`，默认 feature 与默认并行；退出 0，58.43 秒，lib 218 + integration 66 = **284 passed / 0 failed / 0 ignored**。本模块 13 项全部通过。
- 首次 sandbox 运行的 `green-embedded.log/json` 保留实际结果：lib 176 passed / 42 failed，失败为本地 loopback socket 的 `Operation not permitted`。Root 在允许 loopback 的执行环境按相同输入重跑后全过；没有改测试、串行化或隐藏首次失败。
- `green-synthetic-replay.log/json`：示例退出 0，1.90 秒；实际生成、读取并 replay `synthetic-evidence.json`。输出 7 records、16 observed samples、9 reset signals；latest playback closed 且 lost blocks/frames 各 2，latest capture running，被替换 capture 最终状态 unknown，physical completeness false。
- `green-synthetic-no-clobber.log/json`：对已存在路径再次执行相同示例，按预期退出 1，0.64 秒，错误为 File exists；`synthetic-no-clobber-verification.json` 确认原文件 bytes 未变、SHA-256 仍为 `374d84a56124ab88aecf40aaaa8225c2df6f813b3c860e9061efa953a38d1b49`，没有新增残留临时文件。
- `green-format-embedded.json`：Embedded package format check 退出 0。全 workspace format 的未改基线差异由 Root 另行记录，未为此触碰 Server。
- 当前运行环境为 macOS 26.6.2 / Darwin ARM64 / Rust 1.95；`environment.json` 中 OrbStack 为 Stopped，本轮未跑 Linux/ALSA 当前验证。历史 Linux 通过不能替代本轮结果。

本轮未录放音、连接 R1、修改系统音频权限、试验 AEC、电气参考路由或启用新平台。R1 firmware/接口电气、功放/扬声器、真实全双工双讲、AEC 抑制与物理取消尾音仍在后续实机轮次。

Root 收口：源码 `e557ceb962610004287f38e66b50abca3e44b661`，659 个 Git blob 与 GREEN 清单全部一致；最后归档项已关闭，见 [Root HANDOFF](root-HANDOFF.md)。
