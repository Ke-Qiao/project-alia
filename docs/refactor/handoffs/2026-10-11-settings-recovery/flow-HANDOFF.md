# 2026-10-11 Settings recovery — HTTP flow HANDOFF

## 范围与 checklist

基线 `1d6ef37`，沿用当前分支。本分工负责 `api/settings.rs` 和 `api/memories.rs`；SettingsRegistry 的原子保存与错误类型归 storage 分工，真实 HTTP／重启回归归 regression 分工。Root 统一冻结、编译、运行、检查资源和提交。

- [x] 审计 Settings GET／PATCH、SettingsRegistry、启动恢复及 ModuleRegistry 的真实调用顺序。
- [x] 确认已有同值重试与 SQL CAS 保障，保留这些实现。
- [x] 审计唯一另一保存调用者 `/memories/privacy`，向 Root 报告其内存旁路与实际 SQL 政策不一致。
- [x] 按用户确认将 privacy PATCH 统一为管理员修改，并接入共同 SQL 政策流程。
- [x] 抽取窄范围的 SQL current／If-Match／验证／CAS helper，保留原错误 status。
- [x] privacy GET、PATCH 成功响应及 audit.privacy 使用真实 SQL 政策。
- [x] 删除 privacy 直接 RAM 写入与 `settings.save()` 的旧旁路。
- [x] 接入保存的发布前失败／发布后同步未确认类型，保持安全诊断与原响应字段。
- [x] 存储与 HTTP 回归落盘后交叉审阅取消、重试、启动与权限边界。
- [x] Root 冻结并完成双平台验证、资源清理和提交。

当前仅完成源码编辑和 scoped rustfmt，未运行编译或测试。本分工未运行 Cargo、数据库、Docker 或 Git，没有调用外部服务或硬件。以下描述为静态代码结论；测试结果由 Root 完成后补充。

## 既有保障与真正缺口

Settings PATCH 原顺序为管理员检查 → 同进程更新锁 → SQL current／If-Match／policy CAS → desired SettingsRegistry 内存 → modules 逐个 apply → interaction policies → settings.toml 保存。SQL 政策有独立版本与管理员事务核验；读取及提交保留 2 秒期限，失效 ETag 返回 409，明确验证失败返回 400，SQL 失败或确认超时返回 503。失败发生在 CAS 之前时，不先更新本地 desired 或 live 状态。

显式重试已经使用 `changed ∪ requested_keys`：即使 desired 值未变化，明确重发仍会再次执行模块应用并保存。已有回归覆盖政策失败、两实例 CAS、embedding profile 身份及同值修复。本轮不重复建设该机制，也不把模块部分生效、SQL 已提交或文件发布后失败称为原子回滚。

旧 `SettingsRegistry::save` 直接 `std::fs::write`，可能截断原文件；序列化或文件失败发生时，SQL、desired 与部分 live 模块可能已生效。ModuleRegistry 在一个模块返回错误后仍继续其他模块；错误无法证明该模块从未修改内部状态。保留逐模块 reports、当前 applied／ignored／restart 字段与 desired 重试契约，具体文件发布边界由 storage 类型报告。

Settings PATCH 通过 `SettingsSaveError::was_published` 区分两类错误，仍为 HTTP 200、`success=false`，保留完整 changed／applied／requires_restart／ignored_runtime／module_reports／context_policy_version。发布前失败明确原目标未发布本次设置、desired 及已完成政策／live 修改保留；发布后失败明确文件已经发布而持久性尚未确认、没有执行跨域回滚，两者都提示显式重试。响应只包含固定诊断，不回传底层路径、凭据或文件内容。

## SQL 流程复用与 privacy 清理

`prepare_policy_update` 只负责 bounded current、原 If-Match 语法、版本冲突和 `context_policy::patched` 验证；`commit_policy_update` 复用原 `replace_for_actor_with_embedding` 与其 2 秒确认期限；`current_policy` 复用原错误分类。它们保持在 API 模块内部，没有新事务框架或通用协调器。Settings 原 embedding preview、隐式完整 model／dimensions／endpoint、desired 更新和 module apply 顺序保持。

旧 `/memories/privacy` PATCH 允许任意 AuthUser 修改全局内存投影；其 enabled／capture_mode／paused_until 都属于 SQL 政策键，恰好被 `settings.save` 排除。旧 GET 返回本地 RAM，但实际 Generation 用 SQL policy 覆盖这三个字段，重启也从 SQL 恢复。因此仅给旧入口加锁不能修复正确性。用户明确授权本轮统一管理员修改权限后，才迁移此入口。

新的 privacy PATCH 保留请求字段和映射规则：capture_mode 优先于 automatic_capture_enabled；clear_pause 优先于 pause_days；天数仍限于 1..365。它在管理员检查后使用与 Settings 相同的更新锁、If-Match 和 SQL CAS；SQL 成功后才投影 SettingsRegistry 与 InteractionPolicies。这里仅写 SQL 政策，不保存 settings.toml，也不调用无关 provider。成功仍返回原 8 个 privacy 字段，GET 仍允许原 authenticated 用户读取。

