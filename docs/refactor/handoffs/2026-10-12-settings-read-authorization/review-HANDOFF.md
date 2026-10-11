# Settings read authorization — 独立审查 HANDOFF

最终检查项 **8/8 完成**；以下保留各实施时点，最终结果以文末 Root 验收记录为准。
日期 2026-10-12；分工 `settings_auth_review`；Root 提供基线 `b15fa7d`。本分工只读审查代码与执行证据，只拥有本文，不修改生产或测试、不运行 Cargo／数据库／Docker／Git、不创建子 agent。生产必须在 Root 保存未修复源码的有效 RED 后才修改。

## 当前 checklist

- [x] 读取上轮 checkpoint 与 Root／flow／regression／review 四份 HANDOFF。
- [x] 核对 GET 完整等待链、Registry 序列化与实际 API 调用者。
- [x] 独立确定最终准入位置、共用 private helper 范围及权限保证边界。
- [x] 与 fix／reproduce 分工对齐真实见证、敏感数据保护和失败清理。
- [x] 核对 Root 的未修改生产源码 RED 及业务拒绝断言。
- [x] 独立读回实际生产修复、最终回归源码与清理顺序。
- [x] 独立核对最终同源的 macOS／Linux 运行证据。
- [x] Root 完成资源关闭、证据、checklist、恢复点与正常提交。

当前 **7/8**；有效 RED、实际生产修复、最终回归源码、V3 定向及两端完整运行已独立核对，无阻断项。本文停写，剩余 Linux no-default、资源关闭、证据归档与提交由 Root 最终补记，不预报尚未完成的结果。

## 既有行为与准确缺口

`AuthUser::from_token` 已校验 JWT 并在 2 秒内读取当前账户，角色取自 SQL；缺失或 inactive 账户入口返回原 401，handler 的非管理员检查返回原 403。GET 后续顺序为 `lock_updates` → `current_policy` → SQL policy 的 Registry 投影 `batch_update` → 按 prefix 调用 `list` 或 `all` → 构造 JSON 与 quoted policy-version ETag。原管理员身份只是进入时的快照，以上异步等待可能让已提交的降权、停用或删除先于响应发生。

`SettingEntry` 直接序列化 value 与可选 schema；Registry::list/all 直接克隆 entry。注册项包含数据库 URL、各 LLM provider API key，`schema.secret` 仅是 UI 元数据，不负责服务端响应遮罩。审计只读取注册代码，没有读取实际部署值或凭据。现有管理员读取完整值的契约保持；本轮不能借补最终身份核验改成新遮罩协议、删除 schema/value 或改变 prefix 规则。

实际应用调用者 `packages/alia-frontend/lib/api/hooks.ts::useSettings(prefix?)` 请求同一个 `/api/v1/settings`，类型为 `Record<string, SettingEntry>`；设置页依据 schema.secret 使用密码输入呈现。这不消除网络响应中的原始 value，也不需要本轮修改前端。Server runtime 的 context_policy／settings／embedding_profile 等测试还消费 GET 的 quoted ETag 或原字段。本轮只读这些调用者，未扩展到 frontend、embedded device settings 或其他 GET。

## 最终准入与最小复用

GET 的 fresh active-admin 检查应在 `list/all` 的 entries 快照完成之后、`Json(entries).into_response()` 之前。这样它后于 GET 全部已有准备 await，包括 Registry 访问，而检查成功后只剩 JSON／ETag 的同步构造。只在拿到 update gate 或 SQL current 后检查仍会留下后续 Registry 等待窗口。

上轮 PATCH 已实现相同的 id-only 查询与 2 秒期限。把它提取为 `api/settings.rs` 内 private helper，由 GET 最终位置与纯非政策 PATCH 原位置调用，是本轮所需复用；不导出到 auth 全局、不创建授权租约／权限框架、不新增依赖或跨调用长事务。PATCH 的 SQL policy／embedding 分支仍必须保留原事务内 active-admin actor 锁及 CAS，不能被普通查询替换。

缺失、inactive 或非 admin 的最终查询结果为原 403；DB 错误或 2 秒核验超时为原固定脱敏 503。保持 PATCH 现有错误消息，不回传 SQL、账号字段、token、配置值或路径。GET 拒绝不能构造或发送成功 entries／ETag；empty prefix、特定 prefix 和空结果也遵循管理员读取边界，不能靠结果形状跳过核验。

