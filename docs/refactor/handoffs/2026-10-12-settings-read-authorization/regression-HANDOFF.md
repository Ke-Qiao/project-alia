# Settings read authorization — 复现与回归 HANDOFF

最终检查项 **6/6 完成**；以下保留各实施时点，最终结果以文末 Root 验收记录为准。
日期 2026-10-12，基线 `b15fa7d`。分工 `settings_auth_reproduce`，仅拥有 `tests/server_runtime/settings_read_authorization.rs`、其 parent module 声明及本文；生产修改、Cargo、数据库、Docker、Git 和冻结验证由 Root 负责。

## 进度

- [x] 读取现有 GET/PATCH、registry、真实 HTTP/PG fixture 与上轮恢复检查点。
- [x] 准备单一 RED，生产源码未修改时交 Root 执行。
- [x] Root 记录实际 RED 及敏感响应暴露证据。
- [x] 扩展降权、停用、删除、核验失败／超时及原 API 正常响应回归。
- [x] 独立审查并完成定向 GREEN。
- [x] 同一冻结源码完整验证、资源关闭及正常提交。

当前 **4/6**。RED 已实际确认；GREEN V1 定向 **5 passed / 1 failed / 0 ignored**，正常管理员完整 JSON 对比失败，拒绝五项通过；V2 单例已证实是期望值与 HTTP parsed JSON 的浮点 wire roundtrip 边界不一致，V3 已修正测试完整期望，等待 Root 验证。

Root 已冻结 RED 源码，共 655 输入，manifest `e9dbd269854da9e4562a48d763e1875380862dd83c52130c539983a435842161`，并保留原始测试全文及生产源码相对基线零差异证据。后续 GREEN 计划为六项：policy 等待期间降权、停用、删除；入口认证完成后注入最终账户查询错误／真实表锁超时；有效管理员 all/prefix/ETag/secret schema 合同不变。成功项也覆盖原入口无 token 401 和普通用户 403。计划不等于执行结果。

Root RED 实际退出 101，耗时 19.53 秒，**0 passed / 1 failed / 0 ignored**，无源码漂移：

```text
[settings-read-witness] reached=true polls=1 policy_waiters=1 gate_held=true request_finished=false
[settings-read-result] status=200 has_etag=true sentinel_exposed=true file_changed=false
```

这是到达业务断言的有效失败：管理员降权提交后，旧实现仍返回敏感设置及 ETag。原失败日志、结果和原测试源码全文由 Root 保存在 `run/validation-20261012-settings-read-authorization/`，不以后续 GREEN 覆盖。

## GREEN 回归矩阵

| 用例 | 核心验证 |
| --- | --- |
| `settings_get_rechecks_admin_after_policy_wait_before_returning_secrets` | 原 RED；降权后 403，仅原错误 envelope，无设置／sentinel／ETag |
| `settings_get_rejects_disabled_account_after_policy_wait` | 入口已过后停用；403，无设置／sentinel／ETag |
| `settings_get_rejects_deleted_account_after_policy_wait` | 入口已过后删除；403，无设置／sentinel／ETag |
| `settings_get_account_lookup_error_returns_no_settings_or_etag` | 入口已过后临时 rename 本 fixture users；最终查询错误 503，恢复表名后校验 |
| `settings_get_account_lookup_timeout_is_bounded_and_returns_no_settings_or_etag` | 入口已过后 users 表 ACCESS EXCLUSIVE，最终核验实际 SQL Lock waiter；原 2 秒超时 503，响应后才释放锁 |
| `settings_get_preserves_admin_json_secret_prefix_and_etag_contracts` | 有效 admin 保留完整 map/entry/secret schema 和未遮罩值、quoted ETag；namespace、尾点、精确键、空及未知 prefix 与现 registry 合同一致；原入口无 token 401、user 403 均无设置／sentinel／ETag |

共六项共享一个小 fixture driver，未新增生产测试钩子或第三方依赖。所有响应对象和文件比较都采用固定文案的布尔断言，失败时也不输出敏感正文；HTTP 状态、版本号、布尔旗标和耗时可以进入日志。成功响应按原合同保留敏感字段，不引入遮罩或前端变化。

## GREEN V1 失败定位记录

V1 manifest `aa52f64b79e6ec2fd82bb79d1e1828066e3c0c954c4138d92ebb5c95d13aec36`，Root 执行六项定向测试，耗时 41.22 秒，退出 101，无源码漂移。降权／停用／删除为 403，查询错误／超时为 503；均无 ETag/sentinel，timeout 为 2006 ms。唯一失败是正常管理员通过 secret schema、200 和 ETag 检查后，完整 parsed JSON 与直接 `serde_json::to_value(registry)` 的相等断言失败。此时不能将六项记为全过。

生产 GET 仍直接 `Json(entries)`，静态未发现 fixture 中其他运行期 registry 写入，但这不足以直接认定原因。V2 仅增加失败时的有界脱敏诊断，保持全部断言：key 数、至多 16 个受控 setting 路径、value/schema 相等布尔和 JSON 类型，以及将期望值 serialize→parse 后是否与 HTTP parsed JSON 相同。绝不打印 entry 内容、schema 默认值、数值或 sentinel；未修改生产代码，等待 Root 单例实测再决定正确边界比较。

