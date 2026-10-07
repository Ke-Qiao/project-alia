# Boundary / Bootstrap / Platform handoff — 2026-10-08

基线 `adb28c1`，分支 `codex/architecture-refactor`。本文件记录 Boundary subagent
负责的目录归属、依赖检查和当前架构文档；全项目验收以 Root 的最终证据与
[本轮 checklist](../../checklists/layering-2026-10-08.md) 为准。

## 本分工进度：6 / 6

- [x] **B01 装配归属**：`module/{builder,factory,registry}` → `bootstrap`；
  `module/{background,lifecycle}` → `modules/platform`；删除旧 `module` 目录和门面。
- [x] **B02 服务接入**：Builder 直接构造具体 `MemoryService`，删除单实现
  `create_memory` 工厂；Registry / ModuleContext 持有 `Arc<MemoryService>`。
  Builder 显式构造 17 项 `InteractionServices`，接 `spawn_interaction`，
  `interaction_policies`、`interaction_task` 使用新名称。
- [x] **B03 生命周期与装配收尾**：Easy-Turn 构造移至
  `bootstrap::factory::create_turn_detector`；移除 Registry 中重复的
  `memory_capture_llm.apply_settings`；panic cleanup 子进程 `--exact` 使用新模块路径。
  `bootstrap/runtime.rs` 的 lifecycle import 已更新。
- [x] **B04 边界检查工具**：新增标准库 Python 脚本并通过自测，无依赖变更。
- [x] **B05 当前文档**：更新 Server Architecture、Modules and Providers、跨包
  Architecture Overview；明确记录尚未迁移的固定核心与事务耦合。
- [x] **B06 集成验证**：Root 已完成最终边界扫描、Rust 编译与回归，实际结果见末尾；
  subagent 未运行 Cargo、数据库、服务或硬件动作，也未独立提交。

## 关键行为

- 未更改默认 Shadow、持久政策、配置选型、源身份、Context/Response/Delivery 契约。
- 生命周期迁移保留启动前注册 rollback、依赖顺序关闭、取消 waiter 后继续由 owner 清理、
  native/panic 未证实停止时保留资源的实现。
- `ModuleRegistry` 位于 Bootstrap，避免把 Platform 变成全局 service locator。
  Interaction 使用明确服务集合，不依赖整个 registry 或进程 cleanup owner。
- `ModuleContext` 仍是初始化专用的宽依赖包；此轮没有为消除所有 Rust 模块循环添加新 trait。

## 可执行检查

```sh
python3 scripts/check-server-boundaries.py --self-test
python3 scripts/check-server-boundaries.py
```

第一条已于本 subagent 执行，输出 `Server boundary checker self-test passed`。
第二条已由 Root 在消费者迁移完成后执行并冻结通过结果。检查器验证：

- 旧 `module`、`providers/memory`、`channels/pipeline` 与顶层 `memory_policy.rs` 不再存在；
- Rust source/tests/examples 不再引用退休路径与 Memory backend /旧 Pipeline 入口；
- 新 owner 文件存在；Memory 类型/trait 不重新声明或 re-export 到 providers 门面；
- Memory / Interaction 不直接使用聚合 Registry、AppState、Bootstrap 或具体 adapters；
- Memory capture 不依赖 Channels、`PendingTextPersistence` 或 `InteractionServices`；
- Memory 纯域类型不依赖 SeaORM、Tonic、provider/adapter 实现或 entity。

这是有界源码守卫，不是 Rust 解析器或完整语义证明；Cargo 和行为回归仍必须单独运行。
未将输出字数或模块行数当作架构正确性的证据。

## 已记录后续事项

1. `providers/{conversation,cache,persona,router,skills,insights,sensor}` 仍是固定核心实现；
   后续按实际职责迁移，不为文件移动新建无实现价值的 trait 或进程。
2. `providers/adapter.rs` 仍聚合非 Memory 的跨域 contract；应随对应 domain 迁移拆分。
3. Context / Companion / Delivery 现有跨表校验与事务协作仍相互引用；独立评估所有权，
   不拆开本来必须原子提交的事务。
