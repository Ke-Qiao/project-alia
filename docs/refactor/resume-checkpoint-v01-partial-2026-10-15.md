# 恢复检查点：V01 部分初始化与 paired late-drain

2026-10-15；分支 `codex/architecture-refactor`；基线 `af5b35b`，源码 `dc3ec17`。本轮 **10/10完成**，三个分工初始化6/6、Drain5/5、Review5/5。历史V01根因仍开放，R03暂停，R01—R06仍2/6。

先读[checklist](checklists/v01-partial-2026-10-15.md)、[四份HANDOFF](handoffs/2026-10-15-v01-partial/README.md)及[Root](handoffs/2026-10-15-v01-partial/root-HANDOFF.md)，核对Git状态。不要重复已完成批次求绿。

## 已完成

- shared Fixture在CREATE前保留admin/schema/未连接pool，统一接住CREATE、connect、migration、seed、policy、epoch的panic/unwrap错误，尝试完整清理后恢复原失败。旧实现三项RED确认schema遗留，测试自己兜底回收；没有全局fault flag或生产机制变更。
- 实际paired channel.init SQL失败和表锁等待取消均证明部分Runtime保留、RPC/session/目录先退出，DB最后关闭；最外独立证据防止原panic掩盖cleanup失败。
- 持有原SubmissionWrite，真实观察11秒跨原10秒shutdown预算后仍pending；释放后等待同一close和dispatch，保持首次cleanup失败。没有放宽原25/32/45秒、3秒RPC、1秒诊断、10秒shutdown、CASES=3或默认libtest并发。

## 验证与回收

最终冻结674输入，SHA-256 `3f15e093efd3863143d5bcff0be0540ac4fb5c306b01b0650b69935abfd24747`，提交blob全部匹配。一次默认并发workspace **1262/0/5**（5项既有ignored、0filtered），405.60秒；duplex **27/0/0**，45.63秒。新增六项定向6/0，原RED0/3单独留证，不累计。最终退出约0.584秒后，同一未重启PG为0 schema/0其他client；本轮容器/卷已按完整ID与标签删除，共享服务保持。见[最终摘要](handoffs/2026-10-15-v01-partial/evidence/validation-summary.json)、[24份原件](handoffs/2026-10-15-v01-partial/evidence-index.json)及[回收](handoffs/2026-10-15-v01-partial/evidence/cleanup.json)。

## 继续工作的边界

1. 历史V01的25秒超时根因仍开放。本轮确认的是fixture清理缺口，不能倒推历史原因；只有新增真实失败、原失败artifact或可区分条件时再有限定位，不重复历史批次求绿。
2. 整个初始Fixture::new被外部任意drop、所有startup等待点、native锁中毒、supervisor panic/永不退出后代未穷举。running=true/nohandle不能视为已退出；保持owner/DB并明确未完成，不能清旗标或凭watchdog杀进程判成功。初始建库/迁移与最终SQL teardown仍在原45秒窗口外。
3. R02硬件AEC优先和厂商/输出链资料门槛保持；R03仍由用户暂停；R05等现场出口。没有新增依赖、Linux/Pi、真实设备、录放音、模型或长稳验证。其余延期见[台账](deferred.md)。