privacy GET、PATCH 成功响应和 audit.privacy 都由 SQL PolicySnapshot 构造。删除旧 helper 对本机时钟和 RFC3339 字符串的独立恢复判断；SQL `current()` 原有数据库时钟到期转换仍是权威。PolicySnapshot 有 pause 时，显式／自动 Memory 均按暂停计算，已到期暂停由 SQL current 先清除并推进政策版本。隐私响应不再为一个未使用的 capabilities 参数额外执行诊断查询；独立 capabilities 端点和 audit 的 capabilities 字段保持。

## 重试、取消与后续边界

- settings.toml 采用已有部分成功／显式重试语义，不新增 pending guard、API 字段或后台重试。保存失败不会回退已经提交的 SQL 政策，也不会伪称已撤销 provider 操作。
- 如果请求取消在 SQL 提交之后、RAM 投影之前，SQL 仍是政策权威。下一次 Settings／privacy 读取或显式重试可重新观察；不承诺请求取消能回滚 SQL 或外部模块。
- privacy 的相对 `pause_days` 每次请求计算新的日期，沿用原接口语义；同 body 重发不是严格相同的绝对截止时间。
- 本轮不扩展非政策设置在等待更新锁之后的管理员身份重验证。政策事务内原有 active admin 验证保持；非政策路径的等待期间降权作为独立后续候选记录。
- 本轮不扩展所有 provider 的 apply 原子性、并发调度、超时或错误模型；共享 LLM 实例按已有 registry 去重规则应用一次，工具可见性及执行批准未修改。

## 回归计划与证据状态

实施阶段 regression 分工补充了真实 HTTP 的文件失败／同值重试／重启、模块先部分生效再失败／同值修复，以及 privacy 管理员限制／SQL 失败无内存变更／If-Match／跨实例或重启读取。使用受控 ModuleLifecycle extension 与本地文件故障，沿用 runtime fixture 的 cancel／join／schema 清理；不需要外部付费服务或生产测试 hook。

本文件暂不声称以上回归已经通过。最终编译、平台、计数、失败日志与源版本由 Root 验证后更新。

静态复核结论：共享 helper 没有改变 Settings 的 current → If-Match → patch → embedding preview → CAS 次序。把原 `expected` 传参改为 `current.version` 的前提由同一 helper 显式验证；显式 embedding reaffirm、两个 profile pin、descriptor 同 SQL 事务及隐式完整 model／dimensions／endpoint 的 desired 写回保持。privacy 不进入 embedding preview 或 provider apply。

storage 初稿的公开 `SettingsSaveError` 与 HTTP downcast／`was_published` 签名一致；没有配置路径、序列化和 rename 前错误均为未发布，rename 之后的同步错误为已发布未确认。独立 persistence gate 在取得快照之前进入，复制后释放 inner 读锁再执行同步文件操作；load／set path 使用同序锁，无调用 update 锁的反向路径。同步文件操作无中途 async yield，不产生请求取消后失管的保存 task；这不构成对阻塞文件系统调用的可取消保证。

已只读审阅 regression 分工的 3 项 runtime/settings HTTP 回归：失败前后原文件字节对照、同值再次到达受控 module、partial module 失败后 desired 保存、真实 restart、privacy 非管理员／无效与旧 If-Match／SQL 失败、另一实例刻意保留旧 RAM 后读取 GET 与 audit SQL 权威。运行中的 runtime 已在断言 catch 外持有，失败后先 cancel／join 再返回 fixture；restart 的断言也在 shutdown 之前捕获。以上均为源码审阅结论，不代替 Root 实际执行。

## Root 最终验收补记

源码 `2f5e2a2267c8499a116e652a4779f111ddbe5aff`；冻结 V1 manifest `c2fc158414333488da098f06050c4daf681477aef50fbdda193b82b111744312`，653/653 源码 Git blob 一致。

macOS ARM64 workspace **1120/0/5**；Linux ARM64 Server 默认并行完整回归 **838/0/5**；两端 Server 无默认 feature 全目标检查、架构边界正反自测、变更范围 rustfmt 和差异检查通过。数字为 passed/failed/ignored，嵌套子进程摘要不重复累加。全部最终阶段使用同一份源码，没有源码漂移。本轮新增 12 项（7 storage、2 Registry、3 HTTP/runtime）在两端通过；本轮测试阶段均首轮通过，测试期限保持原值。

全部 Cargo 阶段结束。本轮独立 PostgreSQL 与 Linux holder 已停止，关闭前其他 DB 连接为零，测试 schema 为零，遗留 schema 清理数 0；用户 5433 未访问。日志、源快照及缓存保留在 ignored `run/validation-20261011-settings-recovery/`，可核对副本见证据索引。空闲 holder 的停止码不作为 Cargo 结果。

本轮总 checklist **10/10**，L09—L12 仍 **4/4**，R01—R06 仍 **2/6**，不是项目完成百分比。前文实施时点的未运行说明保留，实际整轮结果以本节为准。

未运行真实断电、独立进程 kill/restart、真实 PTY／Easy-Turn、Linux 全 workspace、release／部署 smoke、外部付费服务、Pi／声学／执行器或长稳。存储回归使用软件故障注入，runtime 重启使用实际 ServerBuilder；不等于操作系统进程重启或物理断电。ACL／扩展属性迁移、异 owner 拒绝的提权实测、FIFO 故障和其他本机进程并发改写没有验证。
