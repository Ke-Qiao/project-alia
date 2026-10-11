# Health aggregation Registry HANDOFF — 2026-10-11

负责：`/root/skills_registry`。本轮基线 `d27490f`。Root 统一冻结、运行 Cargo／数据库／容器验证及提交；本 agent 只改本分工源码、内部测试和交接文档。

## 审计结论与边界

原 `ModuleRegistry::health()` 顺序执行所有检查，无整体期限、跨请求并发准入或停机取消。唯一生产调用者是 `api/health.rs`，唯一 Registry 构造点是 `bootstrap/builder.rs`。原 Router 再次调用已列入聚合的 minor Provider，并在等待期间持配置读锁。原 SkillRegistry 检查也可能等待正在 COMMIT 的 index 写锁。

Builder 原来为每个 LLM 角色单独创建、初始化实例，即使 profile 名相同也没有共享 Arc；default_llm 是 minor/major 的别名且不在 health 字段中。不能按 `ModuleLifecycle::name()` 合并独立实例。本轮 Root 负责配置同 profile 的 Builder 实例共享及初始化／start 单所有者；自定义注入保持独立。Router／Gemini 由另一 agent 负责。

`apply_settings()` 在启动和 HTTP settings 路径被调用。聚合 admission 锁不参与设置更新；本轮只把 LLM 角色改为每个共享实例调用一次，其他模块和 extensions 的顺序、报告与失败语义保持。启动 `verify_memory_health` 属于原 startup_step 取消路径，不调用本聚合，也未更改启动策略。

## 已实现 checklist

- [x] Registry 新增 `health_gate: tokio::sync::Mutex<()>` 和 `health_cancel: CancellationToken`，由 Root 在 Builder 绑定服务器取消 token。
- [x] 保留原 health 字段集合和可选角色出现条件；返回类型及 `/health` API 不变。
- [x] 同 Registry `try_lock` 拒绝重叠巡检：已有全部字段返回 Degraded `health check already in progress`，不等待、不启动新 probe。
- [x] 每轮使用 6 秒绝对总期限，同时最多 8 个检查；排队项不会重新获得 6 秒预算。
- [x] 同一对象数据指针出现于多个角色时只检查一次，再向各角色复制结果；同名不同对象不合并。
- [x] 期限届满保留已完成结果，仅未完成字段返回 Degraded `health check timed out after 6s`。
- [x] 服务器取消时保留已完成结果，仅未完成字段返回 Degraded `server is stopping`；已取消后新请求全部字段 stopping 且零 probe。
- [x] 所有检查 Future 由当前请求持有；超时、取消、调用者 drop 均直接释放，不 spawn、无后台所有者和结果缓存。
- [x] 单检查 panic 转为固定 Unhealthy `health check panicked`，不把 panic 文本放入 HTTP 响应，也不中断其他检查。
- [x] 新增 `unique_llms()`，按 minor、major、memory_capture、sentence_expression 的首次出现顺序去重，供 Builder start 与 settings 共用。
- [x] 5 条内部回归完成并 scoped rustfmt：别名与同名异实例／panic、整体期限及已完成保留、8 并发与排队期限、busy／drop 后重新准入、stop 释放及后续拒绝。
- [x] Root 完成 V2 同源双平台验收与源码提交，最终记录见下文。

## 内部测试证据边界

`bootstrap/registry_tests.rs` 调用实际生产聚合 helper，使用现有 Tokio 暂停时间和可控 ModuleLifecycle Future。计数 guard 在 Future 释放时归还 active 并记录 released，可检查并发上限以及超时／取消后无在途 Future；排队场景还验证第 10 项未启动也按原 deadline 结束。

同 Arc 两个角色只产生一次调用，同 name 的独立实例各自产生调用；Root 的真实 HTTP 测试负责证明 Builder 配置同 profile 的生产共享接线。内部测试不宣称同 profile 名本身足以证明实例等价，也不把 future drop 证据表述为任意远端服务已经停止处理 HTTP 请求。

检查是异步合作式执行。整体期限约束可让出的 Future，不为阻塞执行器的任意同步扩展代码建立隔离线程。本轮没有新依赖、配置项、HTTP 查询模式、付费探测或 probe 默认值变化；默认 `probe=false` 继续由 Provider 配置控制。

## 交叉只读审阅

已查看 Router 本地 health 与 Gemini provider health／本地测试：Router 不再调用 minor.health_check；Gemini 先 clone 配置再释放读锁，GET probe 有独立 5 秒期限，错误只返回 timeout／connect／request 类别。原 missing key／probe disabled／HTTP status 语义保持，热设置影响下一次请求，不缓存旧状态。未发现需要新增生产范围的阻断项。

源码范围：`bootstrap/registry.rs`、新增 `bootstrap/registry_tests.rs`。未修改 Builder、API、Router 或 Provider 文件。未运行 Cargo、数据库、Docker 或 Git。

