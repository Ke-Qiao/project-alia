# Routing / Persona / Skills handoff — 2026-10-09

基线 `b9ff6e0`，分支 `codex/architecture-refactor`。本分工负责 L09 中三个固定核心的真实归属；Root 负责聚合契约删除、Bootstrap 装配、全仓消费者和最终验证。

## 分工 checklist：8 / 8

- [x] **RPS01 调用面与默认行为**：检查生产装配、Interaction generation/tools/prompt、API、内外部测试，确认 Persona/Skills 单实现，以及 DirectRouter 仅用于测试。
- [x] **RPS02 Persona 归属**：迁入 `modules/interaction/persona`，具体 `PersonaService`；两个原 trait 方法及继承的 `render_system_prompt` 成为固有方法。
- [x] **RPS03 Skills 归属**：迁入 `modules/interaction/skills`，具体 `SkillRegistry`；五个原 trait 方法成为固有方法，原 SQL、插件注册/卸载、依赖检查、索引热更新保持。
- [x] **RPS04 Routing 归属**：迁入 `modules/interaction/routing`，具体 `RouterService`；移除 PassthroughRouter、DirectRouter，不保留 RouterStrategy 替换槽或旧路径 alias。
- [x] **RPS05 路由构造与回归**：构造显式接收现有 `RouterConfig` 和已初始化的 minor LLM，保留初始校验、热更新原子校验、minor/major 升级与降级；增加一个可执行回归测试。
- [x] **RPS06 静态比对与交接**：独占文件 rustfmt 成功；与基线逐段比较，Persona/Skills 业务体未改，Router 分类、候选过滤、决策和 fallback 原实现保留；本文件记录边界与后续项。
- [x] **RPS07 Root 最终验证**：Root 已完成全仓链接、macOS/Linux 测试和冻结证据，见末尾。Subagent 未执行 Cargo、数据库或服务。
- [x] **RPS08 L09 边界守卫**：扩充 `scripts/check-server-boundaries.py`，现时全仓检查与正/负向自检通过，原始日志与结果 JSON 保存至本轮 run 目录。

## API 与路径迁移

| 原归属 | 新归属 |
|---|---|
| `providers/persona/template.rs::TemplatePersona` | `modules::interaction::persona::PersonaService`（实现 `persona/service.rs`） |
| `providers/skills/builtin.rs::BuiltinSkills` | `modules::interaction::skills::SkillRegistry`（实现 `skills/registry.rs`） |
| `providers/skills/builtin.rs` 公共 LoadedSkill / seed / preferred-tool 函数 | `modules::interaction::skills::registry::*` |
| `providers/skills/markdown.rs` | `modules::interaction::skills::markdown` |
| `providers/router/multi_layer.rs::MultiLayerRouter` | `modules::interaction::routing::RouterService`（实现 `routing/service.rs`） |
| RouterConfig / Classification / LlmMode / parse_tool_selection_response / strip_code_fences | `modules::interaction::routing::*` 根导出 |
| Persona / RenderedPersonaPrompt / DEFAULT_ASSISTANT_NAME / sanitize_assistant_name | `modules::interaction::persona::*` |
| SkillMatch / SkillResolution / SkillInfo / PluginSkillDefinition | `modules::interaction::skills::*` |
| RouterInput / RouteDecision / ToolCandidateInfo / ToolSelectionInput | `modules::interaction::routing::*` |

Root 从 `providers/adapter.rs` 删除这些声明和三个旧聚合 trait；没有迁回 providers 的重导出。

`RouterService::new(config: RouterConfig, minor_llm: Option<Arc<dyn LlmAdapter>>) -> anyhow::Result<Self>` 取代未初始化的默认对象。所需配置仍是原有 timeout、minor/major enable，不增加配置、模式、依赖或 feature。Root 在原 Router 初始化阶段构造。Router 不再读取宽 `ModuleContext`，没有“未初始化”状态；运行期仍以同一个 RwLock 原子更新配置。

Interaction 测试原 DirectRouter 使用真实 RouterService 的现有 major-only 配置且没有 minor LLM，因此不调用模型做分类。它现在包含真实 major-only prompt；实际回答仍由测试 LLM 产生。

## 保留的行为与清理

