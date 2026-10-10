# Skills HTTP 索引清理 Root HANDOFF

本轮 **10/10**，三个 subagent 分别完成 Registry、HTTP、真实回归；Root 统一集成、运行 Cargo/PG/Docker、收尾及提交。L09—L12 **4/4**，R01—R06 **2/6**，不是整个项目完成百分比。

源码 `7aca2a6f02e6b6a4fd74ca241a06bc3970d28956`，V2 manifest `0ecb239bfe6ec3d4a9a12032e518376695e2b5b1b78f8fb3a79f3ce5a9639552`，643/643 Git blob 一致。

## 实现结果

已有 HTTP 领域事务保留；三个 reload TODO 被统一 SkillChange owner 替代。完整 RR snapshot 在提交前准备，提交由 RuntimeTasks 持有，成功后一次发布。准备失败不改变旧索引；提交结果错误先通过表锁等待原事务，再重建实际状态；重建失败时索引不可用，健康 degraded，成功 reload 恢复。HTTP 响应提交前准备，update 读取位于同一事务。等待者取消不丢弃 commit，stop 关闭准入并等待 owned worker。

删除无调用 Registry CRUD/get/list 和 plugin skill facade；边界守卫阻止旧路径恢复。Shadow、工具可见性、执行批准、依赖校验、禁用过滤、请求和错误语义保留。无新增依赖、feature、配置、迁移或硬件动作。

## 验证与真实失败

macOS workspace **1068/0/5**；Linux ARM64 Server 默认并行完整回归 **786/0/5**。两端 Skills lib 定向各 **8/0/0**，无默认全目标检查、边界正反自测与变更文件格式检查通过。计数依次为 passed/failed/ignored，定向测试及嵌套子进程摘要不额外加到完整结果。所有最终结果同源、无源码漂移。

V1 HTTP 3/0/0；V1 macOS/Linux lib 均 7/1/0，失败是 fixture 要求 pool.close 首次 poll Pending。V2 只改一份 registry_tests.rs，允许 Ready/Pending，直接断言 pool.is_closed，确保 worker 完全退出再替换测试依赖；原失败断言的生产目标全部保留。原日志与修正结果分开保存。PG 探针实证先等待表锁再读取 RR snapshot；不声称已注入 Skills 真实网络 ACK 丢失。

完整命令与逐目标计数在 [证据索引](evidence-index.json)；[技术说明](../../skills-2026-10-11.md#最终验证与证据) 给出平台和时间。所有 Cargo 阶段结束，其他 PG 连接与 schema 为零，两个 Linux holder 和 PG 都已停止。用户 5433 未访问；缓存与只读快照留在 ignored `run/validation-20261011-skills/`。

## 恢复步骤

1. 读取 [恢复检查点](../../resume-checkpoint-skills-2026-10-11.md)、[checklist](../../checklists/skills-2026-10-11.md) 和另外三份 HANDOFF；检查 Git branch/status/log。
2. 本轮没有待恢复作业、未完成实现或隐藏测试。不得重跑旧失败版本来替代最终 V2；新变更冻结为新 manifest。
3. 下一项软件任务：独立审计静态 classifier 的 skill 发现与 Registry live snapshot 的衔接。读取 `routing/service.rs` 的两份静态模板、`classification_prompt_for_mode` / `classify` / `to_decision`，以及 `routing/mod.rs::RouterInput`、`generation.rs::route_response`，检查新增／禁用／删除 skill 的候选和退化，以及返回技能名的允许范围。先查已有实现再补缺口，保持全局工具可见性与 Execution 批准；本轮未实施。
4. 继续三个互斥 subagent，Root 统一运行和正常提交。原 V01 与 R02/R03/R05/R06 保持开放，前端及未来端上部署按台账执行。

同步仅保证当前 Registry 的成功 HTTP CRUD；外部 SQL/其他实例仍需显式 reload。请求数组元素与 nullable 语义保持，静态路由自动发现未改。默认 Shadow/当前政策保持，无新部署/硬件授权。未推送或建 PR；本轮未使用 reset 卡。

未运行 Linux 全 workspace、release/native release link、真实 PTY、部署 smoke、Skills 链路的真实网络 COMMIT ACK 丢失注入、真实模型或硬件验证。本轮完整默认测试包含 native 路径，但不替代上述未运行项目；上轮已有证据只作为历史记录。
