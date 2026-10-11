# Settings 保存失败恢复回归 HANDOFF

日期 2026-10-11，基线 `1d6ef37`，分支沿用 `codex/architecture-refactor`。本 subagent 先只读审计，再按 Root 确认的范围编辑 `packages/alia-server/tests/settings.rs`、新增 `tests/server_runtime/settings.rs`，并在 `tests/server_runtime.rs` 增加 module 声明。没有修改生产源码，也没有执行 Cargo、PostgreSQL、Docker 或 Git；最终实际验证由 Root 统一运行。

## 已有证据与本轮缺口

- 原 `tests/settings.rs` 只有 3 条注册／schema 检查，没有保存、加载或文件失败回归。
- `server_runtime/context_policy.rs`、`relationship_api.rs` 与 `embedding_profile.rs` 已覆盖 admin、If-Match 旧版本冲突、SQL 失败不改 RAM、跨实例政策保留、embedding 显式同值重试和重启。本轮继续沿用，不复制完整矩阵。
- `SettingsRegistry::save` 原来直接 `std::fs::write`；PATCH 原顺序为 SQL policy CAS、desired registry、模块／Interaction live apply、文件保存。已有显式请求 key 与 changed 合并，所以同值重试本来就应再次 apply。新增断言保护这一恢复语义，不把部分失败解释为跨域回滚。
- 旧 `PATCH /memories/privacy` 绕过 SQL policy 与 settings gate，只有 AuthUser 门槛，GET／audit 也读本进程 RAM。用户确认本轮将 PATCH 接入统一 SQL 流程并要求 admin，GET 原 authenticated 读取权限和成功 8 字段保持；HTTP agent 负责生产迁移。
- 已读取上轮 config-durability checkpoint／HANDOFF。没有把 setup 的 pending descriptor 协议套到运行时 settings。

## 新增 5 条回归

### 两条公开 SettingsRegistry 文件系统回归

1. `persisted_values_exclude_sql_policy_and_null_and_preserve_registered_schema`：在唯一临时目录保存值，确认 SQL-owned policy 与 null 不进入 TOML，schema metadata 不被写作 setting；新文件在 Unix 为 0600。用另一 Registry 实际加载，确认 values 恢复、已注册 schema／secret metadata 保留。
2. `invalid_persisted_file_does_not_partially_replace_values_or_the_save_target`：非法 TOML 在有效字段之后截断，load 必须失败且不部分覆盖旧值、不更换原保存目标；随后正常 save 和新 Registry load 仍读到原目标的新值，非法文件原字节不变。

这些测试使用公开接口，没有生产测试 hook；临时目录以 Drop 清理，不改变全局环境变量或真实用户配置。

### 三条真实 HTTP／runtime 回归

1. `file_failure_keeps_live_settings_and_same_value_retry_persists_for_restart`：先成功保存原 settings；将保存目标暂时设到这个真实 regular file 的子路径，确定性触发发布前 I/O 错误。HTTP 必须返回 200／success=false／非空 errors，不泄露临时目录；desired、受控 extension 和实际 Interaction tool limit 都已经变化，原保存文件字节保持，纯非 policy 更新不推进 SQL policy 版本。恢复保存路径，重发完全相同 body：changed 为空，但 extension 再消费一次，save 成功；关闭 runtime 后用真实 builder 重建，确认文件中的设置重新应用。
2. `module_partial_failure_saves_desired_settings_and_explicit_retry_applies_again`：受控 ModuleLifecycle extension 先记录所消费值再返回 Err，响应必须承认部分失败；desired、实际 Interaction policy 和保存的 TOML 都保留新值。相同 body 重试虽 changed 为空，仍再次进入 extension，模块报告成功，不能把上次失败伪装成未发生变化。
3. `privacy_patch_uses_admin_sql_cas_and_all_instances_read_committed_policy`：两个真实 runtime 共用隔离 schema；普通用户 PATCH 为 403，但 GET 仍为 200，坏 If-Match 为 400。真实 PostgreSQL trigger 注入 policy UPDATE 失败，断言 503、没有泄露私有 SQL 信息、RAM／registry／policy version／文件不变。移除 trigger 后成功仍为原 8 字段，旧 ETag 返回 409，纯 SQL privacy 不保存 settings 文件。第二 runtime 特意保留旧 RAM，GET 和 audit.privacy 必须读到已提交 SQL。随后同一 fixture 覆盖 pause_days 暂停、跨实例 paused 读取、clear_pause 与 pause_days 同传时 clear 优先，以及已到期 policy 经实际 HTTP GET 恢复后 paused_until 为空、持久版本只推进一次。最后用 builder 重启，核对启用／capture_mode 与最终 SQL version。

