# Settings authorization — 复现与回归 HANDOFF

最终检查项 **8/8 完成**；以下过程说明保留各实施时点，最终结果以文末 Root 验收记录为准。
日期 2026-10-12；分工 `settings_auth_reproduce`。只修改 `tests/server_runtime/settings_authorization.rs`，沿用已有 `server_runtime.rs` module 声明。本分工未运行 Cargo、数据库、Docker 或 Git；全部执行结果、冻结源码及提交以 Root 证据为准。

## 进度与 checklist

- [x] 读已有未提交测试、RED V1/V2 原始日志与结果、独立 review 和本轮 checklist。
- [x] 区分 V1 见证失败与 V2 真正业务复现，保留原用例及全部失败证据。
- [x] 保留真实 HTTP / PostgreSQL policy 行锁回归，补上两次 embedding preview 的精确暂停见证。
- [x] 覆盖降权、停用、删除、核验数据库错误／超时、成功与同值重试、准入后降权语义。
- [x] 审阅失败时释放 gate、回滚表锁、恢复表名、有界 join request/runtime；完成局部 rustfmt。
- [x] Root 完成针对性 GREEN 编译运行；修复独立审查发现的失败清理问题。
- [x] Root 冻结同一最终源码并完成 macOS / Linux 验证和资源清理。
- [x] Root 完成交付证据、检查点与正常提交。

本稿进度 **6/8**；本轮新增授权测试共 **8 项**，V1 针对性验证已实际通过 **8 / 0 / 0**；失败清理小修正后的 V2 全量验证待 Root 完成。

## RED 证据与原因

RED V1（manifest `a71d23e8a563ea75c27f9cabb0c206dafe0165710d681e8202d8b6525e1c1879`）退出 101，`0 passed / 1 failed`；失败发生在 policy 行锁等待见证，不是业务授权断言。不可据此宣称已捕获越权。

RED V2（manifest `17610fb468942b964b2b56eeb4fa2e1986164b5eb4543f8dfae00756ffcfa19f`）在**生产源码未修复**时退出 101，`0 passed / 1 failed`。日志明确记录：

```text
[settings-auth-witness] reached=true polls=2 blocked=1 policy_blocked=1 policy_reads=2 policy_active=1 policy_locks=1 barrier_idle=1 gate_held_seen=true request_finished_seen=false
[settings-auth-wait] status=200 error_code=none json_body=true desired_before=Some(Number(8)) desired_after=Some(Number(9)) live_before=8 live_after=9 file_changed=true module_applies=1
```

HTTP 请求通过入口认证，持有 settings gate，SQL policy 读取等待 barrier；测试在其他连接提交 actor 降权后释放 barrier。旧实现返回 200 且 desired/live/file/module 都改变，403 断言失败。V2 仅将本地 HTTP client 初始化移出屏障期限并补充安全数值诊断，没有放宽原 policy 2 秒超时。

原 policy 见证依赖同 fixture 中真实 SQL blocker 与 settings gate，未将 waiter PID 唯一绑定 handler。新增 `GatedEmbedding` 利用已有 `ServerBuilder::with_embedding`，在构建完成后 arm 第一次或第二次 preview，以 Semaphore 证明该请求已经到达该准确阶段；不使用 sleep，不修改生产插桩，不调用 embedding 服务。

## GREEN 与独立审查

Root 的 V1 定向命令 `cargo test -p alia-server --test server_runtime settings_authorization --offline --locked -- --nocapture`，manifest `d11c9b111c5177a46c332d8877126170bf3188feeae83dbff2a1ec16bf22fa1d`，退出 0，无源码漂移，耗时 43.89 秒，实际 **8 passed / 0 failed / 0 ignored**。原 RED 此次成为 403，desired/live 保持 8，file_changed=false，module_applies=0；fresh 数据库错误和超时均为 503，后者记录 2003 ms；准入后降权和有效 admin 为 200。各拒绝用例均记录 desired/live/file 无变化且模块未应用。

独立 review 发现：新 helper 若 HTTP JoinHandle 已完成但内部 panic，直接 unwrap 可能使外层 cleanup 再次 poll 已完成句柄。V1 运行结束后仅修正该测试清理点：timeout 返回 Ok 时先从 Option 移出已完成句柄，再检查内部结果；真正 timeout 保留句柄给有界 abort/join。原 RED 先将句柄移到局部，没有同类问题。本修正不改生产行为、测试准入断言或 timeout；Root 将以 V2 全量验证覆盖，V1 与 V2 不混记。

## 回归矩阵与边界

