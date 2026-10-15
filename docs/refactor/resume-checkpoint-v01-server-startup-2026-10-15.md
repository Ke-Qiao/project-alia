# 恢复检查点：V01 ServerStartup 初始化取消

2026-10-15；本轮 **10/10 完成**。分支 `codex/architecture-refactor`；源码 `9e3a93401aea481f729b21512431b980dfc3b02c`，上轮基线 `4ab48fe`。恢复先读[checklist](checklists/v01-server-startup-2026-10-15.md)和[四份 HANDOFF／证据](handoffs/2026-10-15-v01-server-startup/README.md)。

已完成：一次真实 RED 确认成功已缓冲后取消仍交付 Ready；交付前检查取消并复用原 rollback，保留原 grace、typed incomplete、完成句柄和原初始化错误。新增四项确定性回归，测试自身先 join 原 owner 再清 DB；三位 subagent 独立分工与交叉审查，没有新依赖或公共 API。

验证：RED0/1/0/81 filtered；一次 GREEN8/0/0/77 filtered；最终一次 workspace **1270/0/5/0 filtered**，505秒，其中 server_runtime85、duplex31。5项为既有 ignored，未启用真实模型或硬件。原期限、默认并发、DATABASE_CASES=2、CASES=3、shadow、政策与执行批准保持。

最终676项输入逐 blob 与源码提交一致，清单 SHA-256 `34595bfb7a942b17a3941474ae8c2c7392caa8c95f4f2e3847fdbf997011f205`。同库退出后约0.248秒快照0 schema/0其他client，任务 PG 容器与卷已按 exact ID/label 回收；共享服务保留。没有活动测试或遗留受控 gate。原始失败和成功证据均保留，恢复时不需要复跑本轮求绿。

后续有界软件候选：检查 `ServerBuilder::from_config` 在 `Database::connect` 返回 owned pool 前取消时，SQLx 已有资源清理与本项目可观察完成边界；先审计/复现，只有确认缺口才改。旧全局 with_fixture 的 body 失败 owner 顺序、SQL 未知确认、discovery 注册失败局部清理及极端 runtime/native/supervisor 失败未被本轮动态证明；不要把它们合并成新框架。Drop仅请求取消，完成必须以原 handle 为据。

**历史 Companion V01 25秒根因仍开放**；本轮是独立交付缺口，不能倒推历史原因。无新失败/artifact/可区分条件不重复历史批次。R02保持硬件AEC优先和资料门槛；R03后续按用户决定暂停；R05等待真实音频、机构与部署出口，六轮仍2/6。本轮没有Linux/Pi、真实模型或硬件验收，其他既定延期保持。
