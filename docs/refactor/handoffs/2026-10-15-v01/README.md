# V01 旧 Companion 配对超时 — 有界调查交接

2026-10-15；基线`c685e5a`，测试修复`f817623`；分支`codex/architecture-refactor`。调查包**10/10完成**，旧超时未复现、**V01仍open**；R01—R06仍2/6，R03暂停。

- [Root](root-HANDOFF.md)：结论、完整验证/资源、恢复与提交。
- [Flow](flow-HANDOFF.md)：scanner/runner/actor/SQL期限与证据边界，7/7。
- [Fixture](fixture-HANDOFF.md)：配对并发、失败诊断与隐私回归，6/6。
- [Review](review-HANDOFF.md)：历史artifact身份、最终diff/计数/资源独立复核，6/6。

两批同baseline binary默认并行各9/0/0，诊断版完整workspace1245/0/5（duplex10/0/0）；三批上限已达到，不再重复。仅修fixture失败观测，未改生产、期限、并发、默认shadow或当前政策。5个ignored是既有PTY/Easy-Turn前置用例，不是本轮规避。

交付入口：[checklist](../../checklists/v01-2026-10-15.md)、[恢复检查点](../../resume-checkpoint-v01-2026-10-15.md)、[历史身份审计](review-history-audit.json)、[验证摘要](evidence/validation-summary.json)、[669提交输入核对](evidence/commit-verification.json)及[证据索引](evidence-index.json)。证据中UTC时间对应本地Asia/Shanghai的2026-10-15，日期不混作不同轮次。

[最终资源记录](evidence/final-resources.json)和[任务回收](evidence/cleanup.json)确认无相关测试进程、任务容器/卷已删除；全量结束当刻schema/连接快照缺失，明确不以baseline0/0或重启后状态替代。共享runtime启动用于回收后保留astrbot运行。大binary在ignored原目录，归档脚本保留原run目录假设，仅作证据，不能直接在这里启动。

收口[静态核对](static-verification.json)确认源码未漂移、归档原件一致、相对文档路径有效，源码与新写文档的diff格式通过；本次恢复没有增加测试批次。

暂存后的完整diff检查在原样证据中报告4处空白：三份日志的文件末尾空行，以及`validate.py:25`行尾空白。为保留原件哈希不改写这些证据；排除`evidence/`后的暂存检查通过，详见静态核对记录。
