# PN01 独立审查与接硬件范围

2026-10-17；基线 `15604e3`。当前 **5/5：冻结生产与测试静审 GO，HTTP 7 项、既有 storage 4 项及资源快照已核对，PN01 独立审查完成**。只读审查，只有本 HANDOFF 可编辑；未运行 Cargo、数据库、服务、网络或硬件。默认 shadow／probe=false、原期限／并发、Execution 批准和 R03 暂停保持。归档索引、受测源码 hash 与正常提交由 Root 统一完成。

## Checklist

- [x] 核对管理 API、storage／auth 调用与既有当前身份复验。
- [x] 按 Root 确定范围复核管理员与本人 scope、等待及事务删除语义。
- [x] 确定有限测试建议与首个 R02 接入的真正前置。
- [x] 独立审查最终 production／test diff，报告 GO 或具体阻断。
- [x] 核对 Root 必要验证与资源证据，收口结论及限制。

## PN01 最小边界

原 `/plugins`、`/{name}/storage` usage／DELETE、`/{name}/storage/export` 都是管理视图；当前 AuthUser 提取只检查当时 active，不替代管理员检查或等待后当前身份。原查询按 plugin 跨 owner，应按已确认 D01 收紧为管理员入口。

Root 已明确同轮交付本人入口：`/{name}/storage/me` GET usage／DELETE（可选 collection），及 `/me/export` GET。查询固定 `scope='user'` 与可信 `auth.id`；未知 owner／scope 参数拒绝，不能借本人接口读取 global、channel、conversation 或别的用户。本人接口只需当前 active，不要求管理员；本轮不建立泛用不可信 broker，也不猜其他 scope 成员权限。

读接口在全部取数／等待结束、返回前复验当前身份，复用 Settings 的 2 秒 fail-closed 语义。DELETE、最终 actor 复验和提交位于同一事务；失权 rollback 后原数据不变，不能先提交删除再返回 Forbidden。最终 `FOR SHARE` 保持到 commit，避免授权核对后已提交的降权先于本次删除提交。当前 user 更新／软删除只写 users，plugin storage owner 为 text、没有 user FK；有限调用核对未发现相反的跨表锁路径，不宣称全局无死锁。

保留原 CAS、TTL、retention、受信 compiled 插件接口和 Execution。当前 export 排除过期记录、purge 可清理过期记录是不同语义，不能在复用 helper 时误改。`force_export_required` 不等于 purge 权限。管理权不能代替 effect owner 的批准。

冻结的 `api/plugins.rs`／`plugins/storage.rs` diff 已独立核对，**生产静审 GO，无阻断**：读的最后 await 为 2 秒当前 actor 复验；本人 SQL 的 user scope 与 UUID 由宿主固定，管理和既有受信内部 helper 才使用无 owner 筛选；DELETE 后事务内最终 `FOR SHARE` 与显式 rollback／commit 符合上述授权边界。原 TTL、usage 过期计数、purge 包含过期记录、retention 与四个旧响应结构保持。仅两个生产文件变化，无全局 auth 框架或新账本；测试实际结果见下文。

## 必要验证建议

Root 统一执行 **7 项 HTTP 回归＋既有 storage 回归**，不另起测试平台或扩大批次。重点是普通用户管理拒绝／管理员正例、本人 scope 隔离与参数拒绝、等待后降权读拒绝、删除等待后失权回滚、collection 范围，以及未认证／失活边界；数据夹具应能区分至少两 owner、global／其他 scope 和其他 plugin，断言被拒绝时原值／计数保持。

等待用真实 PG 锁和明确已进入阶段证据，不用 sleep 猜时序。既有 `tests/plugin_storage.rs` 在缺 `ALIA_TEST_DATABASE_URL` 时打印 skipping 后 return；统一运行需明确提供并验证真实 PG，不能只靠 Cargo 绿色作为真实数据库证据。Root 已确认本次环境显式指向任务 PG，见最终结果。复用现有 HTTP／fixture 清理所有权，不因本轮增加无关完整故障矩阵。

## 冻结测试与 HTTP 结果

新增 `tests/server_runtime/plugin_authorization.rs` 的 7 项测试及主文件模块接线已静审，**GO，无本轮阻断**。夹具同时包含两个 user owner、global、与本人 UUID 同文本的 channel／conversation、过期记录及另一 plugin；拒绝／撤权后的完整行快照比较不只是计数。个人导出校验固定 user scope 与 actor，伪造 owner／scope 被拒绝；管理员正例覆盖原 JSON 结构、collection 删除、全删及邻 namespace 保留。

