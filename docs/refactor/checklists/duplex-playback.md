# Duplex paired playback verification

**2026-09-23：6 / 6 真实 PostgreSQL＋两端 gRPC 配对测试通过，0 fail / 0 ignored，exit 0。** 日志为 `run/duplex-paired-runtime-retirement.log`。它覆盖此子包的软件所有权与回执边界，不替代 Root 的最终 workspace 验收或 Mac / Pi 物理硬件验证。2026-09-22 的 0 执行暂停记录保留在 `run/duplex-playback-handoff-20260922/`；旧 C 媒体包的 59 项不计入本子包。

上层使用真实 `DeliveryService` / `OutputProducer`、Context / Response 与 PostgreSQL 随机 schema；Server `EmbeddedChannel` 走真实初始化、签名认证和 Tonic client；Embedded 使用生产 `PlaybackService`、`PlaybackSession`、`PlaybackControl` 和 `AudioSubmission`。唯一 native 替身为有界 PCM 队列，模拟 callback 必须通过原 `try_begin_output` 门再 `finish` 实际提交数量。

- [x] **DP01 可注入 backend 契约**：公开四个已有方法 `PlaybackControl::{new, apply, retire}`、`AudioSubmission::with_control`，要求保留原 native generation 的控制 Arc；无测试旁路或新 feature。
- [x] **DP02 实际链接与运行**：Server dev-dependency 使用已有 workspace 包 `alia-embedded`、`default-features=false`；无新外部依赖。恢复 check / no-run 均 exit 0，随后完成真实运行。
- [x] **DP03 首次 Start 前暂停**：规范 Response 已绑定，ExplicitWait 期间原媒体租约保留 5.5 秒；生产 gate 位于 reservation 前，因此尚无 Delivery row 和 Push。fresh ExplicitContinue 后仅创建一次 Push，同一原媒体 Submitted 4 samples。
- [x] **DP04 满队列控制**：1600 samples native 队列满时 Condvar 精确阻塞；独立 Hold 到达并关闭原软件输出门；恢复后 6400 samples 逐样本同序且仅一个 submission。
- [x] **DP05 End 后尾部**：原 End 已接受而 tail 尚未提交；Hold 5.5 秒仍保留原回执及 PCM，恢复提交同一尾部，不生成 Presented。
- [x] **DP06 控制 ACK 丢失**：真实服务执行后薄代理丢 ACK；原 actor 通过 Observe 恢复同一 command 回执并入账，明确恢复前仍 Held。
- [x] **DP07 注册晚回执**：延迟原 RegisterOutput ACK，Fence 先撤销 scope；晚 ACK 不能回填旧本地 cache，原绑定不能复活；第一 ACK 未知时再次 Register 立即拒绝，原 waiter 仍保留。
- [x] **DP08 运行与资源收尾**：6 项完整运行 exit 0；两个运行时和 listener 收尾后严格检查 Delivery shutdown Result，不吞 cleanup Err。PG 随机 schema 清完，仅原三个 baseline、其他 client backend 0，窗口已归还。
- [x] **DP09 替换后旧 owner 清理**：旧 tail 尚未提交，新 session 已有规范 Output / native tail；精确 RetireOutput 返回原 binding / backend / identity 的 settled 软件事实。冲突退休请求先被拒绝且原 cache 仍可正常 Start；旧 DB control 已 Retired 后，新 scope 仍 Running，原新 PCM Submitted 4 samples。
- [x] **DP10 精确退休协议与保留证据**：专用 `RetireOutput` 与 admission Fence 分开；区分显式 `NoNativeOwner` 与带原 backend/progress 的 `Settled`。4 项新设备 gRPC 回归和 native/body 失败收尾回归通过，Core/Proto 全集通过。

`RetireOutputRequest` 包含 exact scope、可选 canonical binding 与可选 expected backend。已登记 owner 必须匹配 binding；ACK 未知也不重发 Register。只有本地已知且从未创建 native owner 的 scope 才可返回 NoNativeOwner，同时永久关闭该 epoch；unknown / evicted 是错误，不能当成零输出。第一次退休当前 scope 会同时停止原 body admission；已替换 / 已退休 scope 只查询原记录，不改变新 scope。

