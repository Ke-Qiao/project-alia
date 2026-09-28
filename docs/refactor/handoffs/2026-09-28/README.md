# 2026-09-28 · R01 live 投递整合交接

DU 11/11。R01 CP01—CP07 完成，源码已提交 `a79cb1f`；当前源为 630 输入 V1。默认政策 shadow，显式 Enabled 已接通规范固定邀请链，没有启动用户设备。

## 四份 HANDOFF

- [Root / Runtime](runtime.md)：runner、真实装配、排空、公平轮转、组合证据与后续分工。
- [Policy / Context](policy.md)：专用邀请 Context、完整来源、固定内容与权限检查。
- [Repository](repository.md)：原子准入、Fresh/Existing、预算、来源墓碑与有界容量。
- [Voice / Embedded](voice.md)：原 capture N、端侧临时 Hold、actor reserve 和 H2 完整恢复。

## 验证与后续

当前 workspace 1036/0/5 通过，默认栈/默认线程，source drift 为空；macOS 两端 no-default all-targets 通过。实际 PTY 3、Easy-Turn 2、Linux ARM64 两端编译均已通过，合计 1041 项不同 macOS 测试；源码已提交 `a79cb1f` 且输入一致。最终数字见 [验证摘要](validation-summary.json)。定向与全量重叠用例不重复累计。

运行原件在 `run/validation-20260928/`，可恢复入口为 [检查点](../../resume-checkpoint-2026-09-28.md) 和 [机器状态](../../resume-state-2026-09-28.json)。R01 收口后按 [R04 三项核对](r04-readiness.md) 继续；硬件、声学和长期负载仍在原 checklist 中开放。
