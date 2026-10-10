# Live classifier Registry HANDOFF — 2026-10-11

负责：`/root/skills_registry`。本轮基线：`4e4d595`。Root 统一执行 Cargo、真实 PostgreSQL、容器、验证冻结及提交；本 agent 不执行这些操作。

## 已检查的生产链路

`process_text` 在 safe-direct exact/semantic cache 未命中后调用唯一生产 `route_response`，它构造唯一生产 `RouterInput`。旧 Router 的两套 classifier prompt 固定列出 7 个 builtin，`to_decision` 只要求 skill 名称非空，generation 构造置信度为 1 的临时 SkillMatch 后调用 Registry.resolve。技能在分类期间删除或停用会让整轮报错。

`build_runtime_prompt_context` 在更早的 prepare 阶段读取 enabled skill 的 display_name 用于能力展示；该列表不是权限或模型候选校验。现有缓存元数据没有 skill 身份或目录版本，safe-direct 命中可绕过分类和 resolve。Root 已确定本轮让缓存绑定目录内容，并在分类期间选中技能被修改、停用、删除或同名重建时放弃旧选择；Root 负责 generation/prompt/cache 接线及同源验证。

## 本 agent 当前完成项

- [x] 新增 `SkillCandidate`，仅含 name/display_name/description/category/keywords，derive Debug/Clone/Serialize/Deserialize。
- [x] `SkillRegistry::catalog()` 从当前已发布 enabled map 一次读取，按 priority 降序/name 升序排列；索引 unavailable 保持 Err。
- [x] 候选不携带 prompt_template、preferred_tools、cacheable 或执行权限。
- [x] 添加无 DB 单元回归验证稳定排序、完整 JSON 字段边界与 unavailable 错误。
- [x] 向 Router owner 对齐 `RouterInput.available_skills: Vec<SkillCandidate>`。
- [x] 审计现有 sha2/serde_json 可直接复用，不需要新依赖或跨域通用指纹框架。
- [x] 最终单一快照契约 SkillCatalog：public candidates/fingerprint，private 各名称定义摘要；删除临时 candidates() 旁路。
- [x] `is_current(&catalog)` 比较整目录内容；`resolve(&catalog, name)` 在同一个短读锁内校验本次私有定义摘要，仅同 ID、同内容才返回 resolution。未提供、变化、删除、停用返回 None；索引 unavailable 返回 Err。
- [x] 旧 resolve(SkillMatch) 已删除；迁移 Registry 单元测试。Root 迁移 generation/runtime 测试调用者。
- [x] LoadedSkill 保留 DB row id，更新 tests/skills.rs 三个构造。
- [x] 新增内容检查回归覆盖全部定义字段变化、同名重建、无关技能不受影响、顺序不敏感、仅时间戳变化/重载稳定、不可用与候选字段边界。
- [x] Root 编译、完整回归和提交核对。

## 已实现的内容指纹与消费边界

LoadedSkill 保留 DB row id。`definition_digest` 对带有 `skill-definition-v1` domain 的固定 tuple 序列化后计算 SHA-256，内容包括 id、完整 SkillInfo、keywords、排序后的 preferred_tools、prompt_template、cacheable、priority。排除 updated_at：相同内容的启动/重载不造成误失效。工具偏好在现有选择逻辑中是集合式排名提示，因此只在指纹输入中排序，实际加载/resolve 数据仍保留原表示。

目录摘要通过按名称排序的 BTreeMap 定义摘要和 `skill-catalog-v1` domain 计算。候选按 priority 降序/name 升序输出。已有 sha2/serde_json 直接复用，没有引用执行域私有 digest 或增加框架、依赖、后台任务和全局 epoch。

模型仅收到 SkillCandidate 五个 metadata 字段；目录摘要、DB id、定义摘要、prompt/preferred tools 不进入 classifier payload。generation 持有本次 SkillCatalog，消费前验证所选项；无关技能变化会使目录 is_current 返回 false，但不阻止未变的选中技能 resolve。缓存读取/写入使用整目录指纹，具体接线由 Root 负责。

resolve 校验并 clone resolution 是消费的线性化点；之后已开始的生成不因技能目录更新而自动撤销，执行器可见性及批准仍在执行链路各自验证。没有长期持有读锁跨模型调用。

## 保留边界

技能仍提供认知上下文和工具排序偏好；工具发现、执行器存在性、可见性、批准及 Shadow 规则继续由现有模块负责。候选 metadata 不可作为授权，也不直接进入下游 system prompt。Registry 上轮事务屏障、原子发布、取消所有权、stop 和 unavailable 恢复保持。

