# 输出生命周期阶段验收 · 2026-09-22

状态：用户因额度不足明确要求暂停。实现和测试已停止，Root 完整组合验证尚未通过。
交接位于 `run/output-lifecycle-handoff-20260922/HANDOFF.md`，精确命令、哈希和资源记录见同目录 `RESULT.json`。

- [x] 控制账本固定提交 `4846b4a` 与 `56e9acc` 已合入；完整真实 PG 13/13。
- [x] 后加入 Audio 的硬 scope 期限持久收紧，Resume 不丢失该限制；单次 hold 不永久压缩整个输出寿命。
- [x] 队列 enqueue 前保留原 MediaOwner；drop waiter/最后 producer 不丢排队媒体，重复查询不重放。
- [x] ACK 丢失仅 Observe 原 command；无法确认保持 Unknown，不能自动重发 Control。
- [x] 实际 SQL 许可到期时关闭本地 gate 并退休；不把未收到的 ACK 写成已确认结果。
- [x] Held 期间原媒体 owner 的 SQL heartbeat 持续，未重新 reserve/dispatch。
- [x] 原 scope fence 超时仍保留实际 future/channel，真实 join 前 shutdown 不成功。
- [x] 明确本地取消/退休先持久化，不等待未知设备 stop；同时保留原 stop future 到实际结束。
- [x] 原 `delivery_service` 12 例迁移到真实原 producer/binding，原 exact cancellation 故障语义保留。
- [x] 默认两目标真实 PG：`delivery_service` 12/12（11.35 秒）与 `output_lifecycle` 6/6（5.62 秒），exit 0。日志 `run/output-lifecycle-pg.log`。
- [x] 最后 cleanup-only 调整后，受影响单项再次通过（2.58 秒，exit 0）；日志 `run/output-lifecycle-fence-pg.log`。唯一测试数仍为18，不将重跑累加。
- [x] 两份测试文件冻结；只读审查及生产修复分工已记录，共享测试尚由 Root 统一提交。
- [x] 本代理测试 schema 已清理。另按 Root 授权核对其 SIGABRT 后差集，仅删除2个新增 pipeline schema；原3个历史schema保留，其他客户端连接0。PG窗口已释放。
- [ ] 优先修复 Root Pipeline integration 中 `a_large_provider_chunk_splits_only_at_utf8_boundaries_under_one_delivery` 的 stack overflow/SIGABRT。20项请求仅打印3个ok后中止，exit101；根因未知，原日志已保存，不计整套通过。
- [ ] 用户恢复后完成 A/C/Root 剩余 actor/live/paired transport 验收。
- [ ] 完整组合源码冻结后的 workspace/特性与真实端到端验证；不能将以上定向结果称为全workspace通过。
- [ ] Root审阅dirty归属并完成正常提交；原账本树6项Core-only临时覆盖继续排除。
