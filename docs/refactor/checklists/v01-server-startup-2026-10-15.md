# V01 ServerStartup 初始化取消 checklist

2026-10-15；基线4ab48fe，分支codex/architecture-refactor。当前 **10/10 完成**；源码 `9e3a934`。

- [x] 读取上轮检查点、四份HANDOFF和已有证据，核对基线。
- [x] 三个subagent分工，审计ServerStartup/RuntimeCleanup/调用方。
- [x] 区分已有保证、buffered结果交付候选与未动态覆盖的相邻风险。
- [x] 确定性复现成功已缓冲后取消仍交付的问题，先取证后兜底。
- [x] 只修已确认缺口，复用原回滚错误与typed incomplete语义。
- [x] 覆盖未poll取消、越grace仍保owner、完成重等和原错误保留。
- [x] 独立复审源码及测试的阶段门/失败清理，冻结输入。
- [x] 有界定向和完整回归，原期限/默认并发/既有ignored保持。
- [x] 核对提交输入、同库资源与任务容器回收，归档失败和成功原件。
- [x] 更新四份HANDOFF、全局台账和恢复检查点并正常提交。

默认shadow、政策和执行批准不变；R03暂停，不新增依赖或启用硬件。历史V01根因open，六轮仍2/6。