本 agent 未运行 Cargo/数据库或改动 generation/prompt/cache；当前 checklist 只描述实现状态，不代表验收通过。Root 需格式化与编译，运行真实 PostgreSQL 重载/重建回归和整体验证，更新最终通过数及源码提交。

## V1 冻结后只读复核

V1 共 644 个源码输入，manifest SHA-256 `711f95be89cb35f159c2d6983a04cade7a6d16f85e63b70a9f96e5b387e989f5`。本项为只读审阅，不替代 Root 实际测试；冻结后未修改源码。

exact、semantic direct、semantic reference 共用目录绑定检查：metadata.skill_catalog 必须是字符串且等于本轮快照指纹。旧缓存缺字段、非字符串或不同目录均不能复用；generation 同时对本轮快照执行 is_current。CacheService.apply_review 在锁定原版本后强制保留原 metadata 的目录绑定，原条目无绑定时删除 review 传入的伪造绑定。审核不能把旧缓存重新标记成新目录。

缓存消费的线性化点是 is_current 成功读取；后续 merge Context/trace 的 await 不持有 Registry 锁。写入资格的线性化点是 finish_generation 结束前再次 is_current；之后目录变化可能先于排队写入实际落库，但该条目仍保留旧指纹，未来新目录不能命中。该设计不撤销已经准入的文本，也不宣称目录锁一直持有到响应持久化。

定义摘要覆盖所有 LoadedSkill 的运行内容及 DB id，enabled 变化体现为目录成员变化。source/created_at/updated_at 不参与当前 routing/resolve；preferred tools 原选择逻辑转换为 HashSet 后排序，是集合式排名提示，因此指纹内排序不遗漏运行语义。只读审阅未发现新阻断；实际通过数等待 Root 验收。

## V2 目录检查取消回归补充

- [x] 与 `/root/skills_http` 对齐三个取消阶段 `cache_catalog`、`semantic_cache_catalog`、`cache_write_catalog`；generation 修复由其负责。
- [x] 在既有 `integration_tests/cache.rs` 新增 `cancelled_generation_leaves_blocked_catalog_finish_without_persistence`，调用真实 `process_text`，无生产测试钩子、可见性扩张或新夹具框架。
- [x] 复用现有 PostgreSQL linked fixture。模型先暂停以完成目录捕获，再让真实 SkillChange COMMIT 进入 deferred trigger 的 advisory lock 等待；通过 `pg_stat_activity` 确认这时提交所有者持有 Registry index 写锁。
- [x] 释放模型并 poll 生成 Future：既有夹具同步记录 `llm_result`，随后目录读锁等待保持 Pending。取消后必须在 1 秒内返回，且 COMMIT 仍未完成，精确阶段为 `cache_write_catalog`。
- [x] 断言回答文本与非流式标志保留、interrupted=true、pending_persistence=None、未建立 Response、未交付文本；terminal audit 保留用户输入且明确 assistant/cache/memory 均未持久化。断言失败也先释放 advisory blocker 并等待提交所有者清理后再传播 panic。
- [x] Root 在 V2 冻结后的完整验证中运行新增锁取消回归；本 agent 仅 rustfmt，不运行 Cargo、数据库、Docker 或 Git。

这条新回归覆盖实际 finish 路径的 `cancellable_stage`，不把仅调用工具函数的镜像测试当作生成链路证据。exact/semantic 新增目录检查的取消接线由只读审阅覆盖，本次没有为它们增加重复 PostgreSQL 锁场景。

## Root 最终验收补充

源码 `744cae2a11f8192f0e8ccf3d13004402202c1411`，V2 manifest `adeeaa0d150e6c7a036beeb05aa409da1d811e130d60354fe46e290c2ec132b5`，644/644 Git blob 一致。

macOS workspace **1080/0/5**；Linux ARM64 Server 默认并行完整回归 **798/0/5**。两端无默认 feature 全目标检查、边界正反自测和变更文件格式检查全部通过。计数依次为 passed/failed/ignored，嵌套子进程摘要不重复累计；所有最终阶段同源、无源码漂移。

以上实施时的“待验证”描述保留时序语境，以本段为最终状态。新增回归随完整默认测试实际运行；没有单独把循环场景或嵌套子进程当作额外测试累计。四份 HANDOFF、checklist 和恢复点完成，资源关闭见 Root HANDOFF。
