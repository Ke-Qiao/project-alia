# Settings 等待与关闭边界 — 独立审查 HANDOFF

日期：2026-10-15。基线 `e89d34c`。分工 `settings_review`。本分工读取前轮四份 Settings/hardware HANDOFF、当前 Settings/Privacy API、Registry、ModuleRegistry、Runtime、连接所有者及本机锁定的 Axum 0.8.9 / Hyper 1.9.0 源码；新增真实 HTTP 关闭回归，未修改生产代码，不执行 Cargo、数据库、模型、Git、SSH 或硬件操作。运行复现及最终证据由 Root 统一收集，本文不把静态推导当作测试通过。

## Checklist

- [x] 核对前轮已证明内容与尚未动态验证的边界。
- [x] 核对 GET/PATCH Settings、Privacy PATCH 共用 gate、各阶段副作用与关闭传播。
- [x] 向 Root 提供真实 HTTP 关闭与 pipeline 缓冲故障复现设计。
- [x] 明确客户端断开、handler future drop、服务关闭与同步持久化的区别。
- [x] 新增真实 TCP pipeline、客户端断开与共享 policy 锁等待三项回归，完成 scoped rustfmt。
- [x] 复审 LLM/TTS 最终差异与最初关闭候选的实际结果。
- [x] 核对 Root 执行三项扩充回归的最终结果。

本分工 **7/7**；这是审查与所属回归交付完成数，不是 Settings 全部故障类型、R05 或项目整体完成数。

## 基线发现与测试优先级

1. 三个管理入口都直接等待 `SettingsRegistry::lock_updates`，Settings preview、顺序 module apply 及 InteractionPolicies 写锁没有观察 `AppState.cancel_token`。在途慢模型可通过 provider config 读锁挡住 apply，再由 Settings update gate 挡住其他管理请求；模型锁和 TTS 额度等待由另两位分工复现及修复。
2. Runtime 收到停止后先通知 Axum graceful shutdown，按原 drain 等待，再令 `Connections::close` 取消底层后续 I/O，继续 join。关闭总期限到达只报告 `ShutdownIncomplete` 并保留 cleanup owner，不会悄悄放弃资源。
3. **最初的关闭缺口推测未复现，不能据此新增取消包装。** Root 在未修改 API/Runtime 的基线上执行 pipeline + 挂起 apply 测试，实际 **1 passed / 0 failed / 0 ignored**，日志见证 `apply_dropped=1`、`active_tasks=0`、原文件未变；冻结 manifest `6c15c1053908e4cf9c3fca572798cd73aef50ad9f82e099df65fbdc9e4d78490` 无漂移。Hyper `mid_message_detect_eof` 的确可能因后续缓冲字节不再读，但 dispatcher 每轮仍执行 `poll_flush`。锁定版本默认 `flush_pipeline=false`；即使写缓冲为空，`h1/io.rs::poll_flush` 仍调用底层 flush，现有 `Connection::poll_flush` 会观察关闭 token 并返回错误。只读取 EOF 路径会遗漏这一实际释放机制。证据中的阶段名 `red` 表示当时尝试复现，不表示测试失败。
4. 同一受控 extension 在 await 前记录一次本地效果，用 RAII drop sentinel 记录 future 真正释放。这样可以区分发出 `abort`、丢弃客户端句柄和 handler 自身已结束，也能保护部分成功语义：desired/已提交 SQL/既有模块效果保留，未开始的 save 不得自行在后台继续。

## 保留的事务与权限边界

| 阶段 | 取消后允许保留的状态 | 不得宣称 |
| --- | --- | --- |
| 等待 update gate | 本请求尚未进入操作；其他持有者仍有自己的生命周期 | 客户端 task 被 abort 就等于 server handler 已取消 |
| current / 纯 preview / 身份准入 | `current()` 自己恢复到期政策的 SQL 变化可能独立发生 | 所有 SQL 都无变化 |
| SQL CAS 提交或确认附近 | 已提交政策，或确认不确定；需重新读取政策版本 | 一律回滚到旧政策 |
| desired / 顺序 module apply | 已发布 desired 与已完成的模块效果；同值显式重试继续应用和保存 | 整个 PATCH 的跨模块原子性 |
| 尚未开始的 save | 旧完整磁盘文件；之前的 SQL/runtime/desired 不回滚 | 后台会自动完成保存 |
| 同步文件发布 poll | 原 `SettingsSaveError` 的发布前/发布后持久性语义 | Tokio cancel/timeout 能抢占同步文件系统调用 |

权限继续由入口 AuthUser、非政策 PATCH 最后 current-admin 查询、政策 CAS 的 SQL actor 验证、GET 返回敏感 entries 前的最后 current-admin 查询控制。关闭取消不能代替身份验证，也不能把用户已确认的部分成功语义改成全局回滚。

## 当前修复范围与覆盖结论

保留当前 Settings/Privacy API 和 Runtime 关闭实现。已经通过的 pipeline 关闭证据不支持新增 handler 取消包装，也不支持修改 API 响应或请求预算；本轮只修复由另两位分工实际复现的 provider 配置读锁和 TTS 额度准入预算缺口。不在整个 PATCH 外增加任意总超时，不新增脱离请求所有权的保存任务。

新增 coverage 聚焦实际可观察的 apply 取消、客户端断开及同值恢复、privacy 已提交 SQL 后等待共享 policy 锁时的关闭。复用现有 `server_runtime` fixture、扩展模块和 `ShutdownHandle`，没有新增生产测试接口。shutdown handle 成功意味着 ingress 已 join 服务、连接和受管任务；结合 drop sentinel、任务数为零及端口重绑核对资源清理。

