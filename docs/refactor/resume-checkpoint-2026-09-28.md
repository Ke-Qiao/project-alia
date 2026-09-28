# 2026-09-28 继续构建入口

分支 `codex/architecture-refactor`，本轮起点 `8d74b6c`；已验证 shadow 基础 `98f237c`。用户额度恢复后，Root 和三个 subagent 已完成 CP05 live 整合与 CP06 故障矩阵，CP07 已完成并正常提交 `a79cb1f`，已提交 630 输入逐项与验证 V1 一致。

**DU 11/11；R01 CP01—CP07 7/7；R01—R06 1/6。** 当前继续 R04 的三个已定位软件问题，0/3 待实现/验证。

- [x] 读取上轮四份 HANDOFF，保留当前分支、旧 overlay 和历史失败证据。
- [x] 三个 subagent：`companion_replay_28` 存储，`companion_policy_28` Context/内容与 Linux 编译，`companion_delivery_28` 语音 actor 与设备 capture gate。
- [x] Root runner、服务装配、单 channel/global 容量、公平轮转和原 owner 停机排空。
- [x] 正向主动邀请完整配对；TTS 期间删源零首媒体；原 H1 Resume ACK 丢失后新语音产生 H2，再明确继续原音频后缀。
- [x] 630 输入 V1：workspace **1036/0/5**，真实 PTY **3/3**、实际 Easy-Turn **2/2**；合计 **1041 项不同 macOS 测试**。两平台 Server/Embedded no-default all-targets、fmt 通过，全部源码哈希无漂移。
- [x] 四份 [HANDOFF 与验收摘要](handoffs/2026-09-28/README.md)、准确边界及原始失败已归档。
- [x] 正常本地提交 R01 `a79cb1f`，已提交输入与 V1 一致；CP07/R01 完成。
- [ ] 从 [R04 三项已定位缺口](handoffs/2026-09-28/r04-readiness.md) 继续，不依赖前端或真实硬件。

默认政策仍 shadow；显式 Enabled 已接入规范 Context/Response/Delivery 固定邀请。`admit_before` 仅约束提交与首次 reserve，`valid_until` 保持原候选截止；重试/Hold/继续均不续权。没有启动用户 Server/device。

任务 PG 在 32768 保留运行，测试后非 public schema 与其他连接均为 0；用户 5433 数据库未操作。真实 CPU Easy-Turn 在 8091 保留运行，本次 Linux 验证容器已停止，所有快照和缓存保留。旧工作树不需要重新合并。

机器状态见 [JSON](resume-state-2026-09-28.json)，逐项出口见 [Companion checklist](checklists/companion.md)。真实 Pi 音频、AEC、执行器和 24 小时运行仍开放；硬件信息已向用户询问，暂无回复时先完成独立软件工作。前端和通用 Embedded 适配等继续见 [延期台账](deferred.md)。