核心过期恢复的并发 CAS 已由 `tests/context_lease.rs::timed_resume_is_one_persistent_cas_and_does_not_resign_old_work` 覆盖，本轮只补新 privacy HTTP 读取行为。使用 `clear_pause` 做稳定重试；相同 `pause_days` 请求重新计算相对日期是原输入语义，不声称其时间戳严格幂等。

## 夹具与失败清理

复用 `server_runtime.rs` 的隔离 PG fixture／builder、`delivery::account` 与现有 JSON HTTP helpers。受控 extension 只记录消费和一次错误，不访问模型、硬件或外部服务。每个新增 runtime 断言体用 catch_unwind 包裹；失败后先 cancel 并有界等待服务退出，再由外层清理 schema／目录；双 runtime 使用嵌套 catch 分别 join，重启后的断言也先捕获再 shutdown。没有靠固定 sleep 制造竞争。

发布后同步结果未确认、完整父链、同 Registry 并发保存、symlink 与保留既有 mode 的细节由 storage agent 内部故障测试负责。HTTP 的路径故障只证明确定性发布前失败、live 部分成功与显式重试，不冒充发布后故障或真实断电测试。

## 已审阅与明确边界

已读回 HTTP agent 的 shared policy current／prepare／commit、privacy GET／PATCH／audit 接线，以及保存失败区分是否已发布的响应分类。受控 extension 的失败不会改变其他模块的结果，SQL 结果不被文件失败回滚。现有公开 Registry 接口保持，private 文件权限由 storage 实现与内部测试共同保证。

发现并上报了另一现存边界：纯非 policy PATCH 在等待 gate／policy 读取期间若 actor 降权，仍只使用进入 handler 时的 AuthUser；policy SQL CAS 本身另有当前 admin 验证。Root 明确将该等待后身份重验证作为后续候选，本轮不擅自扩大。取消／drop 不能仅凭客户端断开就声称 Axum handler 已取消，本轮没有伪造此类所有权证据；没有新增跨域事务、进程锁、外部模型、平台或依赖。

## Checklist 与恢复

- [x] 只读核对既有 settings／实际路由／CAS／权限／重试测试与上轮记录。
- [x] 两条公开 Registry 保存、加载与元数据回归。
- [x] HTTP 文件失败、live 状态、同值重试及真实 builder 重启回归。
- [x] 模块先应用后报错、desired 保存及显式重试回归。
- [x] privacy 权限、SQL 失败、跨实例 GET／audit、pause／clear／到期及重启回归。
- [x] 按所属文件 scoped rustfmt，生产 HTTP API 静态读回；已通知 Root source ready。
- [x] Root 在冻结输入上实际编译、定向及完整验证，并记录所有失败和结果。
- [x] Root 最终同源验收、资源清理、checklist／恢复点与正常提交。

实施交接时为 **6/8**，Root 最终验收后为 **8/8**。仅完成源码与静态核对，没有本 subagent 执行的测试通过数字。Root 负责追加最终实际验收证据。

## Root 最终验收补记

源码 `2f5e2a2267c8499a116e652a4779f111ddbe5aff`；冻结 V1 manifest `c2fc158414333488da098f06050c4daf681477aef50fbdda193b82b111744312`，653/653 源码 Git blob 一致。

macOS ARM64 workspace **1120/0/5**；Linux ARM64 Server 默认并行完整回归 **838/0/5**；两端 Server 无默认 feature 全目标检查、架构边界正反自测、变更范围 rustfmt 和差异检查通过。数字为 passed/failed/ignored，嵌套子进程摘要不重复累加。全部最终阶段使用同一份源码，没有源码漂移。本轮新增 12 项（7 storage、2 Registry、3 HTTP/runtime）在两端通过；本轮测试阶段均首轮通过，测试期限保持原值。

全部 Cargo 阶段结束。本轮独立 PostgreSQL 与 Linux holder 已停止，关闭前其他 DB 连接为零，测试 schema 为零，遗留 schema 清理数 0；用户 5433 未访问。日志、源快照及缓存保留在 ignored `run/validation-20261011-settings-recovery/`，可核对副本见证据索引。空闲 holder 的停止码不作为 Cargo 结果。

本轮总 checklist **10/10**，L09—L12 仍 **4/4**，R01—R06 仍 **2/6**，不是项目完成百分比。前文实施时点的未运行说明保留，实际整轮结果以本节为准。

未运行真实断电、独立进程 kill/restart、真实 PTY／Easy-Turn、Linux 全 workspace、release／部署 smoke、外部付费服务、Pi／声学／执行器或长稳。存储回归使用软件故障注入，runtime 重启使用实际 ServerBuilder；不等于操作系统进程重启或物理断电。ACL／扩展属性迁移、异 owner 拒绝的提权实测、FIFO 故障和其他本机进程并发改写没有验证。
