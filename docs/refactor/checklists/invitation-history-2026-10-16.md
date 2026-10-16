# CompanionInvitation 历史只读引用 checklist

2026-10-16；基线95cd077；本轮10/10完成（2026-10-17收口，源码4e2968f）。

- [x] 读取已有实现、历史待办和当前验证身份。
- [x] 三个subagent分工审计，定位历史过滤与输出授权边界。
- [x] 用户确认历史事实与输出有效期/重启/shadow分离，来源与隐私保持。
- [x] 确定[完整契约](../companion-invitation-history.md)与派生/删除影响，保存最小确定性RED（0通过/1预期失败，资源0/0）。
- [x] 实现独立只读历史证据，保持原邀请用途和当前Generation授权。
- [x] 补同owner/session、未提交、篡改、撤回、到期/服务对象重建/shadow及派生回归。
- [x] 独立审查并冻结源码（677项，GREEN清单b6b481d6；fmt/diff通过）。
- [x] 定向与完整回归，失败原件和源码身份独立保留。
- [x] 核对资源回收与提交输入，归档证据。
- [x] 更新四份HANDOFF、全局台账和恢复检查点，正常提交。

默认shadow、执行批准、精确取消与资源清理保持；R03暂停，无新依赖/真实硬件。

最终workspace1277/0/5/0filtered；677项输入逐blob匹配，34份原件已归档，任务PG资源0/0并已回收。详情见[四份HANDOFF](../handoffs/2026-10-16-invitation-history/README.md)。
