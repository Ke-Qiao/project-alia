# Settings authorization — 独立审查 HANDOFF

最终检查项 **8/8 完成**；以下过程说明保留各实施时点，最终结果以文末 Root 验收记录为准。
日期 2026-10-12；当前分工 `settings_auth_review`，接续已有审查记录。本轮只读审查权限、并发和验证证据，只拥有本文；没有修改生产源码或测试。Root 统一记录基线、冻结版本、运行和提交。

## 当前时点与 checklist

- [x] 读取上轮 settings-recovery 的 Root、flow、storage、regression 四份 HANDOFF。
- [x] 核对 AuthUser、SQL actor 锁、其他已有账户核验实现和错误映射。
- [x] 核对 Settings 更新锁、政策查询、embedding preview、desired/live/file 的全部等待边界。
- [x] 提出最终准入位置与明确的可保证/不可保证范围。
- [x] 独立读回 RED 回归的锁见证、降权提交顺序和失败清理。
- [x] 独立审阅实际生产变更、最终回归源码和 V1 定向 GREEN 证据。
- [x] 独立核对 Root 最终冻结源码的两端全量日志与相同源清单。
- [x] Root 完成 checklist、恢复检查点和正常提交。

本稿当前进度 **7/8**。首轮 RED 失败发生在测试屏障见证阶段，不算业务负例复现；第二轮 RED 已建立实际等待见证，并捕获降权后仍成功改变 desired/live/file 的原缺陷。两次日志分别保留；生产修复、八项回归及纯测试失败清理已独立读回，Root V1 定向与 V2 两端完整测试均通过。独立审查完成，剩余 Linux no-default、资源关闭、文档和提交由 Root 最终补记。

## 现行权限证据

`auth/middleware.rs::AuthUser::from_token` 验证 JWT 后，在 2 秒期限内读取当前 users 记录，要求 active，并把数据库的 role 放入 AuthUser。它没有把 JWT 的 role claim 当作 authority。缺失/过期 token、缺失或初始 inactive 账号仍走原 401 语义；handler 中普通 user 的 admin 检查返回 403。

AuthUser 是请求进入时的快照。Settings PATCH 在这之后还可能等待 JSON body、`settings.lock_updates()`、`current_policy()` 的数据库行锁，以及两次 `embedding.preview_profile_settings()`。纯非政策分支当前不执行 SQL policy CAS，因此只靠进入 handler 时的 `auth.is_admin()` 不能覆盖这些等待期间已经提交的降权或停用。

政策或 embedding 变更由 `ContextPolicyStore::replace_inner` 在同一事务内依次锁 policy 行、查询当前 active admin 并持有 actor `FOR SHARE`、执行版本核对/写入、commit。这个现有 actor 检查应保持，不能替换成更早的本地 role 判断。事务提交之后的模块应用原本就不持续持有该 actor 锁。

其他现行实现可作为边界参考而非本轮修改对象：WebSocket 定期重新调用 AuthUser；channel/insights 在各自准入点查询 active account；execution 在其 SQL 事务内锁用户行并核验 role。它们的权限与事务范围不同，没有现成的“持续授权所有 settings 模块”的通用 helper，也不应为本轮制造这类框架。

## 最小完整修复的准入点

最终 fresh active-admin 检查应放在纯非政策 `else` 分支，位于 current/If-Match/patch 和全部 embedding preview 之后，首次 desired `batch_update` 之前。只在取得 settings gate 后立即检查仍有可观测缺口：请求随后可能被 SQL current 或 provider preview 挡住，在那里降权后继续修改。

该 fresh query 的数据库读取快照是一次请求准入证据，不是持续授权租约。已经提交且对该读取可见的 inactive/missing/non-admin 账号必须拒绝；读取成功之后才发生的降权不撤销已准入的模块或文件操作。后续 registry 写锁、模块 apply、InteractionPolicies 和 persistence 等待保留既有部分成功语义，不能伪称跨域回滚。显式同值重试是新的请求，仍必须重新通过最终准入。

本轮不增加跨 SQL/模块/文件长事务，也不持有 users 行锁跨越 provider apply。取消、drop 或新降权不等于已经调用的模块被撤销。JWT 到期与其他接口的权限模型不随本轮扩展。

## 实际生产修复独立读回

RED V2 复现后已读回 `api/settings.rs` 实际修改。唯一新准入查询内联在纯非政策 `else`：它后于 updates gate、SQL current／If-Match／patch 与两次 embedding preview，先于任何本请求 `batch_update`、module apply、InteractionPolicies 修改或文件保存。SeaORM 查询只选择匹配 `id`、`status=active`、`role=admin` 的 users.id，不读取整行账户资料；2 秒 timeout 包含该异步数据库操作等待。

