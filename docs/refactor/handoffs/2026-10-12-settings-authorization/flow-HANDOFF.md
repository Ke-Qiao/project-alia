# 2026-10-12 Settings authorization — 准入流程 HANDOFF

最终检查项 **8/8 完成**；以下过程说明保留各实施时点，最终结果以文末 Root 验收记录为准。
基线 `bdc94dd245251a45b620877be728a7635d3e1296`，沿用 `codex/architecture-refactor`。本分工 `settings_auth_fix` 负责 `packages/alia-server/src/api/settings.rs` 的纯非政策修改准入；认证入口、SQL 政策事务、Registry、provider 和保存流程均保持。Root 统一运行、冻结源码、核对证据和提交，另两名 subagent 负责真实回归与独立审查。

## 当前进度与 checklist

- [x] 审计入口 AuthUser、更新 gate、SQL current、两次 embedding preview 和所有请求设置副作用。
- [x] 核对已有 SQL policy actor/CAS、所有设置保存调用者和部分成功语义。
- [x] 先报告准确缺口及修复位置，等待 Root 确认未改生产源的 RED 业务复现。
- [x] 在纯非政策分支加入最后的 2 秒有界 active admin 查询。
- [x] 保持 API、原认证错误、政策事务、模块应用与保存语义；限定查询及错误诊断内容。
- [x] `rustfmt --edition 2024 --check packages/alia-server/src/api/settings.rs` 通过；停源交独立审查。
- [x] 独立审查读回生产修改，无阻断发现。
- [x] Root 完成最终回归、双平台验证、资源清理与提交证据闭环。

本稿为生产修复完成时点 **7/8**。本分工没有运行 Cargo、数据库、Docker 或 Git，不声称尚未发生的 GREEN 或全量结果。Root 第二轮 RED 已核对 manifest 和原生产源码，业务缺陷证据成立后才授权本分工编辑。

## 既有能力与真正缺口

`AuthUser::from_token` 已有 2 秒读取当前账户的核验，角色来自数据库，不是 JWT 内的 role claim。缺失或 inactive 账户在入口返回原 401，进入 handler 后非 admin 返回原 403。无需重写这些能力或新建通用授权框架。

`update_settings` 还会等待同实例更新 gate、SQL current，以及当前和下一份 embedding profile 的两次纯预览。AuthUser 是进入请求时的身份快照；纯非政策分支原来直接使用 current snapshot，不调用 SQL policy CAS，因此在上述等待期间已提交的降权、停用或删除可以被漏过。

有政策键或 embedding 身份变化时，原 `commit_policy_update` 在 2 秒范围内调用 `replace_for_actor_with_embedding`。SQL 事务内当前 active admin 的 `FOR SHARE` 检查和政策版本 CAS 已是该分支的准入点，不能以提前的普通查询替换或弱化它。显式 embedding reaffirm 和其 profile descriptor/pins 仍走这个原分支。

## 实现与影响范围

唯一生产文件为 `api/settings.rs`。纯非政策 `else` 分支在全部准备/预览等待结束后，首次 `SettingsRegistry::batch_update` 之前，通过现有 SeaORM 查询构建器读取 `users.id`；条件为本请求 actor id、`status='active'`、`role='admin'`。查询包含连接池等待在内受 2 秒 `tokio::time::timeout` 限制。

查询只读取 id，不读取账户密码、邮箱或姓名，不新建公开 helper、身份结构、依赖、feature 或配置项。它不持有显式 SQL 事务或行锁跨越 Registry、模块 apply、InteractionPolicies 和文件保存。

| 最终准入结果 | HTTP 与错误处理 |
| --- | --- |
| 查询未找到当前 active admin（降权、停用或删除） | 原 `AppError::Forbidden`：403 / `AUTH_FORBIDDEN` |
| 查询超时 | 原 `AppError::Unavailable`：503 / `SERVICE_UNAVAILABLE`；固定消息 `settings authorization timed out; retry explicitly` |
| 查询失败 | 同上 503；固定消息 `settings authorization unavailable; retry explicitly`；内部仅记录数据库 error，不记录 AuthUser、token、响应或文件内容 |
| 仍是 active admin | 继续既有 desired → modules → interaction policies → 原子保存流程 |

不改成功响应字段、原 `If-Match` 语法/错误顺序、400/409、HTTP 路由、入口 JWT 检查或 SQL 政策提交。没有请求政策键的空 body 也经过新检查，因为该路径随后仍会投影当前 SQL 政策到 Registry。

原注释“所有 process-local 变化均在 policy CAS 后”对纯非政策分支并不准确；已收窄为政策投影必须使用已提交的 SQL 政策，而非以本地旧 RAM 覆盖政策。