准入是数据库读取快照的一次检查，不是持续授权：之后才发生的降权不会撤回已构造或已发送字节。GET 的 `current_policy()` 可以自动恢复到期 pause、推进政策版本，且既有 RAM policy 投影已在最终检查之前发生；它们不被最终拒绝撤销。因此准确保证是“不交付设置快照或成功 ETag”，而不是“拒绝前绝无 SQL／内存变化”。GET 不新增文件保存或 provider apply。入口 JWT 等待期间到期、其他管理接口和全系统撤权均不随本轮扩张。

## 复现与测试见证要求

RED 可复用上轮真实 policy 行锁等待与更新 gate 已占用的见证：客户端在屏障前创建，在其他连接提交降权后释放 barrier，旧 GET 的 200 与合成 secret 标记可证明缺口。仅屏障超时不算业务复现，须保留原日志区分。原见证未将 SQL waiter PID 唯一绑定该 handler；不得据它夸大为全部 await 的精确定位证明。

GREEN 应覆盖最终降权／停用／删除的 403、最终查询 DB 错误／超时的脱敏 503、有效管理员全量／prefix 成功与原 value/schema/quoted ETag。测试应使用 fixture 合成的 secret 标记，只记录状态、ETag 是否存在、响应是否含该标记等布尔值，不输出响应正文、配置文件、JWT 或真实凭据。应明确检查拒绝响应无成功 ETag、无 settings entries／secret 标记。

纯 update gate 入队与 Registry 内部锁等待若无现有公有接口的可靠见证，应明确未独立动态覆盖，并以最终 helper 在全部 await 之后的静态顺序约束；不能加生产测试开关或用 sleep 伪造时序。fresh 查询故障必须在入口认证已成功之后注入，否则只证明入口错误而非最终核验。

失败清理须先释放 SQL／测试屏障，再有界 join 请求，最后 cancel/join runtime，归还 fixture schema。借鉴上轮审查，已完成 JoinHandle 必须先从 Option 取出再传播内部 panic，避免清理时二次 poll；超时未完成句柄则保留给 abort/join。客户端断开不直接证明 Axum handler 取消，不能据此声称响应字节或已完成 policy 投影被撤销。

## 实际 RED 与生产修改读回

Root 的 `red-policy-wait.log`／result 已独立核对：源 manifest `e9dbd269854da9e4562a48d763e1875380862dd83c52130c539983a435842161`，19.53 秒，退出 101，`source_drift=[]`，**0 passed / 1 failed**。一次查询即看到 policy 等待者、settings gate 已持有、请求未结束；另连接提交降权后释放屏障，GET 返回 **200、成功 ETag、含合成 secret 标记**，文件未变化。失败位置是预期 403、实际 200 的业务断言，所以这次是有效的原缺陷复现。日志没有输出标记原值或响应正文。

RED 后实际生产修复只在 `api/settings.rs`。GET 的 `require_current_admin` 调用位于 list/all entries 快照之后；后面 JSON 与 ETag 构造之间没有 await。PATCH 的非政策分支仍在原位置调用，SQL policy／embedding 分支未变。原 id-only、active admin 条件、2 秒 timeout、403 与两条固定 503 消息逐字移到同文件 private helper，末尾返回 `Ok(())`，没有额外公开接口或权限模式。独立静态读回没有生产阻断项。

## 六项最终回归源码审查

`settings_read_authorization.rs` 六项共用真实 HTTP／PostgreSQL fixture。降权、停用、删除均在入口认证已通过、handler 持有 settings gate 并等待 policy 之后提交；DB 错误临时 rename 本 fixture users 表，超时持有 users 的真实表级锁，并再次观测最终查询的 SQL 锁等待。拒绝检查原 403／503、仅包含 error 的响应外层、无 ETag、无 synthetic secret；DB 诊断不包含 schema、users 或 actor ID。生产 2 秒期限与外层 5 秒 join 保留。