- Persona 仍只管理 persona 资源、启用选择、用户覆盖和 Handlebars 渲染；数据库模板失败时仍退回默认模板。没有引入 Companion 关系持久化，也未移动该领域数据。
- `render_system_prompt` 完整保留原 trait 默认的 `self.render_prompt(user_id).await?.system_prompt` 行为。
- Skill 的 intent 匹配、优先级、提示词和 preferred_tools 仅参与上下文及排序，不授予工具权限。插件注册更新后 reload、卸载前外部依赖检查与事务提交、卸载后 reload 均保留。
- Router 的分类超时、JSON 解析失败 fallback、minor-only/major-only/双模型提示、升级提示和候选名白名单/去重/数量上限均未改变。生命周期名称仍为 `multi_layer`，Persona / Skills 名称仍为 `template` / `builtin_skills`。
- `RouteDecision::CacheHit { response }` 没有生产构造者，且缺少规范缓存身份，故删除该不可达 variant；Root 删除 Generation / debug 对应匹配。真实 CacheService 缓存读写与 Context source lineage 不经这个分支，继续走原规范完成出口。
- 三个旧 providers 目录完全删除；不保留兼容层、单产品工厂或为 fixture 引入生产扩展端口。

## 静态证据与可执行检查

已执行：

```sh
rustfmt --edition 2024 packages/alia-server/src/modules/interaction/routing/mod.rs packages/alia-server/src/modules/interaction/persona/mod.rs packages/alia-server/src/modules/interaction/skills/mod.rs
```

基线逐段比较：SkillRegistry 变化仅名称、归属、导入和 trait 方法变固有方法；PersonaService 另补原默认方法；RouterService 将原 init 中现有参数与验证直接移到构造，原算法体与设置错误不变。`skills/markdown.rs` 与基线内容相同。

新增回归：`modules::interaction::routing::service::tests::concrete_router_preserves_major_only_and_atomic_policy_updates`。覆盖初始双关闭拒绝、major-only 健康与 fallback、无 minor LLM 的候选选择 `None`、无效热更新不部分写入 timeout，以及有效切换 minor-only 时保留无模型降级与无升级提示。Root 已在最终库测试中运行，实际结果见末尾。

## 未做项与已知边界

- Persona / Skills 仍通过 ModuleLifecycle 的宽 ModuleContext 初始化；进一步缩窄属于 L10。
- 技能 HTTP CRUD 原有 reload TODO 位于 `api/skills.rs`，不属于本独占目录；已告知 Root，不能把本次插件注册/卸载保留等同于 CRUD 热更新已补齐。
- 未增加外部模型调用、平台、硬件动作、用户数据访问或依赖；默认 Shadow 和政策不变。
- 最终编译、运行时与 Linux 验收已由 Root 独占完成，见末尾。


## L09 守卫补充 — Root 追加分工

`scripts/check-server-boundaries.py` 已覆盖新增七个退休 providers 目录与路径引用、旧实现/trait 名称及 alias、Bootstrap 固定核心工厂定义及其调用、新 canonical owner 必需文件，以及 providers 对迁移后契约的声明/重导出。HTTP `create_persona` 与真实外部 `create_llm` 不受固定核心工厂禁令影响。

自检逐项造旧路径、旧符号与旧工厂的负向夹具；同时验证域内 `ConversationStore`、`CacheStore`、`SensorReadProvider`、外部 `LlmAdapter` 和生命周期端口正常使用不被误拒绝。检查仍是有界文本守卫，不替代 Rust 编译与运行行为回归。

最终脚本的本次静态证据：

- `run/validation-20261009-layering/boundary-self-test-2.log` / `.result.json`：exit 0，`Server boundary checker self-test passed`。
- `run/validation-20261009-layering/boundaries-2.log` / `.result.json`：exit 0，`Server ownership boundaries passed`。
- result JSON 记录所执行命令和脚本 SHA256；`*-1` 是守卫收紧前的首次通过记录，已保留，不冒充最终脚本证据。

以上为整合期间的静态记录；Root 后续已对最终 source freeze 完成整体验证，见末尾。

## Root 最终验收

源码 `ffd7af19dccb8db28f16c5091e09265d4c5bbd32`；V1 `2a06adad2d01d3faee0d369576a907af61d23918f18e5d956a05556dcf0923e5`，**637/637** 输入匹配 Git blob。macOS workspace **1056/0/5**、真实 PTY **3/0**；无默认 Insights availability **2/0**；Linux ARM64 Server 全包 **774/0/5**、默认 release/native link 与无默认检查通过。macOS 无默认 all-targets、fmt 和边界正负自检通过。

本轮 **10/10**，L09—L12 **1/4**，六轮 **2/6**。实际命令、计数和限制见 [最终验证](../../layering-2026-10-09.md#最终验证与证据) 与 [证据索引](evidence-index.json)。本轮任务资源已停止，三个 subagent 已交回，没有待接回的 Cargo/PG 作业；下一步 L10。两项真实模型未重跑，Linux 非全工作区及独立部署 smoke，不声称硬件验收。
