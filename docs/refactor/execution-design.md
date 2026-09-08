# Execution 实施边界

状态：P2 的 repository、worker、工具入口和 REST 已实现，整合回归已通过，随本包提交。实际测试范围见 [故障验证](execution-validation.md)，操作契约见 [Execution API](execution-api.md)。本包不引入 Temporal，也不改变前端延期范围。

## 现有问题与替换边界

旧工具入口在调用超时后丢弃 future，并生成普通失败结果；普通 trace 也不能保证副作用结果已经持久保存。原来的 REST、语音与 scheduler 没有共同的确认与执行账本。

当前保留可复用的工具定义、Provider、具体 executor、路由和参数解析。执行授权、确认、持久状态、幂等、恢复和超时语义由 `modules/execution` 拥有。ModuleRegistry 暴露的工具注册表与 AppState 共用一个 ExecutionService，原始 executor 仅供该服务的 worker 调用。

## 不变量

1. 所有产品入口通过同一个 Execution 服务；目录查询和插件注册可继续使用工具 catalog。原始 executor 只由 Execution 的 worker 调用，不能保留 REST/调度绕过路径。
2. 用户身份来自已验证入口并在批准和分派前核验当前用户状态、角色及工具可用性。模型参数中的 `confirmed`、角色或权限不能当作授权。确认绑定具体 effect、用户、参数和工具定义版本。
3. 规范 `effect_id` 不使用模型的 `call_id`。幂等范围包含用户、调用请求 ID 和调用关联 ID；后来的新轮次必须有新的请求 ID，不能因模型重复使用 `call_0` 误合并。相同幂等键配不同参数必须拒绝。
4. `proposed → approved → dispatched → succeeded / failed / unknown` 独立于语音和回复状态。未分派的 proposed/approved 可取消；已分派后不能仅凭取消 token 宣称现实动作已撤销。
5. proposed 记录可等待明确确认。approved 记录与待分派 Outbox 在同一个数据库事务提交；批准后进程退出，重新扫描仍可发现待分派任务。通知只用于唤醒，不承担可靠性。
6. worker 认领与 `dispatched` 记录必须原子提交。并发 worker 使用行锁与认领标识；外部调用不持有数据库事务。对已分派但超时、进程退出或丢失回执的不可核对操作，保存 unknown，禁止盲目重新调用。
7. 只在有适配器证据时写成功或确定失败。普通传输错误无法证明副作用没有发生；写结果失败也不能悄悄降级为 trace。恢复扫描把过期分派收敛到 unknown，保留核对入口和原因。
8. 队列、待确认数量、并发、单次执行时限与结果大小有明确上限。运行任务由宿主监督；关闭时停止认领并收束已分派任务，不能只 abort 而丢失执行事实。

## 实施包出口

- 新 PostgreSQL schema、状态转换、与同事务 Outbox 的 repository；迁移编号由主 agent 分配，已经应用的迁移文件不再修改。
- 工具入口接入 Execution；REST 提供查询、确认和取消，并保持用户隔离；无前端也可操作。
- 语音中断不再抹除工具结果。LLM 得到明确的 pending/confirmation/unknown 表达，不把这些状态转换成“已完成”。
- 使用独立真实 PostgreSQL 验证：并发幂等、同键参数冲突、批准与 Outbox 原子性、批准后崩溃恢复、认领后崩溃进入 unknown、撤权、取消与分派竞争、结果落库失败和重复回执。
- 生产路径完成接入后提交；仅有表结构或纯状态机不计为这一实施包完成。

## 已实现的状态与资源约束

`execution_effects` 保存规范意图、批准、认领与结果；`execution_outbox` 仅保存已经批准且尚未认领的任务；`execution_events` 与状态变更同事务提交。恢复扫描不会重新调用已经 dispatched 的工具。失去结果的分派记录在执行期限及宽限期后转为 unknown。

幂等键是 `user_id + request_id + invocation_key`。模型提供的 call ID 仅供关联，不参与幂等或授权摘要。批准绑定意图摘要；分派再次核验当前用户、角色、工具目录、实现版本和批准期限。worker 持有认领时解析出的固定 executor，热注册不会把已经认领的任务换成另一份实现。

待确认意图每用户最多 32 项，批准队列最多 256 项，4 个 worker 并发执行，参数上限 16 KiB，结果上限 64 KiB，批准有效期 15 分钟，执行期限最多 300 秒。结果持久化可以重试，外部副作用不会因此重试。通用错误或超时默认 unknown；只有 executor 明确承诺失败具有确定证据时才保存 failed。

会话取消只关闭尚未认领的自动意图。已认领任务由独立 worker 继续收集结果；需要用户确认的 proposed 意图和明确的非会话后台请求有各自持久生命周期。提交 5 秒、结果等待 10 秒和每次关闭 10 秒的整体期限已经通过真实数据库锁/回执丢失测试；超时保留未确认取消及 worker 句柄，不以调用 future 已结束代替工作已经结束。

## 明确未由此包解决

设备播放的实际呈现、动作到位反馈、Delivery 生命周期和 Companion 策略仍需各自实施与验收。Memory 删除失效在同轮独立包中实现，具体证据见 [Memory 审查](memory-findings.md)。数据库账本不能证明用户听到音频或机构已经到位。后台任务引擎选型继续遵守台账 C01；此包不建立通用消息中间件。
