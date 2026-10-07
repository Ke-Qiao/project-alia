# 当前检查点：2026-10-08 分层迁移完成

分支 `codex/architecture-refactor`；本轮 **L01—L08：8/8**，下一阶段 **L09—L12：0/4**，R01—R06 仍 **2/6**。这不是整个项目完成百分比。实现提交 `08c54be`；Appraisal 测试夹具修正 `978af9c942da42aa3cd98182cde3dafa6a6e33be`。没有待接回的 Cargo、PostgreSQL 或容器任务，三名 subagent 已交回。

恢复时依次读取：

1. [本轮技术说明与最终验证](layering-2026-10-08.md)、[8 项 checklist](checklists/layering-2026-10-08.md)。
2. [四份 HANDOFF](handoffs/2026-10-08/README.md) 与 [证据索引](handoffs/2026-10-08/evidence-index.json)。
3. [下一阶段 L09—L12 checklist](checklists/layering-next.md)、[六轮任务](checklists/next-round.md)、[延期台账](deferred.md)，核对实际 Git 状态后继续。

Memory 的类型、策略、存储与采集已归 `modules/memory`，固定 `MemoryService` 替代旧 provider trait/no-op/单实现工厂。Interaction 拥有输入监督及分阶段生成、上下文、表达和持久化，不再接收全局 Registry；旧 `channels/pipeline` 已删除。Bootstrap 组装、Platform 拥有生命周期，旧 `module` 门面已删除。默认 Shadow、当前持久政策、Context/来源身份与事务、两类暂停、Response/Delivery 和工具迟到结果保持。

最终 V4 630 输入，manifest `ccfaa7981fc7062dfeb43e568e52af038915064a2187352e0891980afc6fcbd3`，630/630 对应 Git blob。macOS 默认 **1053/0/5**、真实 PTY **3/0**；Linux ARM64 Server 全包 **771/0/5**、默认 release/native link 与无默认 all-targets通过；macOS Server 无默认、格式与架构守卫通过。两项真实模型未重跑；Linux 全工作区、独立部署 smoke 和真机不在本轮结果内。

V2 缓存取消断言曾准确失败并修复共享退出点；V3 Linux 首轮 Appraisal 期限夹具竞争为 770/1/5，同源单项复核通过但没有掩盖问题。V4 只改变该夹具，把排队 caller 的确认期限与自身 writer/SQL 期限分开；没有改变生产期限或削弱原身份/清除/重启断言。全部候选日志分开保留，不混算通过数量。

**下一条可执行任务：L09 Conversation 的固定核心与契约归属。** 按实际调用链核对 trait 默认行为、来源双 epoch、Response/history 原子提交与重试身份，再迁往 Interaction。后续顺序见 checklist：Cache、Router、Skills、Persona、Insights、Sensor，再总体验收；L10 初始化依赖、L11 跨域事务、L12 出站契约保持独立验收项。

不恢复旧兼容路径，不增加无实际替换需求的 trait/工厂；保持三个 subagent 的互斥文件分工和 Root 唯一 Cargo/PG 窗口。用户 5433 未访问；任务资源已停止，ignored run 中的缓存与证据可复用。前端、真实 AEC/R02、执行器反馈/R03、V01、Pi/24h/R05、其它指定平台/R06 与端上模型继续延期。没有推送或创建 PR。