没有匹配行时返回原 `AppError::Forbidden`；超时与数据库错误分别返回固定 `Unavailable` 消息，响应不回显 SQL／token／账号资料，数据库错误仅由现有结构化 tracing 记录。没有新增公开 helper、依赖、行锁、后台任务或事务；原政策／embedding 分支仍由 `replace_for_actor_with_embedding` 在 SQL 事务内核验 actor 并执行 CAS，未被新查询替代。API 成功字段、入口错误及 If-Match/验证错误的顺序保持。

本分工对该生产修改的静态审查没有阻断项。新的查询是一次有界准入，不承诺其后持续撤权；超时或取消后释放进程内更新 guard，没有分离的 settings apply/save 作业。数据库驱动取消请求的内部连接回收时点不在本轮新增保证之内。最终测试源码见证及 Root 实际执行结果仍待后续记录。

## 错误和“未产生变更”的准确范围

| 情况 | 预期处理 |
| --- | --- |
| 原入口坏/过期 JWT、缺失或 inactive 账号 | 原 401 保持 |
| 原入口 role 非 admin | 原 403 保持 |
| 最终检查发现账号缺失、inactive 或 role 不再为 admin | 403，不能准入本请求的 desired/live/file 修改 |
| 最终检查数据库错误或 2 秒核验超时 | 503，固定脱敏错误，不当作无权限或继续成功 |
| 先发生的 If-Match 语法错误、版本冲突、输入验证失败 | 原 400/409 顺序保持 |
| 已准入后模块或文件保存失败 | 原 reports、部分成功及显式重试保持 |

`current_policy()` 不是纯读：它会按数据库时钟恢复已到期 pause，并推进政策版本。因此不能声称最终 403/503 之前绝无数据库变化。正确承诺是拒绝本次请求的 desired/live/file 和请求政策变更；既有自动到期恢复不变。测试无到期 pause 时可对照 policy version，但这个测试条件不能外推为所有拒绝均无 SQL 写入。

## 验证矩阵与 RED 独立审阅

已读 `tests/server_runtime/settings_authorization.rs` 的初版 RED：先建立真实 runtime、保存旧文件、记录 desired/live 和受控 module 计数；独立事务锁 policy 行后发出真实纯非政策 PATCH。测试观察 `pg_blocking_pids` 指向该 barrier 的实际 policy 行锁等待，同时确认该 registry 的 updates gate 已被占用；随后用另一连接提交 actor 降权，再释放 barrier。它对照 HTTP 403、desired/live/file 不变和 module apply 为零，记录的仅是受控数值和变更标记，没有打印 JWT、文件内容或响应秘密。

失败清理先回滚 barrier，再有界 join 请求，最后 cancel/join runtime；断言体有 catch，避免负例故意失败遗留 SQL 锁或运行任务。生产 current 的原 2 秒期限未为测试放宽。

Root 首轮 RED 的真实结果是 `0 passed / 1 failed`、退出 101，失败位置为 1 秒内未观察到 authenticated PATCH 的 policy row barrier（`settings_authorization.rs:81`）。源码 manifest 为 `a71d23e8a563ea75c27f9cabb0c206dafe0165710d681e8202d8b6525e1c1879`，结果文件记录无源码漂移。证据在 `run/validation-20261012-settings-authorization/red-policy-wait.log` 和对应 `red-policy-wait-result.json`。测试未执行到降权后的 403/desired/live/file 业务断言，因此这轮结果只证明见证未建立，不能被报告成已经捕获原授权缺陷。

独立静态诊断确认生产 `policy::read` 的 SQL 与测试 LIKE 完全匹配，barrier 和 handler 使用同 schema 的 fixture 数据库池，路径与账号创建沿用既有成功测试。可疑但未获实证的准备耗时是 spawned request 内同步构造 reqwest Client：默认 current-thread runtime 上，该构造可能把 TLS/系统配置准备计入 1 秒观测期限。建议仅把客户端准备移出屏障/观测区，并记录不含秘密的 `blocked_seen`、`gate_seen`、最终 HTTP status 定位阶段；不增加 timeout、不弱化断言。当前失败清理已取得响应，但先恢复 gated panic、后写状态日志，故首轮缺少这条诊断证据。本分工只提建议，没有改测试。

Root 第二轮 RED 已按上述方式将客户端准备移出观测区，保留原 1 秒见证与生产 2 秒期限。独立读回 `red-policy-wait-v2.log` 及对应结果：`reached=true`、两次查询即观察到一个真实 policy 锁等待者、barrier idle、settings gate 已占用且请求未结束；随后另一个数据库连接提交降权，再释放 barrier。HTTP 返回 200，desired 与 live 均从 8 变为 9，文件字节变化且受控 module apply 一次；测试由预期 403、实际 200 的业务断言失败，退出 101。冻结 manifest `17610fb468942b964b2b56eeb4fa2e1986164b5eb4543f8dfae00756ffcfa19f`，记录 `source_drift=[]`。这次结果证实原权限缺口；它不能证明首轮见证失败一定由 TLS 初始化导致，亦不覆盖或抹去首轮失败。