4. Delivery 仍引用 channel transport contract 与 TTS audio budget；后续审查预算归属，
   本轮不改变真实投递证据和 native 生命周期。
5. 更细初始化依赖可后续结合真实消费者缩窄；当前不声称整个架构提案全部落地。

## Root 最终验证补记

- 最终边界扫描：已完成，见末尾最终验收及证据索引。
- macOS 默认 / 无默认 feature 编译和回归：已完成，见末尾最终验收及证据索引。
- 当前 Linux ARM64 验证：已完成，见末尾最终验收及证据索引。
- 源码提交 / 证据清单：已完成，见末尾最终验收及证据索引。


## 独立静态审查补记（V3 冻结后）

只读比对基线和迁移后源码，未在冻结后修改构建输入，也未运行 Cargo、数据库或服务：

- Memory 的原 25 项异步操作全部保留；`explain` / `explain_suppressed` 继承默认实现去空白后完全一致。
  原具体存储、查询、privacy/source guard 和事务体保持；DTO/serde 字段、默认值和检索投影构造保持。
  删除的 `MemoryCapabilities::unavailable` 仅服务于退休的 Noop 实现。
- Lifecycle 除 Memory 类型、导入和 panic 子进程测试名外无控制逻辑变化；OwnedBackground 和
  Bootstrap runtime 在路径规范化后与基线相同。原 owner 校验、持久政策优先级、init/start 注册与
  shutdown phase 顺序保留，Interaction 接收原服务的 Arc 克隆。
- Interaction 的缓存筛选/写入 helper、Context materialize/merge/validate、模型执行与流式开关
  保持原行为。精确/语义缓存完成路径保留来源、cache_hit/route/used_tools 和语义相似度；
  minor 有升级提示时仍禁止流式，Response prepare 仍先冻结原 Context 再绑定原输出 owner。
- 独立审查确实发现新缓存后取消出口漏记中断终态，已向 Root 提交复现条件。
  当前 V3 源码在此出口调用 `persist_interrupted_input_context` 并记录正常终端审计，保留已有
  `cache_lookup` / `semantic_cache_candidates` stage，只为尚无 stage 的取消补 `after_cache_lookup`。
  修复前失败与修复后运行结果由 Root 单独归档；这里不替代实际回归证据。

已复核本轮技术说明、checklist 和四份 HANDOFF。L09–L12 对剩余边界的描述准确；
本轮迁移完成不等于固定核心、初始化依赖、跨表事务或出站契约全部完成。
基线 Memory 已是固定 built-in，未提供配置 override 槽；此次删除的是 provider 目录和类型归属、
`MemoryBackend` 抽象、Noop 与单实现工厂，不应将其描述成取消一个原本可配置的 Memory 选型。

本分工 B06 已由 Root 在最终 macOS / PTY / no-default / Linux 证据齐备后关闭。

## Root 最终验收

本轮 **8/8**。最终源码 `978af9c942da42aa3cd98182cde3dafa6a6e33be`，V4 `ccfaa7981fc7062dfeb43e568e52af038915064a2187352e0891980afc6fcbd3`，630/630 输入匹配。macOS workspace **1053/0/5**、真实 PTY **3/0**、Server 无默认全部目标通过；Linux ARM64 Server 全包 **771/0/5**、Server 无默认全部目标和默认 release/native link 通过。架构守卫、自测、格式与差异检查通过。

实际命令与逐目标计数见 [最终验证表](../../layering-2026-10-08.md#最终验证与证据)、[证据索引](evidence-index.json)。两项真实模型未重跑；不声称 Linux 全工作区或硬件验收。V3 Linux 原失败与定向复核保持原样，V4 仅修测试夹具。三个 subagent 已交回；两个 Linux holder 和独立 PG 已停止，用户 5433 未访问。没有待接回的 Cargo/PG 作业。下一步从 [L09（0/4）](../../checklists/layering-next.md) 开始。
