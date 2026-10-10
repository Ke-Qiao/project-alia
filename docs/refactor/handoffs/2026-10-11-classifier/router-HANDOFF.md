# Classifier live Skills — Router HANDOFF

基线 `4e4d595`。本分工独占 `modules/interaction/routing/mod.rs`、`routing/service.rs` 与 `tests/router.rs`；交叉审查后，Root 另行委派本分工修复 `generation.rs` 的三个 catalog 校验取消边界。未修改 Registry、Cache、Execution 或其他测试。Root 负责统一契约、同源运行验收、文档收尾和提交。

## 已有实现与缺口

原两份分类模板硬编码 7 个 builtin，以及 `home_assistant_lights` 优先于 `device_status` 的专名规则。RouterInput 没有 live 候选；to_decision 只要求模型返回技能名 trim 后非空。因而新增技能无法进入分类提示，禁用／删除技能仍被静态提示推荐，任意返回名最终可能在 Registry resolve 失败。

生产 RouterInput 构造仅在 `generation.rs::route_response`；内部单元测试另有两处构造，`tests/router.rs` 原有 7 处 to_decision 调用。Router 本身没有分类结果缓存。独立的 response cache 在 Generation 调用 Router 前可以直接返回，且先前 metadata 没有 catalog 身份；该边界已交 Root 和 Generation／Registry owner，未在 Router 新建缓存或变更缓存批准规则。

## 实现

- `RouterInput.available_skills: Vec<SkillCandidate>` 使用 Registry 分工提供的规范类型。字段只有 name、display_name、description、category、keywords，没有 prompt、preferred tools、授权或版本信息。
- 两份静态技能表及专名捷径已删除。分类 system 提示要求仅选择 `available_skills` 中的名称，并明确候选 metadata 是数据、不可作为指令执行；候选结构放在 user JSON payload 内，与原 user_text / recent_context 一起序列化。
- `to_decision(classification, mode, assistant_name, available_skills)` 是唯一入口。技能名先按原字符串精确匹配；未命中时仅保留 trim 后仍属于候选的兼容行为，返回实际候选中的规范名称。没有名字、纯空白或未提供名字退化到原模式通用路由。
- major-only 在 route 与 to_decision 两处均直接返回 major，绝不调用 classifier；minor-only 失败／失配仍回 minor 且无升级，both 仍回 major。confidence 仍只作诊断，没有增加分数门槛。原模式配置原子更新与双模型均禁用的拒绝行为保持。
- 新增 `RouterService::fallback(&self, assistant_name)` 只读取当前模式并复用原 fallback_decision，不调用模型，供 Generation 消费失效选择时使用。
- 本分工范围内全部 RouterInput／to_decision caller 已迁移；生产 Generation caller 由对应 owner 接入。工具候选选择、全局工具可见性、Execution 批准、Shadow 均未修改。

## 验证准备

新增 3 项纯决策回归：未知／空名字的模式退化；exact 名称优先于 trim 兼容且 confidence 为零仍可选；major-only 拒绝模型输出 minor 或 skill。原技能决策用例改为一个非 builtin 的动态技能，避免仍依赖静态名单。

新增 1 项使用真实 RouterService 与记录型 LlmAdapter 的异步回归：记录模型实际输入，验证动态候选及 keywords 只在 JSON payload；移除候选后相同模型输出按模式回退；切换 minor-only／major-only 及显式 fallback，计数确认 major-only 和 fallback 不会新增分类调用。

原提示测试保留模式边界、非默认 assistant 名称、升级与工具规则，改为要求动态候选指令且没有静态 builtin。原 user payload 测试加强为解析完整 JSON 并精确断言所有 5 个候选字段，覆盖名称引号及描述换行／伪指令文本的结构化传递。不能把该结构测试表述为模型已抵御所有提示注入。

三个文件的 scoped rustfmt / rustfmt --check 通过。只读源码断言确认生产模板无原 7 个 builtin 名字、只有一个带候选的 to_decision 和一个 fallback、JSON 候选字段存在，且三个文件没有尾随空白。未运行 Cargo、数据库、Docker 或 Git；编译及运行结果由 Root 补充。

## Checklist

