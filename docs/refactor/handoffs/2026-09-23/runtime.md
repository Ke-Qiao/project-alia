# Root 运行时交接 · 2026-09-23

本轮从 `ae75a4e` 暂停快照恢复，在 `codex/duplex-integration` 组合三个 subagent 的修复。源码已正常提交为 `5810687`，V2 完整软件验证已通过；最终合入状态以[检查点](../../resume-checkpoint-2026-09-23.md)和[验收汇总](validation-summary.json)为准。2026-09-22 的四份交接保留原状。

## 实际修复

- **默认线程栈溢出**：`cancellable_stage` 在构造 async 状态之前把同一个原 Future 放入堆，继续持有它及有限结果，不重建 LLM/TTS 请求。提交 `9c4f5b0`。原 poll 链机器码栈帧合计约 2,003,856 bytes，改后约 632,096 bytes；默认栈单项与原 Pipeline 20 项通过。测量二进制与运行二进制各有独立哈希，详见 `run/validation-20260923/stack-analysis/README.md`。
- **Bind 取消前置身份**：本地不可变候选与设备 ACK 后 registration 分离；固定 request/Response/Context/channel/intent 后，在 gate 下入队。ACK 未知、排队取消和语义到期仍能查验并取消规范 Response，不能凭 caller 修改过的 clone 取消别人的行。
- **生成错误原身份**：Pipeline prepare 后立即保留可信 Response ID。尚未收到 Register ACK 时的生成错误仍取消原输出，不在同 scope 另建 source-notice Context/Response。真实 PG 红测原 2 个 Context 变 3 个，修复后保持 2 个；21 项 Pipeline 定向通过。
- **精确输出退休**：新增 `RetireOutput`，请求固定原 scope、可选 binding/backend；回执区分明确无 native owner 与原 backend 已收尾。设备保留最多 64 个原退休记录，仅可回收已结束写入者；旧 session 的收尾不停止替换 session。未知/已逐出记录不能冒充无输出。
- **异步收尾所有权**：设备整个输出退休等待原写入真正结束；Server 的 2 秒报告期限不丢弃同一退休 Future。SQL 取消与 native 收尾并行，所有者和依赖继续保留至实际 join。原 native/body 错误保持失败；诊断错误列表有界。
- **真实传输边界**：首媒体 reservation 使用递增后的正序号；首 Push ACK 可以大于 1，因为控制读取共享进度序号。仍验证原身份、单调递增、首 ACK 未确认任何 PCM/End/terminal。待确认登记不重发；冲突退休不能先破坏原 cache；Fence 之后迟到 ACK 不能复活原登记。
- **语义与时钟夹具**：A 的 23 actor 覆盖 fresh capture、新语音与 Resume、权限及两类暂停；把虚拟 Tokio 时间与真实 PG 混用的三个夹具改为短真实检查间隔，生产 30 秒权限复核间隔不变。真实模型首次冷启动超时与相同 8 秒预算后续通过均保留。

## 验证候选与失败记录

第一个统一候选 V1 共 **605 文件**，清单 SHA256 `4e6eadd8d28aeda542e8de0e6dbfbc855cd60d0c83c8c7cdc910702cc6002166`。`cargo test --workspace --locked --offline --no-fail-fast` 在默认测试栈和并发下得到 **964 passed / 3 failed / 5 ignored**，exit 101，520.33 秒；前后输入无漂移。子进程嵌套结果不重复累计。完整日志、逐目标计数在 `run/validation-20260923/workspace{.log,-counts.json,-result.json}`。

V1 的失败均保留为事实：

1. Embedded 两项绑定测试在解绑后复用同 boot/session/epoch=1，命中新规则的退休身份拒绝；Linux ARM64 完整 Embedded 库同样复现。正确修法是先断言旧 Ready 被拒绝且没有 active owner，再创建新 session 并验证旧 binding Evidence/guard 不影响它，不放松生产防复活规则。
2. Server Runtime 的 `panicked_playback_fence_retains_original_native_future_and_dependencies` 旧夹具直接投递带 voice scope 的 Audio，缺少原 OutputProducer，被当前入口拒绝，未进入 native 故障点。夹具需用规范 voice intent、原 producer/bind 与精确退休接口；保留 precise-cancel panic 后 native Future、channel、DB 的隔离断言。

V1 Linux 非 PG 检查：Core 27 / Proto 4 通过，Server 默认 all-targets、两端无默认 all-targets、受影响 Server 目标实际链接通过；Embedded 无默认完整库 **163 passed / 2 failed**。日志在 `run/validation-20260923/linux-final/evidence/`，源码只读挂载，运行后无漂移。没有把失败矩阵标绿。

## 最终验收与提交 checklist

- [x] 收齐三个原组件包与本轮三名 agent 的边界/定向结果。
- [x] 修复默认栈、Bind 身份、error notice、精确退休与传输竞争，并保留真实红绿证据。
- [x] 完成 V1 完整 workspace，明确发现并记录三项组合夹具问题。
- [x] 两份受影响测试文件修复后，建立 V2 独立清单与只读快照；其余未改输入须逐项核对，不覆盖 V1。
- [x] V2 macOS 完整 workspace、两端无默认 all-targets、三真实 PTY 和两真实模型测试。
- [x] V2 Linux ARM64 受影响矩阵，含整个 Embedded 库及实际 PG；失败、子进程重复和未运行项分别报告。
- [ ] 固定源码正常提交，合回 `codex/architecture-refactor` 后核对所有构建输入相同；不 push。
- [ ] 更新 DU、进度、机器状态和下一轮 checklist；核对任务 PG / 模型保留、临时验证容器退出。

旧 B/Root worktree 中的 Core/Proto overlay 继续保留，没有混入本轮源码。所有 `run/` 证据相对整合工作树；长期源码哈希与结果摘要会归档到 `docs/refactor/handoffs/2026-09-23/`。

## 尚不具备的证据

这些是软件、真实 PostgreSQL、loopback gRPC、终端和指定模型的验证；不证明 Pi 实机、真实声卡已静音、Presented、AEC、双讲听感、真实执行器位置或 24 小时稳定性。首轮剩余 R01—R06 见 [后续 checklist](../../checklists/next-round.md)。用户延期项 D01—D03 及信息 H01—H05 继续保留。

最终 V2：macOS workspace 967/0/5（203.85 秒），真实 PTY/模型 3+2/0；Linux ARM64 定向矩阵 325/0/0，Core/Proto、整个 Embedded 无默认库、受影响 Server 与完整 Runtime；no-default all-targets 与 27 文件 rustfmt 均通过。源码 605 项前后无漂移，任务 PG 原 3 baseline 保留、其它 clients=0，Linux V1/V2 容器已移除；日志/缓存/快照/模型与任务 PG 保留。上述常规/补测不重复计数。

## 用户额度暂停收口

用户在最终 V2 workspace 通过后明确要求暂停。Root exec 32901 已 exit 0 并收集，三个 subagent 均 completed；没有活动构建、测试或临时验证容器。本次提交保存所有源码、验证与当前文档，正式合并留待下一次 DU11。准确进度 10/11；首轮后续六项 0/6。下一次不重跑相同源码的已通过矩阵，先核对 605 个输入并正常合入。主分支生产代码仍为 e6ba75b，恢复材料会同步到主分支。
