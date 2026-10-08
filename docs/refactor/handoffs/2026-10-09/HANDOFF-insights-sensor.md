# HANDOFF — Insights、Memory maintenance 与 Sensor 分层（2026-10-09）

负责人：`/root/insights_sensor_1009`。读取基线：`b9ff6e0`，分支 `codex/architecture-refactor`。本记录区分本代理实现及静态核对与 Root 的统一运行验收；Cargo、数据库、macOS / Linux 最终结果见末尾。

本代理实现 checklist **8/8**；运行验证 **2/2**（Root 已完成）。

- [x] I01 读取 Insights 全调用链、禁用工厂与 lifecycle、缓存审核、Memory 授权和 Sensor 实现。
- [x] I02 SensorRegistry 移入 `modules/platform/sensors`；纯值、freshness policy、SensorReadProvider 与 config 独立于 registry 持久化实现。
- [x] I03 单一具体 `InsightService` 与 `InsightTask / InsightPhase / InsightStatus` 归 `modules/platform/insights`；旧 providers 目录删除，公开方法为固有方法。
- [x] I04 Memory 提取、整固、反思、习惯推导移入 `modules/memory/maintenance`，Memory 自己持有模型输入、策略复验与 guarded batch。
- [x] I05 Cache review 移入 `modules/interaction/cache/review.rs`；保留调用前授权、revision CAS、已确认写入与所属 embedding future。
- [x] I06 原模型 JSON 解析与模型标签聚合复用至 `providers/llm/response.rs`；没有新增解析策略或模型调用。
- [x] I07 `materialize_memory_capture` 改为显式数据库参数的关联函数，删除维护用例中的四处具体 `NoopEmbedding` 构造；完整事务体保留。
- [x] I08 原 Insights 13 项测试保留并归属相应模块；增加禁用与 feature 准入检查，独占文件 rustfmt 和 diff whitespace 检查通过。
- [x] IV01 Root 在默认 feature 下验证编译、Insights / Cache / Memory / Sensor 相关运行测试与工作区回归。
- [x] IV02 Root 验证无默认 feature 构建及 `availability_tests`，记录最终冻结输入、日志与结果。

## 新归属与集成 API

| 归属 | 实际职责 | 依赖和出口 |
| --- | --- | --- |
| `modules/platform/sensors` | 定义、freshness、查询、pull provider 注册、持久化、owned cleanup | 原类型名称保持；从本模块导入，registry 不再是类型出口 |
| `modules/platform/insights` | 日程、事件路由、账户准入、任务统计、30 秒 phase 截止、日志确认、owned worker 关闭 | `InsightService::new(enabled: bool) -> anyhow::Result<Self>`；固有 `schedule/status/trigger/get_conditions/set_conditions`；`ModuleLifecycle` 保留 |
| `modules/memory/maintenance` | extraction / consolidation / reflection / habit 模型业务与原子存储 | crate 内输入 `MemoryMaintenanceUser`、结果 `MemoryMaintenanceStats`、四个业务函数；不引用 Platform Insights / Channel / ModuleRegistry / AppState |
| `modules/interaction/cache/review` | review 用例与模型准入 / CAS / embedding 请求所属关系 | crate 内 `CacheReviewRuntime`、`review_pending_cache_entries`，依赖真实 `CacheStore` port |
| `providers/llm/response` | 已有 structured model reply 解析、模型标签合并 | crate 内 `parse_llm_json_object`、`join_model_labels`；三处真实业务复用 |
| `ContextService::materialize_memory_capture` | 获取并冻结 canonical source rows | 首参 `&DatabaseConnection`，不需要构造 ContextService / embedding adapter |

Root 负责 parent exports、旧 adapter 契约与 `create_insights` 工厂删除、bootstrap registry/builder、API、外部测试调用者的迁移。本代理没有更改共享装配文件。

## 禁用和 feature 行为对照

