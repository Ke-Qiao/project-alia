# 恢复检查点：CompanionInvitation 历史只读引用

2026-10-16 批次，2026-10-17 完成。分支 `codex/architecture-refactor`，源码 `4e2968f18d765f326e76d24a3bcdf5bba9034efc`；本轮 **10/10**，不是项目整体百分比。六轮仍2/6。

先读[checklist](checklists/invitation-history-2026-10-16.md)、[四份HANDOFF](handoffs/2026-10-16-invitation-history/README.md)和[契约](companion-invitation-history.md)。用户已确认已提交邀请在输出到期、服务重建或回shadow后仍可作为历史事实读取；来源、隐私与当前对话政策仍约束读取，原邀请不扩大用途，不重播或恢复批准。

本轮先保存真实RED（0/1/0/377filtered），再完成独立只读marker、规范身份和全部非admission根/真实TTL、派生Memory保存与防自我强化。一遍定向13/0/0/371filtered，一遍完整workspace1277/0/5/0filtered，72目标、555.32秒；Server库381通过/3既有ignored、server_runtime85、duplex31。5项既有ignored未启用，无新增忽略。原期限和默认并发保持，无新依赖或硬件。

最终输入677项，SHA-256 `4713f1a8f475c8524ca8c8dec270fbec441a984eea913bc8d49f98482b785ac2`；逐blob核对与源码提交完全一致。34份原件共656064字节保存到HANDOFF/evidence及evidence-index.json，ignored run保留对应二进制。测试结束约0.258秒后同库0schema/0client，任务容器和卷已回收，共享服务保留。源码已提交；恢复材料随同一轮文档提交，不需重复已完成测试。

下一次先核对git状态和本检查点，按新任务范围选择后续软件出口。历史V01根因仍open；本轮不以成功回归关闭原根因。R03仍暂停，不自动启用Pi、录放音、执行器或付费模型。

## 后续软件验收候选（不计入本轮10项）

- [x] 当前源码Linux构建、测试与release链接已于2026-10-17完成，见[最新恢复检查点](resume-checkpoint-linux-release-2026-10-17.md)。旧版Linux记录仍仅保留各自源码边界。
- [ ] 使用已有全noop staging模板验证真正进程的启动、退出和重启，保持设备与外部付费探测禁用。
- [ ] 用户确认模型服务端点、版本和运行方式后，验证实际模型链；当前七项新测试没有调用真实模型。
- [ ] 冻结受测源码、可执行文件、配置模板和恢复说明；按实际证据决定软件发布出口。
- [ ] 历史V01原根因继续有界跟踪；本轮邀请历史完成不将其关闭。

若后续要求含邀请历史的用户Appraisal再次成为主动候选，需要独立澄清实际读集和防自我强化的来源契约；不能通过放宽marker校验解决。R02仍待硬件AEC资料与实测，R03保持用户暂停，R05在软件与现场门槛齐备后执行。未开启前端或Embedded通用适配。
