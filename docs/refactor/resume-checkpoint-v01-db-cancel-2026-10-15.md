# 恢复检查点：V01 初始数据库构造取消

2026-10-15；分支 `codex/architecture-refactor`。本轮 **10/10 完成**，源码 `8bcda7af892ba36aea98b9de785afa68e3935940`；基线 `aca6ca0`。历史 V01 根因仍 open，R03 暂停，六轮仍 2/6。

先读[checklist](checklists/v01-db-cancel-2026-10-15.md)、[四份 HANDOFF](handoffs/2026-10-15-v01-db-cancel/README.md)与[证据索引](handoffs/2026-10-15-v01-db-cancel/evidence-index.json)，再核对 Git。

已替换测试夹具中可被直接丢弃的局部构造：独立 FixtureInitialization worker 持有原 connect/SQL、Fixture 和 CASE permit；取消只撤销交付，真实在途操作完成后才清理。ready 未 poll 取消、缓冲后取消重等、CREATE 在途与原 completion 重等均有覆盖；初始化原 panic 与次生清理错误分别保留。生产代码、API、默认 shadow 与政策未修改。

验证批次必须分开：旧实现 RED 0/2 证明两处资源遗留；V1 定向7/0通过，但完整V1为1265/1/5，暴露buffered-ready测试在进入阶段前计时。只修UserSeed entered/release阶段门与缺证据observer清理后，V2完整workspace **1266/0/5/0 filtered**、duplex **31/0/0**，285.11秒；5 ignored均既有PTY3与真实模型2，不累加先前批次。

最终675项冻结输入与提交blob完全一致，manifest `51b11511010fedfa47cf8f01b814eb17731ded45b5404fb7ad01ff2e4ce9c553`。同库在退出约0.175秒后为0 schema/0其他client；本轮临时容器与卷已经按exact ID/label移除，共享运行时/服务保留。没有当前测试进程或需要继续使用的本轮PG。原件归档31份，V1与V2二进制保存在ignored `run/validation-20261015-v01-db-cancel/`，SHA入索引。

下一轮规则：无新增失败、原artifact或可区分条件时不重复V01历史批次求绿；本轮具体取消缺口已关闭，不能用它追认历史25秒根因。后续按用户指定范围继续；R02硬件AEC优先，板/固件、输出链、电气/模式/延迟资料仍待补；R03保持暂停，R05等待现场出口。无需重做已完成的本轮测试或重新建本轮数据库。

保留原25/32/45/3/1/10秒、CASES=3与默认libtest并发。初始数据库准入/构造及最终SQL teardown仍在原45秒之外；新增观测5秒只从实际阶段开始，没有延长其长度。Drop不等于完成；所有completion丢弃、runtime退出/强杀、DB永久失联不能排空、native poisoning与supervisor自身panic等不由本轮证明，不新增全局supervisor。未执行Linux/Pi、真实模型、录放音或执行器，无新增依赖。