## Builder 最终只读所有权复核

已复核 Root 的 `builder.rs` 与 `builder_llm_tests.rs`，未发现阻断项。配置角色先按实际 profile 准备唯一 Box；`initialize_llm_role` 仅首次调用 `cleanup.initialize`，成功后才进入 initialized map，后续角色 clone 同一 Arc。配置禁用且未自定义覆盖的角色仍使用 noop；optional profile 缺失返回 None。custom override 优先且独立初始化、不写入共享 map，因此不会被同名配置角色接管；default_llm 仍是原角色别名，不新增所有者。

`initialize_owned` 在 init 首次 poll 前已注册 rollback slot。失败或取消时，即使准备 map 的 Box 已取出，cleanup 仍持有正在初始化的对象；成功共享后仅有一个 stop action。外层 startup_step／ServerStartup 继续持有失败清理，start 与 settings 使用 unique_llms，不会因角色别名重复调用。新增 Builder 单测验证配置共享一次 init／stop、缺失 optional、独立 override 及 override init 失败后的回收；configured 首次失败静态走同一 rollback 守卫，未把未执行的测试当作运行证据。

工厂现有 OpenAI、Anthropic、Gemini 实例只持有 HTTP Client、profile_id 和受 RwLock 保护的配置；Noop 无状态。对话消息、请求参数、SSE decoder、tool-call 聚合和响应均在每次调用内构建，不存在实例级对话历史或解析缓冲。因此同一配置 profile 的角色共享不会把一次回答的内容混入另一次调用，不同 profile 仍使用不同实例。

常规生成路径原来就有持配置读锁等待网络的代码，本轮未更改该路径，也不声称修复其设置更新等待；health 路径的配置快照／独立期限另有本轮实现和验证。以上为静态审阅，未修改 Root 源码，未运行 Cargo／Git。

## V1 编译失败与 V2 候选修正

Root 确认 macOS 与 Linux ARM64 的 V1 编译均退出 101，冻结输入没有漂移。编译器在 `/health` Axum handler 的 Future 约束处报告 `FnOnce is not general enough`，指向 Registry 的 `stream::iter(instances.into_iter().map(... async move ...))`。该惰性 Map 将带 trait-object／角色名称借用的 mapper 闭包保留到跨 await 的聚合 Future，不能满足所需的高阶生命周期约束；V1 因此未通过编译，不能计作测试通过。

Root 确认两端退出后，本 agent 仅将该段改为 for 循环预先构造 Future Vec，再 `stream::iter(pending).buffer_unordered(8)`，并 scoped rustfmt。跨 await 的状态不再含 mapper 闭包；没有新增装箱、线程、任务或依赖。构造 Future 不等于 poll：实际 probe 仍受 8 个并发槽位和原来的绝对 6 秒期限约束，超时／取消／drop 所有权不变，测试和期限没有放宽。

- [x] 已应用上述最小修正，V2 source ready。
- [x] Root 已冻结 V2 并完成双平台编译及完整运行，结果见下文；本 agent 未运行 Cargo。

## Root 最终验收补记

源码 `29420a57d6558d90c36098c061655dff40eb72f4`，V2 manifest `9902508f7743bbbae5d447fec76c753d9f8910b0b1b019ef3ea5f801d03a21b0`，646/646 Git blob 一致。

macOS workspace **1094/0/5**；Linux ARM64 Server 默认并行完整回归 **812/0/5**。两端无默认 feature 全目标检查、边界正反自测、变更文件格式检查和计数器 8 例自测全部通过。数字依次为 passed/failed/ignored，嵌套子进程摘要不重复累计；所有最终阶段同源、无源码漂移。

V1 两端首次编译均因聚合 stream 内 async mapper 的高阶借用生命周期泛化失败而 exit 101，没有测试通过数字。确认两端退出后，V2 仅把 mapper 改为先构造借用 Future Vec 再进入 buffer_unordered，期限、并发、取消和测试逻辑均未改变；两个版本清单与原始日志全部保留。

所有 Cargo 阶段结束；关闭前其他数据库连接为零，最终测试 schema 为零（遗留 schema 清理数 0）。V1/V2 Linux holder 与任务 PostgreSQL 已停止，用户 5433 未访问。空闲 holder 的 sleep 容器停止码不作为测试结果；V2 验证命令均 exit 0，V1 编译失败 exit 101 另行保留。缓存与只读快照保留在 ignored `run/validation-20261011-health/`。

所有 Arc 身份断言在 V1 冻结前已移入既有 catch／stop_runtime 保护区域；运行通过仍以 V2 完整日志为准。上文“尚未运行”描述 subagent 交接当时状态，此处为后续实际验收。

未运行 Linux 全 workspace、release/native release link、部署 smoke、真实 PTY、外部或付费模型、Pi/声学/执行器及长稳验证。6 秒预算约束合作式异步检查，不能强制中断同步阻塞；本地取消不代表远端立即停止处理请求。