| 测试 | 见证与期望 |
| --- | --- |
| `non_policy_patch_rechecks_admin_after_waiting_for_policy` | 原 RED：policy 行锁期间降权；403，desired/live/file 不变，模块未应用 |
| `demotion_during_first_preview_prevents_non_policy_settings` | 第一 preview 精确屏障降权；403，无请求设置副作用 |
| `disabling_during_second_preview_prevents_non_policy_settings` | 第二 preview 精确屏障停用；403，无请求设置副作用 |
| `deletion_during_second_preview_prevents_non_policy_settings` | 第二 preview 精确屏障删除；403，无请求设置副作用 |
| `fresh_account_lookup_error_is_unavailable_without_settings_changes` | 第二 preview 后仅本 fixture 临时 rename users；最终核验 DB 错误为脱敏 503，恢复表名后验证无请求设置副作用 |
| `fresh_account_lookup_timeout_is_bounded_without_settings_changes` | 第二 preview 后锁本 fixture users 表，观察 fresh SELECT 的真实 Lock waiter；原 2 秒期限返回 503，响应后才释放锁，无请求设置副作用 |
| `active_admin_patch_and_same_value_retry_do_not_advance_policy` | admin 保持有效，第二 preview 后成功；同值重试再次 apply，政策版本和内容始终不变 |
| `demotion_after_apply_admission_does_not_claim_cross_module_rollback` | 已进入受控模块 apply 后才降权；已准入请求完成原应用／保存，不承诺持续撤销或跨模块回滚 |

受控 probe 仅报告 TOOL_LIMIT，测试日志只含动作名、HTTP 状态、受控数值变化旗标和计数；不输出 JWT、密码、完整响应或配置文件内容。错误响应断言沿用 `AUTH_FORBIDDEN` / `SERVICE_UNAVAILABLE`，DB 错误消息不暴露 schema、用户 UUID 或表名。当前 fixture 无到期 pause，因此可以核对 policy 版本及内容不变；不把这外推为 `current_policy()` 自动恢复到期 pause 永不发生。

测试明确区分准入前拒绝与准入后部分成功：后者仍不跨 SQL、模块及文件整体回滚。生产 actor 检查和政策 CAS 仍由原 context policy 分支负责，本轮不修改它。

## 未扩展范围

没有加入“仅等待 settings update gate”的独立动态用例：现有公开接口没有能可靠确认另一 HTTP 请求已完成入口认证并排入 mutex 的见证，sleep 或不区分 fixture 的 SQL query 计数会给出虚假覆盖。实际 policy / preview 用例均确认更新 gate 已被持有，最终检查的静态位置位于取得 gate 后的全部准备等待之后。若后续增加独立 gate 排队测试，应先提供可靠、局部的入队见证，不能降低本轮断言。

GET Settings、其他入口的相似等待窗口、持续授权租约、跨域回滚和真实硬件均不属于本轮测试。Root 完成最终验证后更新本文通过数和归档路径。

## Root 最终验收补记

源码 `4553e88138cb91af46f024b343baaa3857c8d769`；最终 V2 manifest `1f88c037cc584c41b88efba5141858930926b1c066f2b0b881e7579e490a90e1`，654/654 源码 Git blobs 一致。

macOS ARM64 workspace **1128/0/5**；Linux ARM64 Server 默认完整回归 **846/0/5**。两端 Server 无默认 feature 全目标检查、架构边界正反自测、变更 Rust 格式、差异和计数器 8 例自测通过。数字为 passed/failed/ignored，嵌套子进程摘要不重复计入。全部最终阶段同源且无漂移，保持默认测试并发和原期限。新增 8 项身份回归均在两端全量中通过，V1 定向 8/0/0 不额外加到全量数字。

RED 首次屏障失败与 V2 有效越权复现分别保留；GREEN V1 定向通过后，仅修复测试 panic 清理再冻结 V2。Linux 首次准备缺少空目录，复制源码和启动容器前失败；补目录后准备成功，未变更生产源码。最终 Cargo 阶段均通过。

全部 Cargo 已结束，本轮 PostgreSQL 和 Linux holder 已停止。关闭前其他 DB 连接 0、测试 schema 0，遗留 schema 清理 0 个；用户 5433 未访问。原日志、源码快照与缓存留在 ignored `run/validation-20261012-settings-authorization/`，可核对副本及哈希见证据索引。空闲 holder 停止码不是 Cargo 结果。

本轮总 checklist **10/10**，L09—L12 **4/4**，R01—R06 仍 **2/6**，不是项目完成百分比。前文记录实施时点，已由本节关闭最终验证、资源、源码提交待项。

未独立动态验证纯 update gate 排队或慢 JSON body 窗口；最终核验位置由静态顺序与真实 policy/preview 等待验证。未运行真实 PTY/Easy-Turn、Linux 全 workspace、release/部署 smoke、外部付费服务、Pi/声学/执行器或长稳。最终核验是一次准入，准入后的降权不追溯撤销已开始的更新。
