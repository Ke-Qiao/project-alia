# 2026-10-12 Settings read authorization — 准入流程 HANDOFF

最终检查项 **8/8 完成**；以下保留各实施时点，最终结果以文末 Root 验收记录为准。
基线 `b15fa7d`，沿用 `codex/architecture-refactor`。本分工 `settings_auth_fix` 只拥有 `packages/alia-server/src/api/settings.rs` 和本文；另两名 subagent 负责真实 HTTP/PG 回归与独立审查，Root 统一运行、源码冻结、资源关闭、证据和提交。

## 当前进度与 checklist

- [x] 阅读上一轮 Settings PATCH 身份重验证的四份 HANDOFF。
- [x] 审计 GET 的全部等待、Registry 读取、响应/ETag 构造和现有调用者。
- [x] 报告最后准入位置、复用方案及拒绝边界，Root 与独立审查确认。
- [x] Root 在生产源码未改时完成有效 RED 业务复现并授权修改。
- [x] 将既有 PATCH 查询原样提取为同文件私有 helper，GET 在最后数据收集等待后调用。
- [x] scoped 格式检查和独立生产审查通过。
- [x] 最终回归及既有 PATCH 行为由 Root 同源验证。
- [x] Root 完成双平台验证、资源清理、交付文档和正常提交。

当前是生产修复完成时点 **6/8**。本分工只修改指定生产文件和本文，已通过 `rustfmt --edition 2024 --check packages/alia-server/src/api/settings.rs`，独立审查读回无阻断发现。没有运行 Cargo、数据库、Docker 或 Git，后续 GREEN/全量结果以 Root 实际执行和本文件补记为准。

## 现有能力与读取缺口

入口 `AuthUser::from_token` 会校验 JWT 并在 2 秒内从数据库读取当前 active 账号，handler 随后要求 admin；角色来自数据库而非 JWT 的 role claim。这些能力仍有效，不需更改认证中间件或重写账户类型。

原 GET 顺序为：入口管理员快照 → `settings.lock_updates().await` → `current_policy().await` → SQL 政策投影 `batch_update().await` → 按 prefix 的 `list().await` 或全量 `all().await` → 同步 JSON 序列化和 ETag → 返回。上述 gate、数据库、Registry 写锁/读锁都可能等待；入口 AuthUser 不会随等待期间已提交的降权、停用或删除自动更新。

Registry 的 `list`/`all` 在读锁内克隆 `SettingEntry`，返回真实 value 和原 schema；`schema.secret` 是 UI 元数据，不执行 HTTP 值遮罩。注册项包括 database URL 和 provider API key，因此旧快照越权会导致敏感配置被返回。前端 `useSettings(prefix?)` 直接消费这些原字段，本轮继续保持管理员读取完整值的 API 契约，不擅自改变遮罩、prefix 或字段设计。

## 最后准入点与最窄复用

GET 现在在 `entries` 已完成 `list` 或 `all` 的最后一次 await 后，`Json(entries).into_response()` 前执行 fresh active-admin 查询；其后只有同步 JSON/ETag 构造和返回，没有新的 handler await。仅在 gate 或 SQL current 后核验仍会漏掉后续 Registry 等待，所以不采用提前核验。

上一轮 PATCH 的查询已经满足所需边界：2 秒 timeout 覆盖连接池/查询等待，SeaORM 只选择符合 actor id、`status='active'`、`role='admin'` 的 `users.id`；无匹配返回 403，超时和查询错误返回固定脱敏 503。已原样移动为同文件私有 `require_current_admin(state, auth) -> Result<()>`，GET 和纯非政策 PATCH 各调用一次；helper 只在原查询末尾添加成功的 `Ok(())`，不新建通用授权框架、依赖、配置或后台任务。

PATCH 调用保持在原非政策分支和全部 preview 后；原错误文本、2 秒期限、政策/embedding 的事务 actor/CAS，以及准入后的模块应用/保存顺序保持。helper 不被政策分支用来替换原 actor `FOR SHARE` 与 CAS，也不会持有用户行锁跨越响应或模块。

## API、错误和拒绝范围

| 条件 | 保持的行为 |
| --- | --- |
| 入口坏/过期 JWT、缺失或 inactive 账号 | 原 401，不改中间件 |
| 入口普通用户 | 原 403，不扩大读取权限 |
| 最后查询发现缺失、停用或降权 | 403 / `AUTH_FORBIDDEN`，不返回 entries 或 ETag |
| 最后查询超时 | 503 / `SERVICE_UNAVAILABLE`，固定 `settings authorization timed out; retry explicitly` |
| 最后查询失败 | 同上 503，固定 `settings authorization unavailable; retry explicitly`；内部仅记录数据库 error |
| 最后查询仍为 active admin | 原全量/prefix Settings JSON、schema 和 quoted policy ETag |

