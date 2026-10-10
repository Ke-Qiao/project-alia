# 当前检查点：2026-10-11 Skills HTTP 索引清理完成

分支 `codex/architecture-refactor`；本轮 **10/10**，L09—L12 **4/4**，R01—R06 仍 **2/6**，不是整个项目完成百分比。三个 subagent 已交接，任务容器/数据库全部停止，无待恢复 Cargo/PG 作业。

依次读取 [技术说明](skills-2026-10-11.md)、[checklist](checklists/skills-2026-10-11.md)、[四份 HANDOFF](handoffs/2026-10-11-skills/README.md) 和 [证据索引](handoffs/2026-10-11-skills/evidence-index.json)，核对 Git 状态后继续。

源码 `7aca2a6f02e6b6a4fd74ca241a06bc3970d28956`，V2 manifest `0ecb239bfe6ec3d4a9a12032e518376695e2b5b1b78f8fb3a79f3ce5a9639552`，643/643 Git blob 一致。

Skills 创建／更新／删除现在在同一变更 owner 下完成 SQL、完整索引准备、owned commit 和一次发布；成功 HTTP 响应之后即反映到当前 Registry 的 match/list/resolve。准备失败保留旧索引；提交失败重建实际状态，重建也失败时停用索引直到 reload 恢复。调用方取消与 stop 不丢弃已交接任务。旧无调用 CRUD/plugin facade 已删除，边界守卫防止回流。

macOS workspace **1068/0/5**；Linux ARM64 Server 默认并行完整回归 **786/0/5**。两端 Skills lib 定向各 **8/0/0**，无默认全目标检查、边界正反自测与变更文件格式检查通过。计数依次为 passed/failed/ignored，定向测试及嵌套子进程摘要不额外加到完整结果。所有最终结果同源、无源码漂移。

V1 两端 lib 7/1/0 的 pool.close 调度假设失败已保留；V2 仅测试夹具修正，不改生产规则或删除失败目标。表锁先于 RR snapshot 的隔离 PG 探针通过；Skills 网络 ACK 丢失本身没有端到端故障注入。

**下一项软件任务：独立审计静态 classifier 的 skill 发现与 Registry live snapshot 的衔接。读取 `routing/service.rs` 的两份静态模板、`classification_prompt_for_mode` / `classify` / `to_decision`，以及 `routing/mod.rs::RouterInput`、`generation.rs::route_response`，检查新增／禁用／删除 skill 的候选和退化，以及返回技能名的允许范围。先查已有实现再补缺口，保持全局工具可见性与 Execution 批准；本轮未实施。**

同步范围不含其他实例／外部 SQL 的主动通知，它们仍需显式 reload；静态 classifier 尚未自动发现新 skill。默认 Shadow、当前政策、工具可见性和执行批准保持。继续三个互斥 subagent，Root 统一验证与正常提交。

原 V01、R02 AEC、R03 物理反馈、Pi/24h/R05、其他平台/R06 和用户暂缓项见 [延期台账](deferred.md)。不扩依赖/feature/平台/模型/硬件动作，不访问用户 5433。资源关闭前其他连接和 schema 均为零；ignored 缓存保留。未推送/未建 PR，用户允许额度不足使用 reset 卡，本轮未使用。

未运行 Linux 全 workspace、release/native release link、真实 PTY、部署 smoke、Skills 链路的真实网络 COMMIT ACK 丢失注入、真实模型或硬件验证。本轮完整默认测试包含 native 路径，但不替代上述未运行项目；上轮已有证据只作为历史记录。
