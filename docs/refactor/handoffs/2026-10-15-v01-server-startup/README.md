# V01 ServerStartup 初始化取消交接

2026-10-15；本轮 **10/10 完成**。分支 `codex/architecture-refactor`，源码 `9e3a93401aea481f729b21512431b980dfc3b02c`。历史 Companion V01 根因仍开放，R03暂停，R01—R06仍2/6。

- [Root](root-HANDOFF.md)：范围、修复、验证、资源与后续边界。
- [Ownership](ownership-HANDOFF.md)：资源接管与生产回滚契约。
- [Cancellation](cancellation-HANDOFF.md)：四项阶段同步与取消回归。
- [Review](review-HANDOFF.md)：独立源码、原始证据和身份复核。
- [恢复检查点](../../resume-checkpoint-v01-server-startup-2026-10-15.md)与[checklist](../../checklists/v01-server-startup-2026-10-15.md)。

| 批次 | 结果 | 说明 |
| --- | --- | --- |
| 旧实现 RED | 0通过/1失败/0 ignored/81 filtered | 已取消的缓冲成功仍交付；先清资源再报告目标失败 |
| 一次定向 GREEN | 8/0/0/77 filtered | 四项新增加四项既有回滚，4.34秒 |
| 一次最终 workspace | **1270/0/5/0 filtered** | 505秒；server_runtime85/0/0、duplex31/0/0 |

5项 ignored 是既有 PTY3项、真实 Easy-Turn2项；没有新增忽略。默认libtest并发、DATABASE_CASES=2、Delivery CASES=3及原期限保持。正常API、默认shadow和当前政策不变。

最终输入676项，SHA-256 `34595bfb7a942b17a3941474ae8c2c7392caa8c95f4f2e3847fdbf997011f205`，[逐blob提交核对](evidence/commit-verification.json)全部一致。[最终结果](evidence/validation-summary.json)与[原件索引](evidence-index.json)可独立复算，RED源码快照和失败原日志均保留。最终binary在ignored run中保留，身份已记入结果；不会把GREEN与全量二进制当成相同产物。

同库结束约0.248秒后[资源为0/0](evidence/final-resources.json)，随后按exact ID/label[回收任务容器与卷](evidence/cleanup.json)，共享服务保留。`git diff --check`仅报告原始`final-workspace.log:1787`和`startup-green.log:24`的EOF空行；为保留逐字节原件不删除，排除原日志后的作者文档/源码检查通过。原始日志不美化；归档脚本沿用原run目录布局，不能直接在evidence目录作为通用工具运行。

没有新依赖、Linux/Pi或真实模型/硬件验收。这里只证明所列初始化与交付路径，不包含所有early connect、SQL未知确认、discovery或极端native/runtime故障；详见Root边界记录。
