# Memory 分层交接（2026-10-08）

基线：`adb28c1`，分支 `codex/architecture-refactor`。本文件记录 Memory 子任务；本轮整体结果、最终提交与正式验证以 root 的 checklist/交付记录为准。

## 本轮完成范围

- [x] 原 `providers/memory` 整个目录迁入 `modules/memory`；未保留旧路径、兼容 re-export 或 `AliaMemory` 别名。
- [x] 从 `providers/adapter.rs` 移出完整 Memory 段（原 885 行）。规范 Memory/Graph/Retrieval 类型、检索投影构造函数统一由 `modules::memory` 导出。
- [x] 删除 `MemoryBackend` trait 与仅测试使用的 `NoopMemory`；唯一真实实现改为具体 `MemoryService`，移除固定核心记忆的 provider 形态残留（基线已禁止配置替换 Memory）。
- [x] 原根层 `memory_policy.rs` 移入 `modules/memory/policy.rs`。
- [x] 原 Pipeline 的自动记忆提取移入 `modules/memory/capture.rs`，以 Memory 自有 `CaptureTurn` 输入替代 `PendingTextPersistence`；显式接收 Context、Memory 与 LLM 依赖，不依赖 ModuleRegistry、Interaction 或 Channels。
- [x] Context 的规范记忆读取、向量查询、记忆来源绑定全部调用 Memory 内部实现，不回绕到 providers。
- [x] 旧 trait 共 25 个异步操作：原真实实现覆盖 23 个；此前实际继承的 `explain`、`explain_suppressed` 两个查询实现已逐项转为固有方法。新具体服务共覆盖原 25/25，无遗漏。
- [x] 静态路径扫描和本子任务文件的独立 `rustfmt --edition 2024 --config skip_children=true` 通过。
- [x] 根代理完成整合编译、Memory/Context/PostgreSQL 与完整服务回归，并写入本轮统一证据。

进度：Memory 实施与静态复核 **8/8**；最终自动化回归 **已由 Root 统一完成**。此计数不替代整体重构进度，也不表示物理设备或产品体验已验收。

## 当前结构与契约

- `modules/memory/types.rs`：规范记忆值、来源写入快照、批处理、检索/图/说明结果；仅依赖 Rust std、serde、serde_json、chrono、uuid，没有 SeaORM entity、数据库连接、Tonic/Proto、外部模型 SDK 或 provider 类型。
- `modules/memory/service.rs`：真实 MemoryService 和 25 项操作；保留原存储、隐私撤销、来源验证、合并/整固、擦除与投影逻辑。
- `modules/memory/service/{context_read,embedding,embedding_worker}.rs`：内部数据库读取与向量投影；不暴露为第二套公开 Memory API。
- `modules/memory/privacy.rs`：同事务隐私锁、策略和来源撤销判断。
- `modules/memory/policy.rs`：现有敏感内容、规范化与合并规则。未调整策略值或匹配规则。
- `modules/memory/capture.rs`：自动记忆提取用例。LLM 是实际需要的外部能力，继续通过已有 `LlmAdapter`/`ChatMessage` 契约调用；没有新增服务工厂、抽象 trait 或依赖。
- `modules/memory/context`、`source`：保留既有持久上下文租约与来源能力。

`CaptureTurn` 含 `context: ContextBinding`、`source_privacy_epoch: i64`、`session_id/user_id: Uuid` 和 `user_id_text/query_text/response_text/channel_id: String`。它不携带缓存写入、Response 状态或 Pipeline 运行对象。

生产提取入口：

```rust
capture_turn_memory(
    context: &ContextService,
    llm: &dyn LlmAdapter,
    memory: &MemoryService,
    turn: &CaptureTurn,
    config: &MemorySection,
)
```

生产入口保持先复验 Context、读取持久有效策略，再进行提取；提取仍在模型调用前获取写快照并验证原始来源，实际写入再次在事务中校验。测试用显式依赖入口为 `capture_turn_memory_with_model(llm, memory, turn, config)`，保留提取/撤销竞态测试的既有覆盖路径；它不恢复任何旧入口名称。

## 已复核的行为保持

原 trait 的未覆盖默认实现只有 `explain` 和 `explain_suppressed`；两者原逻辑已进入具体服务。其它默认 capability、逐个删除、无 guard 降级等实现原本不用于真实 MemoryService，已随 trait 删除。实际生产路径继续使用原具体实现的事务性 `forget_many_guarded`、`apply_batch_guarded` 与 `erase_matching`。

内存 fixture 仍是显式 `MemoryService::new()`，持久运行通过原生命周期初始化数据库和投影 worker；配置过持久层后不会静默回退到内存。没有新增禁用 Memory provider；禁用记忆访问仍由现有持久策略控制，擦除与管理保持可用。

根代理负责外部调用方、测试 imports、`Arc<MemoryService>` 和构造调用迁移；Interaction 子代理负责 Pipeline 内部 fixtures 与 `CaptureTurn` 映射；Bootstrap 子代理负责 registry/lifecycle/builder。这份交接不单方面声称其它所有权范围已完成验证。

## 本轮未扩大范围

未运行 Cargo、PostgreSQL、服务、真实模型或硬件；root 保有唯一验证窗口。没有修改依赖、数据库迁移、默认 Shadow、用户本地配置、当前策略阈值或设备动作权限。

保留现有 Context 对跨领域来源/撤销的一致事务协调；没有把 Companion/Delivery 的事务拆成分别提交。更大范围的 Context/Companion 持久化归属、其它 providers 中的核心领域与整个 provider 契约文件进一步拆分，需要单独按真实调用链评估，不能把本轮 Memory 归位写成全仓所有分层已经完成。

## Root 已确认的整合结果

源码 `978af9c`（实现 `08c54be` 加测试夹具修正）、最终 V4 输入 630 项已逐项对应提交。macOS 默认工作区 **1053 passed / 0 failed / 5 ignored**；真实 PTY **3 passed / 0 failed**；Server 无默认 feature all-targets、格式与架构边界自测/扫描均通过。两项新增缓存回归已包含在默认通过计数中。Linux 最终源码验证已完成，详见下方 Root 最终验收。此前 subagent 的未验证状态仅表示当时分工交接时点。

## Root 最终验收

本轮 **8/8**。最终源码 `978af9c942da42aa3cd98182cde3dafa6a6e33be`，V4 `ccfaa7981fc7062dfeb43e568e52af038915064a2187352e0891980afc6fcbd3`，630/630 输入匹配。macOS workspace **1053/0/5**、真实 PTY **3/0**、Server 无默认全部目标通过；Linux ARM64 Server 全包 **771/0/5**、Server 无默认全部目标和默认 release/native link 通过。架构守卫、自测、格式与差异检查通过。

实际命令与逐目标计数见 [最终验证表](../../layering-2026-10-08.md#最终验证与证据)、[证据索引](evidence-index.json)。两项真实模型未重跑；不声称 Linux 全工作区或硬件验收。V3 Linux 原失败与定向复核保持原样，V4 仅修测试夹具。三个 subagent 已交回；两个 Linux holder 和独立 PG 已停止，用户 5433 未访问。没有待接回的 Cargo/PG 作业。下一步从 [L09（0/4）](../../checklists/layering-next.md) 开始。
