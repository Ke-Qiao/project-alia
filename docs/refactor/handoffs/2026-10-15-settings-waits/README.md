# Settings 等待与关闭收尾交接

日期：2026-10-15；基线 `e89d34c`；分支 `codex/architecture-refactor`。本轮只修复实际复现的 Server 等待缺口，R03 暂停，无新增依赖、硬件动作或外部模型探测。

恢复先读 [Root](root-HANDOFF.md)，再读三份分工：

- [LLM 配置快照](model-HANDOFF.md)：三个 provider 的慢响应锁与生成取消。
- [SoVITS 额度期限](audio-HANDOFF.md)：额度准入、已有单请求期限、部分成功与 reservation 回收。
- [关闭与独立复审](review-HANDOFF.md)：HTTP/1 pipeline、客户端断开重试、privacy 共享锁及原静态猜测的撤回。

[本轮 checklist](../../checklists/settings-waits-2026-10-15.md) 与 [设计/审计记录](../../settings-waits-2026-10-15.md) 固定软件完成范围。本包 **10/10** 已完成，源码 `4e99134`。整包 Server **868/0/5**；最终 provider **51/0/0** 与无默认 all-targets 检查通过。见 [证据索引](evidence-index.json)、[统计](evidence/validation-summary.json)、[提交身份](evidence/commit-verification.json)和[清理](evidence/cleanup.json)。子进程重入及定向重跑不累计为新的测试数。

完整 Server 使用 green 清单；之后只整理一处 cfg 换行，最终源码使用 final 清单。二者的精确差异在 [格式转换记录](evidence/format-only-transition.json)，没有生产或测试逻辑变化。源码与测试正常提交后，669个输入逐blob核对通过。

保留的 Python 工具可从仓库内执行，默认只使用离线 Cargo 和现有缓存；复跑时须创建新的证据目录/冻结清单，并提供届时独立数据库记录。旧创建记录与固定容器 ID 是历史身份，不能直接用来创建或清理新一轮资源。不要删除原始日志以复用文件名。