同步文件 I/O 的不可抢占性、任意外部扩展的内部不可取消资源以及真实远端请求在取消时的外部副作用仍应如实限定。R03 保持暂停，本轮不触碰音频/执行器、默认政策、执行批准或依赖。

## 新增回归与交付边界

新增 `packages/alia-server/tests/server_runtime/settings_shutdown.rs`，父文件 `tests/server_runtime.rs` 只增加模块声明。`shutdown_drops_pipelined_settings_apply_and_preserves_partial_outcome` 使用既有隔离 PostgreSQL/Runtime fixture；一次 TCP 写入包含完整 PATCH 与下一个 GET，extension 的阶段 semaphore 确认实际 apply 已开始。在停止之前检查 desired 已变、政策版本已真实提交；停止后要求 1 秒内 cleanup 完成、实际 apply future 已 drop、gate 释放、已产生的单次效果/desired/SQL 保留而文件仍为原完整内容、受管任务归零、端口可重绑。该测试不调用外部模型或硬件。

测试失败也会 release 原探针、丢弃 socket、再次请求停止并 await cleanup / runtime join，使未修复基线的 RED 不留下悬挂 owner。1 秒是受控本地 fixture 的回归期限，不提升为生产端到端 SLA；原 Runtime 的 grace/drain 配置未改变。

扩充后的另外两项：

- `client_disconnect_drops_apply_and_same_value_retry_completes`：完整请求已经进入 apply 后真正丢弃 TCP socket，等待服务端 RAII drop 信号，核对 update gate、部分成功与旧文件；服务本身未停止，随后以同值 HTTP PATCH 显式重试，要求再次 apply 且完整保存。这不由客户端请求 task abort 推断取消。
- `shutdown_releases_privacy_gate_while_waiting_for_shared_policy_lock`：测试持有 `interaction_policies` 写锁；真实 Privacy PATCH 在 SQL CAS 完成后投影 registry，此投影作为确定的阶段见证。关闭后在测试仍持写锁时取得释放的 update gate，证明 handler 不可能正常完成其锁等待；核对已提交 SQL / registry 投影保留、旧文件不变，再释放测试写锁并 join cleanup。

Root 已执行扩充命令 `cargo test -p alia-server --test server_runtime settings_shutdown:: --offline --locked -- --nocapture`，当前 macOS **3 passed / 0 failed / 0 ignored**，78 filtered out；测试段 2.23 秒，包含构建的阶段 28.31 秒，退出码 0。日志分别记录真实 disconnect / pipeline shutdown 的 `apply_dropped=1`、`active_tasks=0`、SQL/desired 保留与 file unchanged，以及共享锁情形的 SQL/投影保留、gate 释放。冻结 manifest `2b4524993978bcc30c675b4ffc8bd2b2757e8b2250ebdddbcc912932d9fd105e`，`source_drift=[]`。原始证据为 `run/validation-20261015-settings-waits/green-shutdown.{json,log}`，归档位置由 Root 的本轮证据索引提供。

没有另加以 sleep 推断 HTTP gate 排队的测试：网络写入完成只证明 bytes 交给 socket，不能证明 Auth 已完成或 handler 已进入 gate。**纯 GET/PATCH 等待 update gate 的 HTTP 阶段仍无独立动态见证**。前轮两项 Registry future 取消测试继续覆盖队列与持有者；本轮扩充使用真实 apply Drop 与 SQL 后 registry 投影见证。HTTP/1 本地断开行为不推广为所有代理、HTTP/2 流或任意客户端取消的保证。

## LLM / TTS 独立差异审查

- OpenAI 四个公开 generation 入口各取一次完整 `OpenAiConfig` clone，所选 API family、endpoint、model、key 与 request options 同源；两个 private request helper 使用该 snapshot，不在网络期间取配置锁。Anthropic/Gemini 的两个 helper 覆盖各自四个公开入口，初始化/能力读取等仍是无网络的短 guard。没有发现将 API family 判定与 payload 分成两次配置读取的新竞态。
- LLM loopback fixture 覆盖成功 body、错误 body、headers 与 streaming body 的挂起；一代请求使用旧配置，后续请求使用新配置，取消实际拥有的 generation future 后检查 peer EOF/reset。没有新增 detach task；失败 unwind 也释放 scope 内 future/socket。
- SoVITS 将已有单次权重请求 timeout 包住预算创建、真实全局 `reserve`、send 以及有界错误 body 读取。额度与 response 仍按 RAII 释放，未放大容量、移除背压或新增 quota 接口。两次远端权重步骤仍各自使用现有 `request_timeout_sec`，不把它描述为整个 PATCH 的统一期限。
- SoVITS 本地配置先变更，远端 setup 失败只 warn 并继续返回已消费键，是原有行为；本轮保持该部分成功语义。配额耗尽测试在独立进程中持有真实 reservation，不分配对应音频 bytes，不阻塞同进程其它并行用例；timeout/drop 后重新获得完整容量。

静态复审未发现需修正项。Root 同一冻结源码执行 `--lib providers::`，实际 **51 passed / 0 failed / 0 ignored**，326 filtered out、退出码 0、无源码漂移；本分工已读取对应 `green-providers.{json,log}` 核对，不冒称独立执行。全 Server / feature 检查与总计由 Root 统一记录；这不确认真实模型服务或任意扩展模块的取消安全。

## Root 最终收口补记

本分工完成后统一Server全目标868/0/5通过；随后仅`llm/mod.rs`的cfg格式换行，最终provider51/0/0及无默认all-targets检查通过。源码提交`4e99134`的669个输入匹配final清单；green到final的精确格式转换和统计边界见[Root HANDOFF](root-HANDOFF.md)与[证据索引](evidence-index.json)。本轮隔离数据库已删除，共享服务和运行时保留；没有新增硬件动作。