## 准入、拒绝与取消的精确边界

保证：在最后查询的读取快照中已可见的降权、停用、删除会拒绝；核验失败或超时也拒绝。上述拒绝不会进入本请求的 desired 更新、module apply、interaction policy 更新或 settings 文件保存。

不把最终查询误称为持续授权租约。它之后发生的降权不会倒转已准入操作；Registry 锁、apply 或文件保存的后续等待仍遵循既有部分成功/显式重试语义。新的同值重试是新请求，必须重新经过入口和最终核验。取消请求也不代表已调用模块被撤销。

`ContextPolicyStore::current()` 本身可以按数据库时钟恢复已到期 pause 并推进政策版本；这是此前即存在的独立政策转换。最终拒绝的“无副作用”专指没有应用本请求携带的设置，不能承诺此前 current 绝无 SQL 写入。

本轮不扩大为 JWT 等待期间到期的重新验签、不覆盖 Settings GET 的相似读取窗口、不改其他管理 API、不新增跨 SQL/模块/文件回滚或长期 users 行锁。上述边界保持当前政策、工具可见性和执行批准，不扩大平台或硬件动作。

## RED 与后续验证交接

Root 保留两轮 RED：第一轮未在原见证期限内到达 policy barrier，仅是见证失败，不能当业务复现；第二轮将 reqwest 客户端准备移出见证区，保留原生产期限与断言。第二轮在 policy 行锁实际等待、settings gate 已占用且请求未结束后，另连接提交降权，再放行；原实现仍返回 200、desired/live 从受控值 8 变为 9、文件字节改变、受控 module apply 一次，因预期 403 而失败。

第二轮 RED manifest 为 `17610fb468942b964b2b56eeb4fa2e1986164b5eb4543f8dfae00756ffcfa19f`，654 个输入，Root 已核对 `source_drift=[]` 及生产 src 相对基线无修改。原日志和失败记录应保留，不能由后续 GREEN 覆盖。

回归分工已收到实际查询位置、错误码和超时语义。最终计划包含真实 HTTP 的 policy/gate/受控 preview 等待、降权/停用/删除拒绝、核验错误/超时、仍具权限的成功及同值重试，以及准入后才降权的既有部分成功边界。政策与 embedding 原 actor/CAS 回归仍需保留；最终执行和计数以 Root 证据为准。

未运行真实硬件、外部模型或全平台测试；本分工只做源码、调用链和格式检查。最后准入之后的并发降权仍不撤销已准入操作，这是明确保留的范围，而非跨域原子性保证。

## Root 最终验收补记

源码 `4553e88138cb91af46f024b343baaa3857c8d769`；最终 V2 manifest `1f88c037cc584c41b88efba5141858930926b1c066f2b0b881e7579e490a90e1`，654/654 源码 Git blobs 一致。

macOS ARM64 workspace **1128/0/5**；Linux ARM64 Server 默认完整回归 **846/0/5**。两端 Server 无默认 feature 全目标检查、架构边界正反自测、变更 Rust 格式、差异和计数器 8 例自测通过。数字为 passed/failed/ignored，嵌套子进程摘要不重复计入。全部最终阶段同源且无漂移，保持默认测试并发和原期限。新增 8 项身份回归均在两端全量中通过，V1 定向 8/0/0 不额外加到全量数字。

RED 首次屏障失败与 V2 有效越权复现分别保留；GREEN V1 定向通过后，仅修复测试 panic 清理再冻结 V2。Linux 首次准备缺少空目录，复制源码和启动容器前失败；补目录后准备成功，未变更生产源码。最终 Cargo 阶段均通过。

全部 Cargo 已结束，本轮 PostgreSQL 和 Linux holder 已停止。关闭前其他 DB 连接 0、测试 schema 0，遗留 schema 清理 0 个；用户 5433 未访问。原日志、源码快照与缓存留在 ignored `run/validation-20261012-settings-authorization/`，可核对副本及哈希见证据索引。空闲 holder 停止码不是 Cargo 结果。

本轮总 checklist **10/10**，L09—L12 **4/4**，R01—R06 仍 **2/6**，不是项目完成百分比。前文记录实施时点，已由本节关闭最终验证、资源、源码提交待项。

未独立动态验证纯 update gate 排队或慢 JSON body 窗口；最终核验位置由静态顺序与真实 policy/preview 等待验证。未运行真实 PTY/Easy-Turn、Linux 全 workspace、release/部署 smoke、外部付费服务、Pi/声学/执行器或长稳。最终核验是一次准入，准入后的降权不追溯撤销已开始的更新。
