# PN01 Root HANDOFF

2026-10-17；基线`15604e3`，当前分支`codex/architecture-refactor`。三个原subagent分工为实现、复现测试、独立审查；Root统一运行、整合和提交。本轮**6/6**，插件实施**1/4**。

## 变更与证据

生产限`api/plugins.rs`和`plugins/storage.rs`：四管理操作要求当前admin，三个本人入口绑定认证UUID与user scope；读后复验，事务DELETE后锁定当前账号复验再commit，拒绝rollback。没有改Execution、政策、配置、依赖或schema。新增7项真实HTTP回归，复用既有runtime／PG夹具。

原生产RED 0/2：普通用户200跨owner导出，200删除8条目标插件记录。修复后HTTP 7/0/0（85个无关filtered），storage 4/0/0（全部执行），共11项；三项PG锁等待后失权均403且数据全快照不变。实际结果分别见[evidence-index.json](evidence-index.json)，不重复计算RED转绿或旧平台用例。没有新增或改动测试期限／并发。

任务使用已有pgvector镜像、显式loopback临时PG；结束前0 schema／0其他连接，`--rm`任务容器已删除。未触碰共享容器或启用模型、外部服务、真实硬件。最终提交核对限定改动文件与受测源码摘要，不重做旧全平台证据包。

## 接续与限制

用户明确希望加快接硬件：PN01后优先R02首个R1 USB接入、枚举与目标Pi音频权限核对。开启实际采集前补齐运行产物／准确设备／固定无插件且motion关闭配置／录音窗口；AEC参考接线另待功放和厂商电气资料。软件条件按这一步判断，不将完整模型链、PN02—PN04、市场、额外平台或长稳当作USB接入前置。

插件未经审核安装／隔离运行仍是明确后续目标，未在PN01实现；编译内storage仍是受信接口，不宣称已完成不可信broker。新API没有整体SQL／HTTP期限，账户复验复用既有2秒界；事务commit错误不声称成功。未做所有取消／数据库失联／崩溃故障穷举，不扩为另一个整备轮次。

默认shadow与当前政策／执行批准保持，R03继续暂停；V01历史根因open、R01—R06仍2/6。下一轮从[PN01恢复检查点](../../resume-checkpoint-pn01-2026-10-17.md)继续现场接入，不自动执行剩余三个插件阶段。