该见证可证明请求已通过入口认证并占有更新 gate，且目标 policy barrier 存在实际等待者。它没有把该 SQL waiter PID 唯一绑定到 HTTP handler；理论上的其他后台 current 等待者可能与“请求刚拿 gate”同时出现。因此不据它声称任意假设修法都必然被捕获。Root 已决定最终 GREEN 补充受控 embedding preview 屏障，精确证明检查位于全部准备等待之后，并用于确定 fresh-query 故障/超时注入时点。

最终矩阵建议包括：等待期间 role 降级/账号停用或缺失的拒绝；fresh 核验 DB error/timeout 的 503；仍为 active admin 的成功与同值重试；已进入受控 apply 后才降权仍保持已准入语义。政策/embedding 的现有 SQL actor CAS 回归继续保留，不能因新 helper 重复或削弱它。

## 最终八项回归源码审阅

实际新增文件由一项真实 policy 行等待测试与七项受控 preview/apply 用例组成。两次 preview 均使用 `EmbeddingAdapter` 已有纯 preview 接口，受控 adapter 返回 None，不改变 embedding identity、不访问外部 embedding 服务。信号量屏障确认目标请求已经进入相应 preview 或 module apply，之后才提交账号修改或注入数据库故障；无需固定 sleep，也没有生产测试 hook。

- 第一 preview 中降权、第二 preview 中停用或删除均要求 403，desired/live/文件字节保持，受控 apply 次数为零。
- 第二 preview 中分别临时更名 users 表或持有真实表级锁，精确触发 fresh query 错误或等待；固定 503 响应不得回显 schema、表名或 actor ID。超时用例还观察实际 SQL 锁等待，保留 2 秒生产期限，并在 5 秒外层界限内收束。
- 仍为 active admin 的请求成功，显式同值重试再次 apply；均分别核对 policy version 和 policy 内容没有变化。
- 已进入 module apply 屏障后才降权的请求仍完成，用例明确保护已准入后的部分成功边界，不伪称持续撤销或跨模块回滚。

上述用例无到期 pause，因此 policy 内容/版本不变断言成立；不是所有拒绝都绝无 SQL 变化的通用证明。独立检查了文件的 imports、trait 继承与运行期 fixture 接口，未发现必须增加生产导出或依赖的问题。审查指出并协调修复一个纯测试失败清理问题：已完成的请求 JoinHandle 必须先从 Option 取出，再传播内部 panic，避免清理阶段二次 poll 已消费的完成结果而跳过 runtime 关闭。V1 定向通过后，该修复已落盘并独立读回：timeout 返回 Ok 时先 take，再 unwrap；超时返回 Err 时继续保留未完成 handle 给清理流程。

本轮没有独立动态建立“入口认证完成但仍排队于 updates gate”的见证；已有 policy/preview 等待均能证明请求持有该 gate，生产最终检查明确位于 gate 之后。这个覆盖边界直接记录，不增加生产认证 hook、SQL 日志计数或脆弱轮询来假造独立 gate 见证。

## V1 定向 GREEN 证据独立核对

已读回 Root 的 `authorization-v1.log` 和 `authorization-v1-result.json`：manifest `d11c9b111c5177a46c332d8877126170bf3188feeae83dbff2a1ec16bf22fa1d`，`source_drift=[]`，退出 0，**8 passed / 0 failed / 0 ignored**。原 RED 用例现为 403，desired/live 均保持 8、文件不变、apply 为零；两个 preview 的拒绝用例也均无这些副作用。fresh 查询错误为 503，真实锁等待超时为 2003 ms 后的 503。active admin 与准入后降权两例均按既有语义返回 200，同值重试断言通过。

该 V1 结果先于上面的纯测试 JoinHandle 清理修复；不把它冒充最终源码全量通过。Root 后续冻结 V2，macOS／Linux 全量、资源关闭及提交结果分别记录。

## V2 最终输入与分阶段证据

最终 V2 manifest `1f88c037cc584c41b88efba5141858930926b1c066f2b0b881e7579e490a90e1`，654 个输入；相对 V1 的源码变动只有授权测试的 JoinHandle 失败清理。已独立读取 macOS workspace V2 原日志、结果与计数：退出 0，524.89 秒，`source_drift=[]`，**1128 passed / 0 failed / 5 ignored**；server_runtime **72/0/0**，本轮八项授权测试逐项通过，计数排除了嵌套子进程摘要。

