# 2026-09-23 暂停纵向整合收口与后续入口

**用户因额度要求暂停，已停止新增构建和测试，三个 subagent 均已完成并收口。当前完成 DU01—DU10：10/11，剩余 DU11：1/11。** 本次只保存检查点，正式合入留待额度恢复。机器状态见 [resume-state-2026-09-23.json](resume-state-2026-09-23.json)。本轮从 2026-09-22 暂停快照接续，未重新启动 DU01/DU02，也未覆盖旧工作树的 Core/Proto overlay。

## 源码与实际行为

- 主分支生产源码仍为已验收历史基线 `e6ba75b`；本次主分支仅同步恢复文档，不能把整合分支通过等同于已经部署/合入。
- 工作分支：`codex/duplex-integration`；最终目标分支：`codex/architecture-refactor`。
- 默认线程栈修复：`9c4f5b0d379166126d9dee0049dbcda1dddef01b`。
- 身份、退休与组合修复：`581068795fb1dbf5449ed9a0f0b73665c1797a7a`。
- V2 输入：605 文件，清单 SHA256 `1939a948191002882ba70e2578ad87d266d428b2ba423ccfa31aa178a34998d4`；[完整清单](handoffs/2026-09-23/source-manifest-v2.json)。与 V1 只差两份旧测试夹具，603 个其余输入完全相同。

voice v2 与 playback v3 成对接入 Server、Embedded 和规范 PostgreSQL 账本。临时疑似插话等待新鲜语义，确认附和可恢复；明确 WAIT 保留原输出并等待明确 Continue。取消后的 generation 不复活。Pause/Resume 保留原 Response、Context、来源、epoch、backend、播放位置、producer 和媒体所有权，在途 LLM/TTS 只保留原操作及有界结果。

Bind 在设备 ACK 前也有可信规范身份；错误和取消不会漏掉原 Response，也不在同 scope 生成第二份错误回答。精确 `RetireOutput` 保留旧 owner 的真实收尾事实，不能用新的 active session 代替旧输出已经结束的证明。首个媒体身份序号为 1；首个进度回执序号可能大于 1，控制与媒体共享观察计数。

60 秒默认 Hold、120 秒上限和 3 秒临时语义期限是工程参数；仍受原输出寿命约束。真实声卡、物理静音、Presented、AEC 和双讲听感未因此完成。

## 验证与失败保留

| 检查 | 当前结果 |
| --- | --- |
| V1 macOS 默认完整 workspace | 964 passed / 3 failed / 5 ignored，exit 101，无输入漂移 |
| V2 macOS 默认完整 workspace | 967 passed / 0 failed / 5 ignored，exit 0，203.85 秒；5 个 ignored 已另行全部通过 |
| V2 macOS 两端 no-default all-targets | exit 0 |
| V2 真实 PTY / Easy-Turn | 3 + 2 passed，0 failed，原期限不变 |
| V2 Linux ARM64 本轮矩阵 | 325 unique passed，0 failed；Core/Proto、整个 Embedded 无默认库、受影响 Server 与完整 Runtime；两种 all-targets 检查通过 |
| 变更 Rust 格式 | 27 文件通过 |

V1 的两项 Embedded 失败来自旧夹具试图复用已退休 epoch；修正先断言旧身份拒绝，再用新 session 验证旧 Evidence/guard 不影响新 owner。Runtime 的一项旧夹具遗漏原 producer/bind，未进入预期 native panic 场景；修正使用正常入口，保留隔离子进程、20 秒期限和依赖存活断言。定向 8/8 与完整 Runtime 43/43 通过；V1 失败日志和汇总保留，复跑不增加不同测试数。

此前栈溢出、模型首次 8 秒超时、Bind 取消、额外 Context、首媒体序号与配对失败也分别保留原始证据。模型服务未重启或清缓存，本次补测不称冷启动验证。完整平台说明见 [platform-validation.md](platform-validation.md)。

## 四份交接与证据位置

- [Voice](handoffs/2026-09-23/voice.md)：语义、权限、采集屏障与真实模型；[V2 终端/模型补测](handoffs/2026-09-23/pty-model-v2-result.json)。
- [Lifecycle](handoffs/2026-09-23/lifecycle.md)：栈分析、Bind 取消、原生收尾与错误边界；[Runtime 夹具补测](handoffs/2026-09-23/runtime-fixture-result.json)。
- [Playback](handoffs/2026-09-23/playback.md)：真实两端 gRPC、旧 owner、替换 session 与精确退休。
- [Root Runtime](handoffs/2026-09-23/runtime.md)：纵向整合、候选差异、全量验证及提交。

原始日志、只读源码快照、二进制和本轮缓存保留在 `run/worktrees/alia-duplex-integration/run/validation-20260923/`。组件阶段原件与最终 V2 结果分开归档，不能把组件通过数相加成最终测试数量。旧 2026-09-20/22 检查点继续作为历史。

## 后续 checklist

- [x] 完成 V2 最终 workspace、平台、真实 PTY/模型验证，正常保存源码与文档；四份最终 HANDOFF/结果/源码清单已归档。
- [ ] **DU11（下次首项）** 核对两分支 clean 与保存提交；合回 `codex/architecture-refactor`，处理文档冲突时保留最新暂停/验证事实；核对全部 605 源码哈希，再更新 11/11 并正常提交。不因纯文档合并重跑整套测试；源代码发生变化才按影响复验。
- [ ] **R01** Companion shadow / 回放与受约束主动表达。
- [ ] **R02** AEC、时钟映射/漂移、Mac/Pi 双讲。
- [ ] **R03** D3 动作资产、执行器反馈/限位与真实位置证据。
- [ ] **R04** CLI/REST 管理诊断与选定组件。
- [ ] **R05** Mac/Pi 全新安装、故障恢复及至少 24 小时混合负载。
- [ ] **R06** 其他 Server 平台与未来同机部署/模型下沉的分项评估。

六个方向当前 **0/6 完成**，细分出口见 [下一轮 checklist](checklists/next-round.md)。首轮质量目标不降低；Web/iOS/Android、通用 Embedded 适配按 [D01—D03 台账](deferred.md)继续延期，Pi 5 运行 Easy-Turn 不纳入当前评估。H01—H05 缺失的信息与相应实机验收仍开放。

继续时使用三个 subagent，先核对 Git 与本文件最终状态，再按下一条未完成 checklist 执行。不要重新合并旧 overlay，不要把尚未取得的硬件证据写成完成，也不要反复征询已确认的两类暂停选择。

## 暂停资源与恢复提示

活动 subagent **0**，活动构建/测试 exec **0**。Linux 两个本轮验证容器已移除。任务 PG `alia-refactor-test-db:32768` 与 Easy-Turn `127.0.0.1:8091` 保留；测试 DB 只有原三个 `pipeline_delivery_*` schema 和 public，其他客户端 0。旧工作树、Core/Proto overlay、模型、缓存、原始日志和只读快照均保留；未操作用户 5433 数据库，没有 push。

可直接发送：

> 额度已恢复。请从 `docs/refactor/resume-checkpoint-2026-09-23.md` 继续，读取四份最新 HANDOFF，继续使用三个 subagent 并维护 checklist。DU01—DU10 已完成且 V2 验证通过，先完成 DU11 正式合入和合并后哈希核对，不重跑未变源码的完整验证；然后从 `docs/refactor/checklists/next-round.md` 的 R01 开始。