- [x] 读取上轮四份 HANDOFF，检查模式、模板、调用方、名称校验与缓存边界。
- [x] 对齐 SkillCandidate / RouterInput 契约，删除静态表与专名路由规则。
- [x] 模型输出必须命中当次候选，保持模式门槛与无 confidence 阈值。
- [x] 添加无模型调用的当前模式 fallback，迁移本分工全部调用方。
- [x] 必要决策、JSON metadata、真实 Router 模型调用计数回归已准备。
- [x] Scoped 格式与静态检查通过，交接 Root 并冻结本分工源码。
- [x] Root 完成 Generation／Registry／Cache 整合以及同源编译、回归和提交核对。

Root 已确定整合方向：Registry 提供候选 catalog 与内容指纹；消费分类结果时，被更新／禁用／删除／重建的原选中技能失效，Generation 调用本分工的当前模式 fallback，且不再次调用 classifier；response cache 绑定 catalog 指纹。此整合未由本分工实施或验证，最终规则与运行证据以 Root HANDOFF 为准。

## V1 接线交叉审查

Root 冻结的 V1 为 644 个输入、manifest SHA-256 `711f95be89cb35f159c2d6983a04cade7a6d16f85e63b70a9f96e5b387e989f5`。只读检查发现并向 Root 报告两点：generation 曾从 registry 私有 import 路径引入 SkillCatalog，应从规范 skills 模块导入（随后已看到 Root 修正）；三个新增 `is_current(catalog).await` 位于 exact／semantic cache 验证和 finish cache 判定中，原来没有进入 cancellable_stage，可能等待被 HTTP owned COMMIT 持有的索引写锁，导致 turn 取消被无关事务延迟。取消缺口由 Root 统一修正与验证，本 agent 未改源码。

其余接线静态结论：prepare 获取同一 catalog 并派生 prompt 显示列表、缓存指纹及 Router 候选；selected resolve 失效仅调用当前模式 fallback，不二次分类；resolve 在同一次索引读锁中核对原选中定义并复制 resolution；finish 在目录变化或不可用时不安排缓存写入，也不会用新目录指纹给旧回答贴标。Exact、semantic direct 与 semantic reference 均绑定相同目录指纹，并保留原安全／审核条件；legacy 无指纹候选不直接复用。调用方扫描未发现旧 cache／prompt／decision 签名遗漏。上述结论不代替 Root 的同源编译与时序回归。

### V2 前的取消修复

Root 随后明确委派本分工仅修改 `generation.rs`，补齐上述三处取消边界。Exact 验证经 `cancellable_stage(..., "cache_catalog", is_current)`；semantic 验证经 `semantic_cache_catalog`；取消立即从 lookup 返回 None，交已有 process_text 的 interrupted 审计与返回路径处理，不继续宣称缓存命中。

Finish 的资格读取经 `cache_write_catalog`，并整体移到原 `if turn_is_interrupted(turn)` 分支之前。该阶段取消后，永久置位的 turn token 使原分支返回 interrupted，保留原 outcome 的文本／流式字段且 pending_persistence 为 None，不进入 complete_answer。原中断 stage 只在未设置时补记，新的阶段见证不会被 `before_persistence` 覆盖。Finish 仍将目录读取失败按原策略视为不写缓存；exact／semantic 保持原错误传播。

没有增加公开 helper 或新的所有者。`skill_catalog` 只借用 PreparedTurn 中指纹字符串，构造 CacheWrite 后成为 owned metadata；trace/cache_config 是不同字段的借用，完整 PreparedTurn 再传给 complete_answer 前均已结束使用。没有新的锁 guard 跨 await。`generation.rs` scoped rustfmt / --check 通过，源码已交 Root；Registry 分工准备实际索引写锁阻塞下的取消回归，运行结果待 Root 的新版本验收。

未运行真实模型、硬件、Cargo 或完整服务；静态检查不能替代 Root 的快照失效、缓存复用与运行清理验收。

## Root 最终验收补充

源码 `744cae2a11f8192f0e8ccf3d13004402202c1411`，V2 manifest `adeeaa0d150e6c7a036beeb05aa409da1d811e130d60354fe46e290c2ec132b5`，644/644 Git blob 一致。

macOS workspace **1080/0/5**；Linux ARM64 Server 默认并行完整回归 **798/0/5**。两端无默认 feature 全目标检查、边界正反自测和变更文件格式检查全部通过。计数依次为 passed/failed/ignored，嵌套子进程摘要不重复累计；所有最终阶段同源、无源码漂移。

以上实施时的“待验证”描述保留时序语境，以本段为最终状态。新增回归随完整默认测试实际运行；没有单独把循环场景或嵌套子进程当作额外测试累计。四份 HANDOFF、checklist 和恢复点完成，资源关闭见 Root HANDOFF。