仍为 admin 的用例将完整响应与 Registry entries 序列化结果比较，保留未遮罩的合成 value、secret/type schema 与 quoted policy ETag；随后核对 namespace、带末尾点 namespace、单键、空 prefix 和未知 prefix 五种原过滤行为。无 token 401、普通 user 403 的原入口行为也验证无 ETag／secret。所有响应与标记比较都使用不打印原值的布尔断言，日志不回显内容。

六项都核对文件未改变；policy 内容/版本不变仅适用于无到期 pause 的测试状态。没有扩张成“GET 的 current／RAM 投影绝无变化”的产品承诺。失败清理先释放 policy 与 account 两种屏障、恢复表名，再有界 join 请求和 cancel/join runtime；已完成句柄先 take，避免上轮发现的二次 poll。四连接 fixture 同时最多由 policy barrier、current 与 account barrier 占用三条，观测使用独立 admin 连接，未发现测试自身的连接池等待环。

独立源码审查没有阻断项。private helper 提取仍应由上轮八项 PATCH 授权回归及原 policy／embedding 测试共同验证。没有为证明最后 Registry await 新增生产插桩，最终位置依据静态代码与已验证 policy 等待约束；此覆盖边界继续记录。

本分工没有运行测试或修改源码。下一步读回 Root 冻结运行的实际结果。

## V1 定向失败与 V2 诊断证据

Root V1 定向实际 **5 passed / 1 failed**，manifest `aa52f64b79e6ec2fd82bb79d1e1828066e3c0c954c4138d92ebb5c95d13aec36`，41.22 秒，退出 101，无源码漂移。五个降权／停用／删除／DB 错误／超时拒绝均通过，均无 ETag／合成 secret；超时实际 2006 ms。KeepAdmin 的状态 200、secret schema、成功 ETag 和原始合成 value 检查已经通过，但失败于整个响应与 `serde_json::to_value(Registry)` 的直接相等断言。不能把这次运行报告为全部通过，也不能为绕过失败删除完整响应检查。

Root V2 保留原失败断言，仅添加无值诊断，定向运行 KeepAdmin：manifest `2e2a4a5dcb0a6fd5bd7744d14f91b66c9a998b416e377fec976b29bf633a92c8`，11.24 秒，退出 101，无源码漂移。独立读回日志确认双方 166 个 key，仅 3 项 float value 不等、schema 相等；将期望完整 JSON 经过同样 serialize→parse 后，完整响应与三个不同 entry 均严格相等。诊断没有输出具体值或秘密。

这支持比较层次不一致的测试原因：HTTP 响应已经过 JSON 编码解码，而内存 to_value 尚未经过该数值转换。正确修正是令期望也经过同样编码解码，再执行完整严格相等；不增加浮点容差、不删字段、不改变生产数值／API／依赖 feature。V3 实际修复已独立读回：all 与每种 prefix 的期望均直接对 Registry entries 执行 `to_vec` → `from_slice`，完整 Value 相等断言和 secret/schema/ETag 检查保留，临时诊断 helper 删除。生产修复保持不动，没有静态阻断。实际 GREEN 待后续记录，两次原失败日志保留。

## V3 定向 GREEN

已独立核对 `read-authorization-v3.log`／result：最终 manifest `48547827ad52952af80c9275cc83dd35ef9da5a25cb58b7fb32e8cab57fc1c7c`，655 个输入，退出 0，13.23 秒，`source_drift=[]`，**6 passed / 0 failed / 0 ignored**。降权／停用／删除均 403、查询错误 503、查询锁等待超时 2005 ms 后 503，全部无 ETag／合成 secret；正常 admin 的完整 JSON／prefix／schema／ETag 及入口 401／403 子断言通过。`source-transition.json` 记录 V1→V2→V3 只改变本轮新测试文件，生产保持 V1 修复，与独立读回一致。

定向通过不代替最终两端完整验证，也不重复累加进全量计数；后续需核对同源 workspace／Linux server 与原八项 PATCH 授权回归。独立 Registry 内部等待、纯 update gate 排队和完整请求取消仍未动态证明；下一候选仅记录为 Settings HTTP 共享 gate 的排队预算与取消所有权审计，先审计再确认缺口，不自动增加整请求 timeout 或改变部分成功语义。

## V3 两端最终验证分阶段核对

