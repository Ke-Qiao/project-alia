# Conversation / Cache 领域归属交接（2026-10-09）

基线：`b9ff6e04075ae16bbc23ba6f904610d82f973c5b`，分支 `codex/architecture-refactor`。本文件仅记录 Conversation / Cache 子任务；最终构建、数据库回归、提交和整轮进度由 Root 统一验收。

## Checklist 与进度

- [x] 追踪原 `providers/conversation`、`providers/cache` 的全部实现、共享 adapter 契约、调用者、故障注入与 PostgreSQL 事务边界。
- [x] `ConversationService` 迁入 `modules/interaction/conversation/service.rs`；原来源持久化回归随服务迁移。
- [x] `ConversationStore` 与 `ConversationEventEntry` 归 `modules::interaction::conversation`；9 项方法全部显式要求实现，删除原 `append_event` 默认静默成功及两项默认不支持回退。
- [x] `CacheService` 迁入 `modules/interaction/cache/store.rs`，Cache 类型与 `CacheStore` 归同领域 `types.rs`。
- [x] 两个域端口仅要求 `Send + Sync`；具体服务独立实现 `ModuleLifecycle`。不向消费方提供启动、停止等装配权限。
- [x] 删除原 Conversation / Cache provider 目录；不提供旧路径兼容别名、单产品工厂或转发包裹层。
- [x] 8 项基线静态比对全部通过；所属文件 `rustfmt --check` 与 `git diff --check` 通过。
- [x] Root 完成共享 exports、全仓消费者及 fixture 整合，已运行统一编译、数据库与服务回归，结果见末尾。

子任务实现与静态交接 **7/7**；整合验收 **1/1，已由 Root 完成**。静态比对 **8 passed / 0 failed**，不能替代运行测试。

## 新 API 与文件归属

`modules::interaction::conversation` 导出：

- `ConversationService`：原 DatabaseConversationBackend 的真实实现。
- `ConversationStore`：现有 Interaction 故障注入、Companion、CLI 使用的领域端口。
- `ConversationEventEntry`：事件输入值。

`modules::interaction::cache` 导出：

- `CacheService`、`CacheStore`。
- `CacheVersion`、`CacheStatus`、`CachedResponse`、`SemanticCacheCandidate`、`CacheEntry`、`CacheReview`、`CacheWork`、`CacheCas`。
- `DEFAULT_CACHE_SCOPE`、`CACHE_METADATA_SCHEMA`、`response_digest`。
- 原有缓存准入、语义候选与重用策略 helper 保留原行为。

`cache.rs` 是领域入口及现有策略实现，`cache/types.rs` 定义数据和域端口，`cache/store.rs` 实现事务持久化。没有额外包装或重复服务 API。`cache/review.rs` 由 Insights 子代理负责，未占用其修改范围。

Root 负责从 `providers/adapter.rs` 删除原契约、从 `providers/mod.rs` 删除旧导出、注册 `interaction::conversation`，迁移 `ModuleRegistry` / `ModuleContext` / `InteractionServices` 与全仓外部使用者。生产注册表使用具体服务，真实需要可控故障的消费边界保留域端口。

## 已核对的行为与保留边界

对比基线，除导入、服务名与端口名替换和 rustfmt 外，两个真实服务完整实现保持一致。Conversation 的原始来源时间、owner、privacy / relationship 双 epoch，规范 Response 与助手 history 同事务提交、重试不复活已删除历史，均未改变。Cache 的 Context 授权、私有 owner / scope、摘要验证、版本 CAS、review / embedding 前复验、向量 profile 校验、过期清理锁顺序及同事务撤销均保留。

原生命周期健康键字符串 `database_conversation`、`db` 保留，避免改变运行审计字段；它们不是旧类型别名或可替换 provider 入口。缓存 metadata 的历史 `created_by: "pipeline"` 同样保持既有事实，不恢复 Pipeline。

Cache 仍调用 `providers::embedding::profile_store::ensure_profile` 这一现有共享向量 profile 持久化 helper；契约使用现有 `EmbeddingProfile` / `EmbeddingVector`。该复用未引入新依赖、具体模型 adapter 或循环服务定位器。跨领域 Context / Delivery 事务按既有授权保留，归属进一步调整需按 L11 单独维护原子性。

## 实际静态检查

`run/layering-20261009-static/conversation-cache.json` 记录以下 8 项比对（由 Python 标准库读取 `git show b9ff6e0:<path>`，与新文件移除格式差异后比较）：

1. Conversation 完整实现仅类型名和 imports 改变。
2. Cache 完整实现仅类型名和 imports 改变。
3. Conversation 来源持久化回归测试保留。
4. Cache 原有策略 helper 保留。
5. Cache 全部值类型和方法契约保留，仅替换域端口名并移除 Lifecycle 父 trait。
6. Conversation 9 个 async 方法均为必需声明，没有默认方法体。
7. 两域端口均为 `Send + Sync`，不继承生命周期。
8. 两个原 provider 目录已不存在。

可执行检查：

```sh
rustfmt --edition 2024 --check \
  packages/alia-server/src/modules/interaction/conversation/mod.rs \
  packages/alia-server/src/modules/interaction/cache.rs

git diff --check -- \
  packages/alia-server/src/providers/conversation \
  packages/alia-server/src/providers/cache \
  packages/alia-server/src/modules/interaction/conversation \
  packages/alia-server/src/modules/interaction/cache.rs \
  packages/alia-server/src/modules/interaction/cache
```

## 代理边界与 Root 验收

本子代理没有运行 Cargo、PostgreSQL、容器、服务、模型或硬件；Root 独占验证窗口。未修改依赖、schema migrations、策略、默认 Shadow、私有配置及用户数据库。

Root 已完成全目标编译，以及现有 durable input provenance、规范 Response commit / retry、Cache Context / CAS / 撤销和 cache cancellation 回归。实际结果见末尾及同轮证据，不使用上一轮通过数。

## Root 最终验收

源码 `ffd7af19dccb8db28f16c5091e09265d4c5bbd32`；V1 `2a06adad2d01d3faee0d369576a907af61d23918f18e5d956a05556dcf0923e5`，**637/637** 输入匹配 Git blob。macOS workspace **1056/0/5**、真实 PTY **3/0**；无默认 Insights availability **2/0**；Linux ARM64 Server 全包 **774/0/5**、默认 release/native link 与无默认检查通过。macOS 无默认 all-targets、fmt 和边界正负自检通过。

本轮 **10/10**，L09—L12 **1/4**，六轮 **2/6**。实际命令、计数和限制见 [最终验证](../../layering-2026-10-09.md#最终验证与证据) 与 [证据索引](evidence-index.json)。本轮任务资源已停止，三个 subagent 已交回，没有待接回的 Cargo/PG 作业；下一步 L10。两项真实模型未重跑，Linux 非全工作区及独立部署 smoke，不声称硬件验收。