Embedded 保留原 registration / control / submissions 的 Arc。退休记录上限 64，只可回收已 quiescent 的记录；每个 live epoch 预留一个退休位置，全是未收尾 owner 时拒绝新 scope。整条 output 退休等待不设一个会丢弃原 owner 的 1500ms 截断，原写入收尾前不报成功；精确 playback cancellation 在替换后也查询同一原记录。RPC 入场及返回前重验 token，旧 scope 无须继续 active。native/body 失败保留首次错误，最终 shutdown 仍失败；运行时错误列表有 64 条上限。

| 验证 | 实际结果 | 日志 |
| --- | --- | --- |
| 原 5 项首次真实运行 | 1 pass / 4 fail，exit 101；保留红测 | `run/duplex-paired-runtime-initial.log` |
| 第 6 项首次运行 | 0 pass / 1 fail，停在 native 准备；不是 replacement 本体复现 | `run/duplex-paired-replacement-initial.log` |
| native 早退诊断 | `reserve playback: invalid playback reservation`；Root 修正 AtomicU64 fetch_update 返回旧值造成的首序号 0 | `run/duplex-paired-media-early-result.log` |
| Embedded playback 相关组 | 72 pass / 0 fail / 0 ignored，57.20 秒；发生在最后错误上限收紧前 | `run/duplex-retirement-embedded-playback.log` |
| 原 native/body 错误专项 | 1 pass / 0 fail；成功 close 不抹除既有失败 | `run/duplex-retirement-prior-failure.log` |
| 最终 retirement 过滤组 | 8 pass / 0 fail / 0 ignored，4.27 秒；包含前述 4 项新 gRPC 与错误专项，不重复累加 | `run/duplex-retirement-embedded-final.log` |
| Core＋Proto 全集 | 27＋4 pass / 0 fail / 0 ignored，exit 0 | `run/duplex-retirement-core-proto.log` |
| 两端完整配对 6 项 | 6 pass / 0 fail / 0 ignored，13.42 秒，exit 0 | `run/duplex-paired-runtime-retirement.log` |
| 最后 cache / pending Register 修复的受影响 2 项 | 分别 1 pass / 0 fail，0.44 / 0.36 秒，不新增 unique case | `run/duplex-paired-replacement-final.log`、`run/duplex-paired-registration-final.log` |

恢复时还修正了两个测试夹具问题：原夹具只检查 `shutdown_pending=false`，未检查 shutdown Err；首块前 Hold 原来等待生产 gate 之前不应存在的 Delivery row。native 屏障现与媒体 Future 竞速，提前返回会打印真实 Result，而不只显示超时。失败与修复日志均保留，未放松原 admission validator。

配对 fixture 使用临时签名密钥、私有 SQLite 和 loopback listener，复用已有真实 Context / Response 夹具。状态查询 3 秒上限，单 case body 32 秒，外层 PG fixture 45 秒；两次 5.5 秒专门验证实际期限，不用概率等待制造竞态。正常、panic、body timeout 都先释放队列，再 join 原 owner 与 listener。所有软件 Submitted / quiescence 事实均**不证明声卡已播放、扬声器已静音、Presented、AEC 或声学效果**。

本轮经 Root 扩展的源码归属为 Core output_control、Proto playback/codec、Embedded PlaybackSession / PlaybackService及测试、新 paired test 和本清单；Server adapter / EmbeddedChannel / output actor 接线由 Root 负责。子包 manifest 与最终交接为 `run/duplex-playback-validation-20260923/`。独立 Server 接口复核若导致 Root 新增修复，将以最终冻结源码和 Root 的统一验收为准。

独立 Server 接口复核另外发现首 Push ACK 不一定是全局 receipt sequence 1：独立 Control / Observe 可能先读取同一 submission 并消耗计数。该发现已交 Root，在现有真实 TCP 夹具做确定性首 ACK 2 红绿回归；原正数 / 单调性 / identity / accepted 0 / 未 End / 无 terminal 校验仍保留。它属于 Root 的最终统一源码验收。