macOS workspace V3 已独立读回原日志、result 与 counter：退出 0，500.16 秒，manifest 与最终 V3 一致，`source_drift=[]`，**1134 passed / 0 failed / 5 ignored**，嵌套子进程摘要没有重复计入。server_runtime **78/0/0**；本轮 GET 六项与上轮 PATCH 八项均逐项通过，保护了共用 helper 提取前后的写入准入语义。

Linux server V3 的原日志、result 与 counter 也已独立读回：退出 0，401.249 秒，**852 passed / 0 failed / 5 ignored**；server_runtime **78/0/0**，GET 六项与 PATCH 八项逐项通过。`authorization-regressions.json` 与两端原日志一致，这 14 项只是全量子集，不另加到总数。Linux 的 source_before/source_after 均为 655 个文件且 errors 为空，`source_drift=[]`。本分工实际读取 Root `source-v3-manifest.json` 与 Linux `linux-arm64-v3/evidence/source-manifest.json` 的原字节并计算 SHA-256，均为 `48547827ad52952af80c9275cc83dd35ef9da5a25cb58b7fb32e8cab57fc1c7c`，确认为同一最终源码。

macOS no-default 全目标检查退出 0，51.83 秒，manifest 同 V3、无漂移；Linux no-default 在本审查收口时仍运行，由 Root 补记最终结果。Root 还负责资源关闭、655 项输入的源码 Git blob 核对、证据归档及提交；此处不提前宣称完成。

独立最终结论：本轮 GET 在等待后交付敏感设置前的当前管理员核验缺口已由有效 RED、六项 GREEN 及两端完整回归闭合。实际 helper 复用保留 PATCH、SQL policy／embedding 事务、API 和权限语义。没有生产或测试阻断项；V1／V2 的测试比较失败及 V3 的完整 JSON 编解码修正均有原证据，不能抹去失败或误记为生产字段变化。未修改本轮范围外接口或持续撤权模型。

## Root 最终验收补记

源码 `d080710052ae308b962b826d0f096fb10067469e`；最终 V3 manifest `48547827ad52952af80c9275cc83dd35ef9da5a25cb58b7fb32e8cab57fc1c7c`，655/655 源码 Git blobs 一致。

macOS ARM64 workspace **1134/0/5**；Linux ARM64 Server 默认完整回归 **852/0/5**。两端 Server 无默认 feature 全目标检查、架构边界正反自测、变更 Rust 格式、差异及计数器 8 例自测通过。数字为 passed/failed/ignored，嵌套子进程摘要不重复计入。全部最终阶段同源且无漂移，原期限与默认测试并发保留。新增 GET 6 项及既有 PATCH 8 项均在两端全量中逐项通过；定向 V3 的 6/0/0 不额外计入总数。

RED 在未改生产时有效复现降权后的 200/ETag/假密钥泄露。GREEN V1 为 5/1，V2 保留原断言的诊断单例为 0/1；166 个键中 3 个浮点 value 与直接内存值比较不同，但期望按 HTTP 同样 JSON 编码解码后完整严格相等。V3 只修正测试比较层次，不用容差或删除字段，生产自 V1 起未变。Linux V1 仅准备环境，未执行 Cargo；Linux 最终阶段明确使用 V3。失败、诊断、原 RED 测试全文和各 manifest 均保留。

全部 Cargo 已结束，本轮 PostgreSQL 及 Linux V1/V3 holder 均已停止。关闭前其他 DB 连接 0、测试 schema 0，遗留 schema 清理 0 个；用户 5433 未访问。原日志、源码快照与缓存留在 ignored `run/validation-20261012-settings-read-authorization/`，可核对副本与哈希见证据索引。空闲 holder 停止码不是 Cargo 结果。

本轮总 checklist **10/10**，L09—L12 **4/4**，R01—R06 仍 **2/6**，不是项目完成百分比。

未独立动态注入纯更新 gate 排队或 Registry 私有锁等待；最后核验位置由完整 await 静态审查和真实政策等待验证。未运行真实 PTY/Easy-Turn、Linux 全 workspace、release/部署 smoke、外部付费服务、Pi/声学/执行器或长稳。核验是一次响应准入，不承诺后续持续撤权或撤回已发送响应。