Linux 第一次准备因缺失空 source 目录失败，发生在源复制、容器启动和 Cargo 之前；`linux-prepare-initial-failure.json` 明确三者均未开始。补建运行目录后重试准备，源码不变。Linux 运行目录名 `linux-arm64-v1` 表示首次 Linux 验证尝试，其实际源快照是上述最终 V2，不能因目录名误标为已被替换的源 V1。

已独立核对 Linux server 完整原日志、result 与计数：退出 0，400.418 秒，**846 passed / 0 failed / 5 ignored**；server_runtime **72/0/0**，本轮八项授权测试逐项通过。结果文件的 source_before/source_after 均为 654 个文件且 errors 为空，`source_drift=[]`。实际读取 Root 与 Linux 两份 manifest 字节并分别计算 SHA-256，均为上面的 `1f88c0…90e1`；两端结果属于同一 V2 源码。macOS no-default 全目标检查退出 0，49.45 秒，无源码漂移。Linux no-default 在本审查收口时仍运行，由 Root 追加最终实际结果；不预报通过。

独立最终结论：本轮限定的非政策 PATCH 最终准入缺口已由 RED/GREEN 证据闭合，实际生产改动保留政策／embedding 事务、API 与部分成功语义。未发现需阻断本轮交付的生产或测试问题。上述通过不扩大到独立 gate 排队见证、GET、持续撤销、真实硬件或全部管理接口。资源关闭、最终 Git blobs、证据归档和正常提交继续由 Root 核验。

## 明确暂缓与交接

GET settings 在等待期间仍使用进入时的 AuthUser 快照，是独立后续候选，本轮不扩大。静态调用链为入口 admin 检查 → updates gate → current policy → registry list/all → JSON；registry 直接克隆 SettingEntry，`schema.secret` 仅为 UI 元数据，不执行响应遮罩，注册项包含数据库 URL 与 provider API key。因此下一轮应先复现等待期间降权后读取的边界，核对返回敏感设置前的最终准入，并保留现有管理员读取契约；本轮没有读取任何实际凭据，也没有改 GET。空 privacy PATCH 与其他管理接口也没有被本轮统一成持续授权。这里没有重新审计整个系统权限模型，不给全项目“已解决所有降权窗口”的结论。

本分工没有运行 Cargo、数据库、Docker、Git，也没有新增 subagent。RED、生产修改、最终测试源码、V1 定向 GREEN、V2 两端完整测试和相同 manifest 已独立读回；本文在该时点停写。资源关闭、剩余 no-default、证据归档与提交由 Root 收口。

## Root 最终验收补记

源码 `4553e88138cb91af46f024b343baaa3857c8d769`；最终 V2 manifest `1f88c037cc584c41b88efba5141858930926b1c066f2b0b881e7579e490a90e1`，654/654 源码 Git blobs 一致。

macOS ARM64 workspace **1128/0/5**；Linux ARM64 Server 默认完整回归 **846/0/5**。两端 Server 无默认 feature 全目标检查、架构边界正反自测、变更 Rust 格式、差异和计数器 8 例自测通过。数字为 passed/failed/ignored，嵌套子进程摘要不重复计入。全部最终阶段同源且无漂移，保持默认测试并发和原期限。新增 8 项身份回归均在两端全量中通过，V1 定向 8/0/0 不额外加到全量数字。

RED 首次屏障失败与 V2 有效越权复现分别保留；GREEN V1 定向通过后，仅修复测试 panic 清理再冻结 V2。Linux 首次准备缺少空目录，复制源码和启动容器前失败；补目录后准备成功，未变更生产源码。最终 Cargo 阶段均通过。

全部 Cargo 已结束，本轮 PostgreSQL 和 Linux holder 已停止。关闭前其他 DB 连接 0、测试 schema 0，遗留 schema 清理 0 个；用户 5433 未访问。原日志、源码快照与缓存留在 ignored `run/validation-20261012-settings-authorization/`，可核对副本及哈希见证据索引。空闲 holder 停止码不是 Cargo 结果。

本轮总 checklist **10/10**，L09—L12 **4/4**，R01—R06 仍 **2/6**，不是项目完成百分比。前文记录实施时点，已由本节关闭最终验证、资源、源码提交待项。

未独立动态验证纯 update gate 排队或慢 JSON body 窗口；最终核验位置由静态顺序与真实 policy/preview 等待验证。未运行真实 PTY/Easy-Turn、Linux 全 workspace、release/部署 smoke、外部付费服务、Pi/声学/执行器或长稳。最终核验是一次准入，准入后的降权不追溯撤销已开始的更新。
