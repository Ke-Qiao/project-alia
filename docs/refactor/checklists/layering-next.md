# 后续分层 checklist：L09—L12

更新：2026-10-10。承接 [本轮 L01—L08](../layering-2026-10-08.md)，当前 **4 / 4 完成**。这是剩余架构工作的续接清单，不计入 R01—R06，也不代表整个项目的完成百分比。

- [x] **L09 固定核心与契约归属**：逐项检查实际调用者、trait 默认实现、持久数据与生命周期所有者；迁移实现时一起迁移核心契约，删除旧入口、单实现工厂和无替换需求的抽象。不能只重命名文件。
- [x] **L10 初始化依赖**：删除 `ModuleContext`，由 Bootstrap 绑定实际依赖，生命周期 init 无参数；默认/无默认、失败回滚和平台验证通过，见 [本轮 10/10](l10-2026-10-10.md)。
- [x] **L11 跨域事务**：ResponseCommitter 统一 Response/history；Companion 拥有 source/grant/policy，Context 保留私有租约和证据适配，必要原子性保持；见 [本轮 10/10](l11-2026-10-10.md)。
- [x] **L12 出站契约**：Delivery 拥有 transport 和共享 audio budget，旧 Channel/TTS 出口删除；保留原实例、精确取消、限额和 native 清理，见 [本轮 10/10](l12-2026-10-10.md)。

## L09 已完成的执行顺序与验收条件

L09 已按真实调用链完成并验收，见 [本轮 10/10 checklist](l09-2026-10-09.md) 与 [最终验证](../layering-2026-10-09.md#最终验证与证据)。下表保留执行时的核对重点，L10/L11/L12 均已验收，下一项软件任务为 Skills HTTP CRUD reload 缺口。

| 顺序 | 当前固定实现 | 下一步与验收重点 |
| --- | --- | --- |
| 1 | Conversation | 归 Interaction 的会话/消息/事件职责；保留来源双 epoch、Response 与 assistant history 同事务提交、重试消息 ID 稳定。删除 trait 前逐项核对默认方法。 |
| 2 | Cache | 实现、版本和审查契约一起归 Interaction；保持 owner/scope 隔离、Context 来源、revision CAS、到期与擦除撤销。复用本轮 exact/semantic 命中及取消回归。 |
| 3 | Router | 固定路由归 Interaction，只做决策；工具仍经 Execution、输出仍经 Delivery，保留能力筛选和 minor→major 升级。 |
| 4 | Skills | 注册/解析归 Interaction；保持加载、卸载、热更新，preferred tools 仅贡献排序偏好，不能绕过工具可见性与执行批准。 |
| 5 | Persona | 先区分人格资源、提示渲染与 Companion 持久关系；保留名称清理、资源更新及提示行为，不能整体搬到 Companion。 |
| 6 | Insights | 记忆提取/整固归 Memory，缓存审查归 Interaction；调度和 owned worker 关闭保持明确责任。验证模型前授权、模型后事务复验、原子批次及禁用/未编译行为。 |
| 7 | Sensor | 核对固定注册/存储/查询/清理的 Platform 归属；实际采集保留必要适配契约。验证 owner、freshness、时间戳和 quality，不启动硬件。 |
| 8 | 总体验收 | 按已完成迁移收紧架构守卫和负例；当前源码真实数据库回归、默认/无默认检查，更新四份 HANDOFF、冻结身份和提交。 |

三个 subagent 分别承担互不覆盖的职责包，Root 负责跨包契约与唯一 Cargo/PostgreSQL 验证窗口。相互依赖的迁移先明确接口再改调用者。每完成一项记录真实结果；跨域事务已按 L11 运行验收，L12 亦已按真实契约和关闭证据完成。

继续保持默认 Shadow、当前政策、无新依赖/平台/模型/硬件动作、用户 5433 不访问。前端、真实 AEC、执行器反馈、V01、Pi 整机与其它平台仍见 [延期台账](../deferred.md) 和 [六轮 checklist](next-round.md)。

L12 已完成；Linux 当前 hash 启动瓶颈已测量修复，原 20 秒期限/并发保持，历史证据边界继续记录。下一项软件任务：核对 `packages/alia-server/src/api/skills.rs` 中创建/更新/删除后的三个 reload TODO 与 `modules/interaction/skills` 的实际索引更新路径；先检查既有实现，再补缺口和真实整合回归。保持工具可见性、批准、加载/卸载/热更新和当前政策；本轮未实施该任务。 恢复见 [L12 检查点](../resume-checkpoint-l12-2026-10-10.md)。