V2 manifest `2e2a4a5dcb0a6fd5bd7744d14f91b66c9a998b416e377fec976b29bf633a92c8`，Root 单例退出 101，耗时 11.24 秒，无源码漂移，原相等断言仍失败。诊断实证为 actual/expected 各 166 keys，仅 `core.policies.affect.fatigue_per_turn`、`core.policies.cache.semantic_threshold`、`core.providers.gpt_sovits.min_fragment_interval` 三个 float value 不同，三者 schema 相同；`serialized_expected_matches=true`，三项 `serialized_entry_matches=true`。因此问题是测试将直接内存 `to_value` 与经过 JSON 编码／解析的 HTTP map 比较，而非 GET 丢字段、registry 漂移或生产授权修复变更数据。

V3 只把 all/prefix 期望也通过 `to_vec(registry entries) → from_slice<Value>` 后与 HTTP parsed JSON **完整相等比较**，保留所有字段、数值、secret/schema 及过滤检查。未加浮点容差，未跳过失败字段，未改生产序列化或新增 feature/依赖；删除临时诊断 helper 并留一行说明比较同一 wire 边界。V1/V2 原失败及诊断证据继续保留，V3 未验证前不报告通过。

## RED 设计与不泄露诊断

真实 runtime 建立管理员账号，将 fixture 已有 provider API key 替换为随机、不可用于任何真实服务的 sentinel；保留原 schema.secret。预先保存文件，独立事务锁 context_policy，发起 GET，观察指向该事务的真实 policy SQL Lock waiter 与本 registry update gate 被持有。后者证明请求已经通过入口认证。随后另一连接提交管理员降权，再释放 policy barrier。

预期为原 403 error envelope，没有 ETag，响应中不含 sentinel，GET 不保存文件。旧实现预期返回 200 和完整敏感设置；日志只报告 `sentinel_exposed`、`has_etag`、文件变化布尔及 HTTP 状态，绝不打印 sentinel、JWT、全响应或配置快照。所有可能包含 sentinel 的比较使用布尔断言和固定错误文字，禁止 `assert_eq!` 打印实际对象。

失败清理先回滚 policy transaction，再有界 join 请求，最后 cancel/join runtime。JoinHandle 完成时先从 Option 移出后再检查内部结果，避免 assertion/panic 清理二次 poll；真正 timeout 的句柄留给 abort/join。

## 最终位置与证据边界

GET 在入口认证后会等待 update gate、current_policy、projection 的 registry 写锁、list/all 的读锁。最终 fresh active-admin 检查必须位于 entries 收集完成之后，且其后到 Json/ETag 返回只执行同步代码。当前 registry 内部 RwLock 没有公开可控等待钩子；本回归文件不能为此修改生产接口或用 sleep 假装精确见证。policy barrier 可以证明原缺陷，最终 registry await 之后的位置另由静态审查保证，不虚称每个等待都已被单独动态验证。

GET 原有 current_policy 到期恢复及 registry 政策投影可能产生 SQL/RAM 变化，不将最终拒绝描述成整个 GET 无任何副作用。本 fixture 无到期 pause，才对照 policy 版本和内容不变。初始认证的 401/403、正常管理员原 secret 字段和 prefix/ETag API 都继续保留。

## Root 最终验收补记

源码 `d080710052ae308b962b826d0f096fb10067469e`；最终 V3 manifest `48547827ad52952af80c9275cc83dd35ef9da5a25cb58b7fb32e8cab57fc1c7c`，655/655 源码 Git blobs 一致。

macOS ARM64 workspace **1134/0/5**；Linux ARM64 Server 默认完整回归 **852/0/5**。两端 Server 无默认 feature 全目标检查、架构边界正反自测、变更 Rust 格式、差异及计数器 8 例自测通过。数字为 passed/failed/ignored，嵌套子进程摘要不重复计入。全部最终阶段同源且无漂移，原期限与默认测试并发保留。新增 GET 6 项及既有 PATCH 8 项均在两端全量中逐项通过；定向 V3 的 6/0/0 不额外计入总数。

RED 在未改生产时有效复现降权后的 200/ETag/假密钥泄露。GREEN V1 为 5/1，V2 保留原断言的诊断单例为 0/1；166 个键中 3 个浮点 value 与直接内存值比较不同，但期望按 HTTP 同样 JSON 编码解码后完整严格相等。V3 只修正测试比较层次，不用容差或删除字段，生产自 V1 起未变。Linux V1 仅准备环境，未执行 Cargo；Linux 最终阶段明确使用 V3。失败、诊断、原 RED 测试全文和各 manifest 均保留。

全部 Cargo 已结束，本轮 PostgreSQL 及 Linux V1/V3 holder 均已停止。关闭前其他 DB 连接 0、测试 schema 0，遗留 schema 清理 0 个；用户 5433 未访问。原日志、源码快照与缓存留在 ignored `run/validation-20261012-settings-read-authorization/`，可核对副本与哈希见证据索引。空闲 holder 停止码不是 Cargo 结果。

本轮总 checklist **10/10**，L09—L12 **4/4**，R01—R06 仍 **2/6**，不是项目完成百分比。

未独立动态注入纯更新 gate 排队或 Registry 私有锁等待；最后核验位置由完整 await 静态审查和真实政策等待验证。未运行真实 PTY/Easy-Turn、Linux 全 workspace、release/部署 smoke、外部付费服务、Pi/声学/执行器或长稳。核验是一次响应准入，不承诺后续持续撤权或撤回已发送响应。