三个等待用例通过专属 blocker PID 与 `pg_blocking_pids` 见证当前请求已进入真实 storage SQL 等待，确认请求尚未结束后更新角色／active 状态，提交变更再释放锁。失败路径保存 blocker／请求句柄，先释放锁、排空请求，再取消并等待运行时；没有用 sleep 或改变默认并发。该局部清理核对不等于覆盖全部 runtime 初始化／任意取消故障矩阵。

`run/validation-20261017-pn01/green-http.log` 已独立读取：**7 passed／0 failed／0 ignored／85 filtered，3.32 秒**。三个等待均 `observed=true`、`request_finished=false`，撤权响应均 403、`exposed_other=false`、`changed=false`。原两项 RED 的管理入口现在均 403，拒绝后记录 9→9；管理员和个人正例也通过。原 RED 为 0／2，证明旧入口确有跨 owner 导出／删除；不累计为 GREEN。

`run/validation-20261017-pn01/green-storage.log` 已独立读取：**4 passed／0 failed／0 ignored／0 filtered，0.07 秒**，包含既有 PostgreSQL round trip／CAS／TTL／retention 回归。Root 明确设置 `ALIA_TEST_DATABASE_URL` 指向真实任务 PG；该执行前提来自统一执行者确认，本 agent 不把单独的 Cargo 绿色推断成数据库确已执行，也未自行重跑。

`run/validation-20261017-pn01/resources-before-stop.json` 已核对为 `fixture_schemas=0`、`other_database_clients=0`。Root 随后确认任务容器 `alia-pn01-pg-20261017` 停止成功，`--rm` 后按任务名查询为空；容器操作由 Root 执行，本 agent 未操作 Docker。原始日志、资源记录与四个受测 Rust 文件的身份由 Root 统一归档／提交，不在此预写尚未返回的 commit。

**最终审查 GO**：已确认越权缺口的修复及本人 scope 路径有与改动相称的证据，本轮不追加全 workspace 或无关故障矩阵。11 项 GREEN 不等于第三方安装／隔离运行器、完整插件生命周期、真实模型链或硬件通过；PN02—PN04 继续留待后续，V01 原始配对问题的历史结论不被本轮改写。

## PN01 后尽快转 R02：三个不同层次

| 接入层次 | 真正前置 | 不应作为这一层的门槛 |
| --- | --- | --- |
| R1 USB 接入与只读枚举 | 用户实际接入 R1、确定实物／固件资料和 USB 归属；Pi 账户与设备节点可见性 | LLM／TTS链、PN02—PN04、MCP、插件市场、完整升级／备份矩阵 |
| 有界真实采集／播放与声学基线 | 目标 Pi 可运行的 Embedded、准确音频端点／格式、安全任务配置（motion 关闭、无插件）、单 consumer、停止／清理入口、明确录音窗口与保留／删除规则；连接 AEC 参考前取得扬声器／功放及厂商电气接法 | 未审核第三方运行器、更多平台、通用软件 AEC 框架、真实聊天模型全部选定 |
| 自然全双工整链 | 已确认的输入／输出和硬件 AEC 路线，加当前 Mac 模型／Generation／Delivery 链的真实组件与组合证据 | 插件市场、任意语言隔离或未来同机部署产品化 |

已有 mixer、单 reader、双流有界证据、离线重放和独立时钟软件不重复建设；硬件 AEC 优先已确认，USB 接同一 Pi 不证明功放参考已连通或效果通过。缺少电气资料时不能擅自试接参考，但不应将功放未知扩大为禁止无参考的只读 USB 枚举。

建议 PN01 收口后转首个有界 R02 现场前置／接入，而非自动串行执行 PN02—PN04。在明确不安装／不启用插件的首个音频试验中，后续第三方包、表达 ABI 与隔离平台不是音频门槛；它们继续留在插件实施清单，不删除目标。真实操作仍需按用户确认窗口开展，本轮不执行。R03 保持暂停，R05 全部整机条件不能倒用为开始 R02 的前置条件。

依据：`docs/refactor/resume-checkpoint-r02-r1-2026-10-14.md`、`docs/operations/hardware-deployment-readiness.md`、`docs/refactor/resume-checkpoint-model-preflight-2026-10-17.md` 及当前插件检查点。旧台账中某轮未做 Linux 的记录保留其历史含义，不忽略后续已取得的 Linux 构建／测试证据，也不将容器通过冒充目标 Pi 音频已运行。
