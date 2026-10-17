# PN01 HTTP 权限回归 HANDOFF

日期：2026-10-17；负责人：`v01_fixture`；基线 `15604e3`。仅新增 `packages/alia-server/tests/server_runtime/plugin_authorization.rs`、接入对应测试 module，并维护本交接。生产由 `v01_flow` 修改；Cargo／数据库运行与证据由 Root 统一执行，本 agent 未自行运行测试或服务。

## Checklist

- [x] 复用真实 loopback HTTP、隔离 PG schema、原 `with_fixture` 2 槽并发／45 秒外层及 5 秒 HTTP client；未增加依赖或测试框架。
- [x] 未修生产前新增两项负例，Root 首次运行确认 RED 并保留原日志。
- [x] 补齐已约定 7 项 HTTP 场景；仅格式化新增测试文件，源码候选冻结。
- [x] 核对 Root 统一 HTTP GREEN、关联存储回归和资源快照原件；本轮测试交接完成 4/4，候选冻结。

## 原始复现

原件：`run/validation-20261017-pn01/red.log`。本 agent 已只读核对：**0 passed／2 failed／0 ignored／85 filtered，2.44 秒**。两项均在预期的 403 状态断言失败：

- `ordinary_user_cannot_export_all_plugin_owners`：实际 200，`exposed_other=true`；记录 9→9。
- `ordinary_user_cannot_purge_all_plugin_owners`：实际 200，`changed=true`；记录 9→1，仅邻插件记录保留。

测试先采真实响应和 SQL 全记录快照，再断言；panic 被捕获，先取消并 join runtime，再交外层清理 schema／连接及重抛。RED 日志没有用“预计失败”代替实际越权证据。

## 最终候选覆盖

| 场景 | 验证内容 |
| --- | --- |
| 原两项普通用户管理负例 | 全量 export／purge 403、原错误码、不返回他人值、所有 scope／namespace 数据不变 |
| `administrator_retains_global_management_contract` | 四管理入口管理员成功；普通用户 list／usage 拒绝；未认证拒绝；原 usage／export JSON、TTL、collection 与全量 purge、邻插件保留 |
| `personal_storage_is_bound_to_authenticated_user_and_scope` | `/storage/me` 与 `/storage/me/export` 仅 auth.id＋user scope；双方用户分别只能读本人；拒 owner_id／scope query；同 UUID 的 channel／conversation 仍隔离；本人 collection／全量删除包含过期记录，但他人和邻插件保持 |
| `global_export_rechecks_admin_after_storage_wait` | 真实表锁等待后管理员降权，响应 403，无敏感值／数据变更 |
| `global_purge_rechecks_admin_after_storage_wait_without_deleting` | DELETE 真实等待后降权，拒绝且全部原数据保持 |
| `personal_purge_rechecks_active_account_after_storage_wait_without_deleting` | 本人 DELETE 真实等待后账号失活，拒绝且全部原数据保持 |

等待场景持有 `plugin_storage_records` 表锁，通过同 blocker PID 的 `pg_stat_activity`／`pg_blocking_pids` 观察已认证 HTTP 请求实际到达存储等待，再变更账号并释放锁；不用 sleep 猜测到达阶段。失败收尾先释放 blocker、join 请求和 runtime，再由原 fixture 清理数据库。

## 结果与边界

HTTP GREEN 原件：`run/validation-20261017-pn01/green-http.log`，本 agent 已只读核对为 **7 passed／0 failed／0 ignored／85 filtered，3.32 秒**。三项等待均 `observed=true`、`request_finished=false`，撤权后均 403、`exposed_other=false`、`changed=false`；原两项 RED 的请求均改为 403，完整记录均 9→9。管理员原契约和本人隔离两项同时通过。

关联存储原件：`run/validation-20261017-pn01/green-storage.log`，本 agent 已只读核对为 **4 passed／0 failed／0 ignored／0 filtered，0.07 秒**，包括原 `postgres_plugin_storage_round_trips_records_and_policy`。Root 明确设置 `ALIA_TEST_DATABASE_URL` 指向本轮真实任务 PG，未以缺失数据库跳过该用例。

资源原件：`run/validation-20261017-pn01/resources-before-stop.json`，本 agent 已核对 `fixture_schemas=0`、`other_database_clients=0`。Root 已确认随后任务容器停止成功，`--rm` 后按任务名查询为空；本 agent 未自行启动或操作容器。

本轮保留原 RED 与两个 GREEN 原件，不扩大到全 workspace；HTTP GREEN 后源码保持冻结。测试交接完成 **4/4**，正常提交及源码身份由 Root 统一归档。

本轮不验收第三方安装／隔离后端、生命周期、执行器效果或硬件；不声称完整插件系统验收完成。既有 Execution owner 批准不修改；默认 shadow／当前政策及 R03 暂停保持。
