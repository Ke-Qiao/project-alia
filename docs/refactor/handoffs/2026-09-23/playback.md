# Playback / exact retirement handoff — 2026-09-23

本子包已完成配对组件验证，等待 Root 冻结并验收完整 workspace。工作树为 `run/worktrees/alia-duplex-integration`，没有自行 add / commit / push。可读清单是 `docs/refactor/checklists/duplex-playback.md`；机器结果、精确文件摘要见本目录 `RESULT.json` 与 `source-manifest.json`。Root 后续修改 Server handshake 时，以 Root 最终 manifest 为准。

## 恢复与真实红测

先核对上一日交接六文件 SHA256，全部一致；等 Root 保存 `ae75a4e` 源码快照后才修改、执行。原五项编译及 no-run 已成功，实际首跑是 **1 pass / 4 fail**。丢 ACK 正文成功但夹具原本吞掉的 `DeliveryService.shutdown()` 错误被新严格断言捕获；原三个媒体用例卡在 native 准备。第六项 replacement 首次也停在 native 准备，未将其错误写成 replacement 已复现。

诊断确认两个夹具问题和一个生产 bug：

- 原夹具忽略 shutdown Err，只检查 pending=false。现在先收齐运行时 / listener，再严格检查 Result。
- 首块前 Hold 不应等待 Delivery row：生产 wait_open 在 reservation 前。现在等 5.5 秒后断言原 budget 保留、尚无 row / Push，再恢复。
- `AtomicU64::fetch_update` 成功返回旧值，Server 第一个 playback sequence 为 0，导致 `invalid playback reservation`。Root 修成 checked_add 后的值，第一个序号为 1。

原始红测与 early-result 日志均保留，没有通过放松断言抹除。

## 最终实现

Root 扩展授权本 agent 维护 Core/Proto/Embedded 退休协议。新 `RetireOutput` 以 exact scope、可选 canonical binding、可选 expected backend 绑定原 owner，与原 minimum-generation admission Fence 分开。回执为显式 `NoNativeOwner` 或带原 backend / progress 的 `Settled`；类型与 codec 均严格校验原身份、版本、软件 quiescence 和证据边界。

只有本地已知 scope 且无 registration、native generation、control、submission 才能返回 NoNativeOwner，并永久关闭原 epoch。登记过的 owner 必须带原 binding；ACK 未知可以不带 expected backend，但不能重发 Register。unknown / evicted 是错误，不能当成零输出。

`PlaybackSession` 保留原 registration / control / submissions Arc 的退休记录；容量 64，live scope 预留一槽，只可驱逐已 settled 记录，全是原在途写入时拒绝新 scope。整条 output 等待无 1500ms 截断，锁外等待已入场写入；RPC 丢失时原记录仍持有 owner。设备 shutdown 同样等待这些原写入。

当前 exact scope 的首次退休同步 fence 原 body admission，并仍执行 audio 退休；不改变 active.scope、不创建新的 native generation。旧 / 已退休 scope 仅查询原记录，不触新 body/native owner。所有入口的 native/body 失败保留，重复 close 不抹除错误；运行时错误列表统一最多 64 条、日志仍记录后续失败，单 record 只存首次错误。

旧精确 Playback cancel/observe 沿同一退休记录取得事实，不再错误要求旧 scope 仍 active；入口和返回前继续验证 token。`Submitted`、gate quiescence 均不是 `Presented`、扬声器静音或声学证明。

## 实际验证

| 目标 | 实际结果 | 日志 |
| --- | --- | --- |
| 完整配对六项 | 6 pass / 0 fail / 0 ignored，13.42 秒，exit 0 | `duplex-paired-runtime-retirement.log` |
| 最后受影响 late Register case | 1 pass；pending 再登记立即拒绝、原 waiter 保留、late ACK 不复活，0.36 秒 | `duplex-paired-registration-final.log` |
| 最后受影响 replacement case | 1 pass；冲突退休保留原 cache，随后原 Start 正常；旧账本 Retired 后新 native 仍 Running，0.44 秒 | `duplex-paired-replacement-final.log` |
| Embedded playback 相关组 | 72 pass / 0 fail / 0 ignored；最后错误上限收紧之前 | `duplex-retirement-embedded-playback.log` |
| prior native/body failure 专项 | 1 pass；早先失败不因后续成功 close 消失 | `duplex-retirement-prior-failure.log` |
| 最终 Embedded retirement 过滤组 | 8 pass / 0 fail / 0 ignored，4.27 秒 | `duplex-retirement-embedded-final.log` |
| Core / Proto 全集 | 27 / 4 pass，均 0 fail / 0 ignored，exit 0 | `duplex-retirement-core-proto.log` |
| Rust 格式 | 所有 owned Rust 的 rustfmt --check exit 0 | 已在最终交接前实际执行 |

8 项过滤组与前面的结果有交集，两个配对单项也是六项中的复跑，不能相加为新测试数。最后四项新设备 gRPC 回归覆盖：原写入持有超过两秒仍不成功；换 session 后不动新 owner且旧精确 cancellation 保留 prefix；NoNative 先于 Register、unknown/conflict/eviction 不成功；64 个未收尾 owner 不被驱逐；返回前 token 过期拒绝。

## 独立 Server review

已检查新 Retire 映射、late Register cache token、Push 时钟与序号。Root 已采纳并修复本地 cache mutation 前的 binding/backend 冲突校验，以及 pending Register 不再重发。已在现有两个 paired case 补断言并绿测。

另发现首 Push ACK 的全局 receipt_sequence 不一定为 1：独立 Control/Observe 可以先消耗同一 submission 的计数。已向 Root 提交确切代码路径，Root 负责现有真实 TCP EvidenceDevice 的 ACK=2 确定性红绿回归；本 agent 不修改该 Server 文件。最终 workspace 冻结应包含它。

## 文件归属

本 agent 最后实际改动：

- `crates/alia-core/src/output_control.rs`
- `crates/proto/proto/playback.proto`
- `crates/proto/src/output_control.rs`
- `packages/alia-embedded/src/module/playback_session.rs`
- `packages/alia-embedded/src/module/playback_session/{capture,control,tests}.rs`
- **新文件** `packages/alia-embedded/src/module/playback_session/retirement.rs`
- `packages/alia-embedded/src/grpc/playback_service.rs`
- `packages/alia-embedded/src/grpc/playback_service/tests.rs`
- **新文件** `packages/alia-embedded/src/grpc/playback_service/tests/retirement.rs`
- `packages/alia-server/tests/duplex_playback.rs`
- `docs/refactor/checklists/duplex-playback.md`

四个公开 backend 方法、Server devdep 和 Cargo.lock 依赖边已包含在上一日 WIP；本次没有额外外部依赖。Root owns Server adapter/channel/playback/output actor 接线，本 agent仅只读检查并协作请求。

## 资源收尾与限制

最后一次两个配对 case 后已实际查询任务 PG：随机 `delivery_test_*` 为 0，其他 client backend 为 0，仅原三个 `pipeline_delivery_*` baseline。没有手工删 schema、没有创建或停止容器；PG窗口已归还 Root。无活动本 agent cargo / exec / native waiter / listener。临时密钥和私有 SQLite 由 fixture 清理。

本机证据为 macOS arm64 的软件 backend、真实 Postgres 与 loopback 两端 gRPC。无 Pi 物理硬件、扬声器静音、AEC、Presented、听感或长期负载证据，仍由主任务明确分开。