| 情况 | 迁移后行为 | 对照原实现 |
| --- | --- | --- |
| constructor `enabled=false` | 创建惰性 InsightService；`name="noop"`，health Healthy | 原 NoopInsights 行为 |
| 惰性服务 init/start/stop | 不附加 DB、模型或 scheduler，不创建 worker，重复 start/stop 成功 | 原 lifecycle 默认 no-op |
| 惰性 schedule/trigger | 返回成功，无 phase/账户/DB 操作 | 包括无效 phase，保留原 Noop 行为 |
| 惰性 status | enabled=false、无 last phase / time、pending=0 | 原 Noop status |
| 惰性条件 / hot settings | get_conditions 空，set_conditions 不保存，apply_settings 无消费键 | 原 Noop 行为 |
| constructor `enabled=true` 且无 `insights` feature | 原错误 `enabled insights require feature 'insights'` | 保留原工厂错误 |
| enabled 服务未初始化 / 无效 phase / stopping | 保留原错误传播 | 未转为 no-op 或吞错 |
| enabled 服务 init 后政策 disabled | 原 Degraded health 与 disabled phase 错误保留 | 此情况与 constructor-disabled 的惰性状态原本不同 |
| enabled 服务 hot settings | 只更新既有四个 prompt 键，不热启用、不开新 worker | 保留原生命周期实现 |

服务入口始终可编译；`insights` feature 当前控制启用准入，其具体服务体不再通过另一份 Noop 实现做条件替换。该 feature 原无依赖项；没有新增 feature 或库。

## 可核对的静态证据

`run/validation-20261009-layering/insights-sensor-static.json` 的四项源码比较均为 true。比较以 `git show b9ff6e0:...` 为原件，仅忽略格式并按列出的迁移替换名称；不是运行结果：

1. Memory 用例体除命名空间、自由函数调用、静态 capture 调用外保持。
2. Cache review 的模型前授权、CAS 和所属 embedding 请求体保持。
3. Context capture 的事务级别、statement timeout、账户锁、政策、source revisions、payload、提交未知错误体保持。
4. SensorRegistry 实现体保持，仅类型移至相邻 owner。

原 Insights 的 **13/13** 个测试函数均保留：原子多修改/新建、后序非法目标整批不落库、retirement 保留证据、clear 后拒绝旧候选、cache 已确认审批在挂起 embedding 超时后不重放、permit 释放、取消 future、cron 部分失败清理、解析和模型名记录等。新增默认 feature 两项测试；无 feature 另有准入拒绝测试。以上为本代理静态核对范围；实际执行结果见末尾 Root 验收。

## 未扩大范围

未更改默认 Shadow、政策内容、表结构、模型实现、依赖、硬件适配或任何实机动作；本代理未启动 Cargo、PostgreSQL 或 Docker；本轮未读取用户 5433 数据库。Memory transaction ownership 与 ModuleContext 广泛初始化依赖仍按后续 L10 / L11 清单处理。运行结论以 Root 最终冻结源码的证据为准。

## Root 整合后的只读复核

核对了 constructor → bootstrap initialize → settings/health → start/stop 的完整接入：Insights 仍登记在 `ShutdownPhase::Producers`，start 仍在 Scheduler 启动之前注册 cron，关闭保留先停止 owned workers 再清理 scheduler task 的顺序。ModuleContext 中 canonical `Arc<CacheService>` 只在 review 边界 map 为 `Arc<dyn CacheStore>`，共享同一个服务实例。Memory 仍按 source snapshot → policy → canonical capture → bind/validate → model → guarded atomic batch 的顺序执行。

复核发现的两处测试整合遗漏已报告 Root，随后只读确认已修正：`tests/cache_context.rs` 的构造改为 `InsightService::new(true).unwrap()`，并从新 owner 显式导入 `InsightPhase`。服务源码与测试中未再发现旧 Insight/Memory/Cache/Sensor provider 路径或旧契约引用；guard 的旧符号否定 fixture 属于预期。

未见需要改变政策、扩大硬件范围或增加兼容 facade 的问题。本复核没有执行 Cargo / 数据库，也没有修改冻结候选源码；最终运行证据仍由 Root 记录。

## Root 最终验收

源码 `ffd7af19dccb8db28f16c5091e09265d4c5bbd32`；V1 `2a06adad2d01d3faee0d369576a907af61d23918f18e5d956a05556dcf0923e5`，**637/637** 输入匹配 Git blob。macOS workspace **1056/0/5**、真实 PTY **3/0**；无默认 Insights availability **2/0**；Linux ARM64 Server 全包 **774/0/5**、默认 release/native link 与无默认检查通过。macOS 无默认 all-targets、fmt 和边界正负自检通过。

本轮 **10/10**，L09—L12 **1/4**，六轮 **2/6**。实际命令、计数和限制见 [最终验证](../../layering-2026-10-09.md#最终验证与证据) 与 [证据索引](evidence-index.json)。本轮任务资源已停止，三个 subagent 已交回，没有待接回的 Cargo/PG 作业；下一步 L10。两项真实模型未重跑，Linux 非全工作区及独立部署 smoke，不声称硬件验收。
