# Expression D1：实例所有权与动作证据

本包处理 Server 本地表达编排及回执解释。设备端 D3 命令协议尚未实现；当前 Embedded 未声明下面的精确命令协议，身体动作会明确 `skipped`，不会调用旧 `execute_expression` 或无作用域取消。语音韵律规划仍可独立使用，本包不把它当作身体动作已完成。

## 本地所有权

`ExpressionRuntime` 持有自己的 `ExpressionOrchestrator`，通过 `orchestrator()` 访问。执行整计划、执行阶段和取消函数都显式接收该实例，已移除进程全局 `OnceLock`。状态键是实例内的 `(channel_id, user_id)`，记录原始 `plan_id`、完整计划、`voice_scope`、优先级及当前未确认 `command_id`。

每键使用一个异步操作锁，覆盖 capability 检查、优先级决策、精确取消和动作调用。相同计划/阶段重入复用已有观察，不重复发送；相同计划 ID 改意图或作用域会拒绝。高优先级只能在相同原始 scope 内抢占；若存在未确认命令，必须先取得该命令匹配的取消完成回执。协议不支持、取消失败、超时或仅 accepted 均不能被当作已取消。

取消函数在等待操作锁前捕获原所有者，获得锁后再次核对 plan、scope 和 command。若已更换则明确跳过，不采纳新计划作为取消目标。执行完成也按原 plan/scope/command 条件更新状态，旧操作不能清掉新所有者。future 被丢弃会释放操作锁，但不会抹掉已发送、尚未确认的命令。等待锁及单次 adapter 调用均限制为 2 秒；没有新增后台任务或自动重试。

状态是当前 runtime 生命周期内的编排状态，不是持久设备账本。只保留每键当前/最近计划的阶段观察，不宣称跨进程重启的去重或可恢复执行。D3 仍须实现设备端命令与负载绑定、幂等、精确停止及实际执行证据。

## 显式 adapter 契约

`expression_capabilities` 只有返回 `command_protocol: "expression-command-v1"` 才允许身体动作。旧的 `hardware_control` 或单项 `supported` 声明不足以通过此门槛；当前生产 Embedded 不应声明尚未实现的协议。

`execute_expression` 参数保留原计划的 `timeline`，并由 Server 填入以下不可变字段：

| 字段 | 含义 |
| --- | --- |
| `command_id` | 实例内 channel/user 身份 UUID、计划 UUID 和 phase 组成的 opaque ID；不可由 adapter 随机替换 |
| `voice_scope` | 原始 `device_boot_id`、`session_id`、`generation_epoch`，无 voice 来源时明确为 null |
| `target_user_id` | 原请求所属用户 |
| `phase` | 本次实际发送的阶段 |

`cancel_expression` 只携带原未确认命令的 `command_id`、`voice_scope`、`target_user_id`、`plan_id` 和取消原因。协议必须按精确命令处理，不能把未知命令解释为“停止当前动作”。不存在未确认身体命令时，仅关闭本地计划并返回 skipped，不伪造设备取消确认。

回执必须回显原 `command_id` 与 `voice_scope`，并为本次发送动作按顺序提供 typed `action_receipts`。动作名、数量和状态都需匹配。只有 `status=completed`、显式 `completion_acknowledged=true`、每个对应 typed receipt 均为 Completed 且无矛盾跳过信息，才确认该已发送命令完成。

`completion_acknowledged` 缺省为 false。无法解析 JSON、缺失/随机命令 ID、错误 scope、错误字段类型、缺失 typed receipt、仅 legacy `executed_actions` 或互相矛盾的状态均返回 Unknown。`executed_actions` 仅从验证后的 typed Completed 观察派生，不再反向合成完成回执。Accepted/Started 只表示相应观察；本地 skipped 不表示设备失败或完成。若计划中另有本地不支持的动作，即使已发送部分完成，也只报告 partial。

## 验证

### Affect 写入所有权

`AffectStore` 的数据库写入现在由该 `ExpressionRuntime` 独占的有界队列和单个 writer 执行，替换未托管的后台 spawn。队列最多接纳 128 批，满队列、持久化失败及提交确认超时均记录错误；不把静默丢弃当作成功。单次写入等待上限为 2 秒，提交确认超时只说明结果未知。

builder 在 attach/start 之前注册 `expression_persistence` 清理责任。关闭封闭与写入同一个接纳锁，排空已接纳批次、join 原始 worker，再允许自有数据库关闭。等待者超时或被丢弃不移走唯一 join handle；已经发生的写入失败在重复关闭时仍保留。该变更解决任务所有权，不代表 Companion 来源失效和隐私传播已经完成。

真实 PostgreSQL 运行时故障测试验证：数据库锁挂起 INSERT，宿主关闭等待超时后仍保留 writer 与 pool，释放锁后同一 shutdown handle 完成排空；数据库拒绝 INSERT 时，最终报告保留错误，但已 join 的 writer 不阻止后续资源和自有 pool 关闭。

关系清除与写入共用同一队列。同步 mutation gate 将 RAM 修改与对应入队排序；Clear 入队失败不会先清 RAM，写入入队失败也不会先应用 appraisal，并通过 `AffectObservation.persistence_error` 明确返回。Clear 要收到数据库提交确认才成功；等待者退出或 2 秒确认超时不撤销已接纳命令。同一用户的 PostgreSQL 事务 advisory lock 还防止超时的旧 SQL 晚提交越过清除。

定向运行 `cargo test -p alia-server --test server_runtime --no-default-features --locked expression:: -- --nocapture`，2026-09-08 最终为 **7 passed / 0 failed / 0 ignored**。新增回归实际阻塞旧 INSERT，清除等待，释放后重启读取不复活；还覆盖丢弃清除等待方、DELETE 故障、队列满/关闭拒绝及确认超时。此处仅保证该实例已经接纳的写入与清除顺序；跨实例和尚未接纳生成结果的完整来源撤销仍属 ContextLease，不能用这些测试外推。

### 编排与回执

本包测试使用明确声明上述协议的受控 adapter，验证 JSON、命令关联和真实 Tokio future/锁竞争，不使用或模拟声称真实硬件验收。覆盖无效 JSON/错误类型、缺失命令及 scope、无 receipt/legacy false 结果、accepted 与完成矛盾、重复阶段、同 ID 改意图、两个 runtime 与用户隔离、同键竞争、旧取消、失败/未知取消以及丢弃 future 后精确取消。

定向命令：

```bash
cargo test -p alia-server --lib expression:: --locked
cargo test -p alia-server --test expression_eval --locked
```

2026-09-08 实际定向结果：默认 features `expression::` 库测试 **41 passed / 0 failed / 0 ignored**（含本包 11 项编排测试），测试执行 0.01 秒；已有 `expression_eval` **7 passed / 0 failed / 0 ignored**，0.02 秒。关闭默认 features 的同组 Expression 库测试也 **41/41** 通过。两种 feature 运行不重复计算测试数量；本包共 48 个不同测试通过。`git diff --check` 通过。此结果不代表整个 workspace、数据库 Delivery 或真实身体动作协议已验收。
