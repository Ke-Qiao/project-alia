# CompanionInvitation 历史只读引用交接

2026-10-16 批次，2026-10-17 完成；本轮 **10/10**。分支 `codex/architecture-refactor`，源码 `4e2968f18d765f326e76d24a3bcdf5bba9034efc`。默认shadow与当前政策保持；R03暂停，历史V01根因open，六轮仍2/6。

- [Root](root-HANDOFF.md)：完整契约、改动、证据和后续软件出口。
- [Ownership](ownership-HANDOFF.md)：来源、权限、期限与派生根归属。
- [Tests](tests-HANDOFF.md)：RED与七项新回归的精确覆盖和限制。
- [Review](review-HANDOFF.md)：独立源码与证据复核。
- [契约](../../companion-invitation-history.md)、[checklist](../../checklists/invitation-history-2026-10-16.md)、[恢复检查点](../../resume-checkpoint-invitation-history-2026-10-16.md)。

| 批次 | 通过/失败/ignored/filtered | 证据含义 |
| --- | --- | --- |
| 旧实现RED | 0/1/0/377 | 唯一预期遗漏断言失败，旧源码与原日志保留 |
| 一次定向GREEN | 13/0/0/371 | 原6项+新7项，测试体5.63秒 |
| 一次最终workspace | **1277/0/5/0** | 72目标；555.32秒，Server库381通过/3既有ignored、runtime85、duplex31 |

5项ignored是原有PTY3项与真实Easy-Turn2项，无新增忽略；默认libtest并发、原fixture期限与CASE许可保持。定向、RED和全量不相加。

最终677输入SHA-256 `4713f1a8f475c8524ca8c8dec270fbec441a984eea913bc8d49f98482b785ac2`，[逐blob提交核对](evidence/commit-verification.json)无缺失或差异。[结果摘要](evidence/validation-summary.json)、[原件索引](evidence-index.json)和34份原始文件共656064字节均可复算；二进制保留在ignored run中，身份记入JSON，GREEN与全量产物分别记录。

结束约0.258秒后[同库资源0/0](evidence/final-resources.json)，随后[exact ID/label回收任务容器和卷](evidence/cleanup.json)，共享服务保留。fmt与作者源码/文档diff检查通过；归档后的完整diff检查仅报告原始 `final-workspace.log:1794` 和 `invitation-green.log:21` 的EOF空行；排除这些原日志后的作者源码/文档检查通过，原件不美化字节。证据脚本沿用原run目录布局，不是可在本归档目录直接运行的通用工具。

没有新依赖或迁移，没有Linux/Pi、真实模型/音频/执行器验收。服务对象重建不等于实际进程重启；模型消费撤回不等于裸删SQL后物理擦除所有管理可见副本。携带历史来源的Appraisal不得再次成为Companion来源；后续若需恢复候选，应明确实际读集，不能白名单放行marker。