响应不包含查询 SQL、actor、token、实际设置内容或文件路径的错误诊断。测试应使用不可用的受控 secret sentinel，并仅打印是否暴露的布尔值，不能输出真实凭据或完整敏感响应。

GET 拒绝保障是“不交付敏感 entries 和 ETag”，不是“此前所有状态完全不变”：`current_policy()` 仍可按数据库时钟恢复到期 pause、推进 SQL 版本，GET 原有 SQL 政策 RAM 投影也已经执行。二者不是请求携带的新设置，本轮不撤销或移除。测试的无 pause fixture 可对照政策不变，但不能外推为所有拒绝都没有 SQL 或 RAM 变化。

最终查询是一份准入证据，不是连续撤权租约。它之后发生的降权不会回收已经交付的响应，网络发送本身也不由用户行锁覆盖；不新增持续重新验签、跨模块回滚或长期事务。Settings PATCH 原部分成功与显式重试保持。

## 后续验证与暂缓

Root 已先完成真实 policy 等待期间降权的 RED，再授权生产编辑。本轮 RED manifest `e9dbd269854da9e4562a48d763e1875380862dd83c52130c539983a435842161`、655 输入，结果 `source_drift=[]`，19.53 秒，退出 101。原 GET 已通过入口认证、持有更新 gate 且在 policy 行锁等待；另连接提交降权后放行，实际仍返回 HTTP 200、`has_etag=true`、`sentinel_exposed=true`、`file_changed=false`，随后 403 断言失败。这是有效读取越权业务复现；原日志不因后续修复覆盖。

最终回归应分别覆盖全量与 prefix、降权/停用/删除、最终查询错误/超时、仍有效管理员和旧 PATCH。GET 没有 provider preview 接口；没有可靠独立见证的 Registry/gate 等待不能用 sleep 假称精确覆盖，最后查询位置须同时由静态调用顺序核实。

本轮不扩展其他管理 API、JWT 等待期间到期重验、权限持续撤销、响应字段遮罩政策、硬件或平台范围。实际 Cargo 数字、冻结源码和提交由 Root 完成后追加。

## Root 最终验收补记

源码 `d080710052ae308b962b826d0f096fb10067469e`；最终 V3 manifest `48547827ad52952af80c9275cc83dd35ef9da5a25cb58b7fb32e8cab57fc1c7c`，655/655 源码 Git blobs 一致。

macOS ARM64 workspace **1134/0/5**；Linux ARM64 Server 默认完整回归 **852/0/5**。两端 Server 无默认 feature 全目标检查、架构边界正反自测、变更 Rust 格式、差异及计数器 8 例自测通过。数字为 passed/failed/ignored，嵌套子进程摘要不重复计入。全部最终阶段同源且无漂移，原期限与默认测试并发保留。新增 GET 6 项及既有 PATCH 8 项均在两端全量中逐项通过；定向 V3 的 6/0/0 不额外计入总数。

RED 在未改生产时有效复现降权后的 200/ETag/假密钥泄露。GREEN V1 为 5/1，V2 保留原断言的诊断单例为 0/1；166 个键中 3 个浮点 value 与直接内存值比较不同，但期望按 HTTP 同样 JSON 编码解码后完整严格相等。V3 只修正测试比较层次，不用容差或删除字段，生产自 V1 起未变。Linux V1 仅准备环境，未执行 Cargo；Linux 最终阶段明确使用 V3。失败、诊断、原 RED 测试全文和各 manifest 均保留。

全部 Cargo 已结束，本轮 PostgreSQL 及 Linux V1/V3 holder 均已停止。关闭前其他 DB 连接 0、测试 schema 0，遗留 schema 清理 0 个；用户 5433 未访问。原日志、源码快照与缓存留在 ignored `run/validation-20261012-settings-read-authorization/`，可核对副本与哈希见证据索引。空闲 holder 停止码不是 Cargo 结果。

本轮总 checklist **10/10**，L09—L12 **4/4**，R01—R06 仍 **2/6**，不是项目完成百分比。

未独立动态注入纯更新 gate 排队或 Registry 私有锁等待；最后核验位置由完整 await 静态审查和真实政策等待验证。未运行真实 PTY/Easy-Turn、Linux 全 workspace、release/部署 smoke、外部付费服务、Pi/声学/执行器或长稳。核验是一次响应准入，不承诺后续持续撤权或撤回已发送响应。
