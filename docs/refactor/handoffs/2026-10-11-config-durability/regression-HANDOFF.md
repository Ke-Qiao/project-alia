# 配置目录持久性回归 HANDOFF

日期 2026-10-11，基线 `7165ad6`。本 subagent 负责 `src/oobe/routes/tests.rs` 的既有接口迁移及新增 `src/oobe/routes/tests/durability.rs`，没有修改生产 routes、bootstrap、config loader 或 publication。没有执行 Cargo、PostgreSQL、Docker 或 Git，没有读取或修改真实用户配置。

## 只读审计与沿用证据

先读取 OOBE HTTP fixture、CLI/setup runtime、配置加载／分片合并、操作说明和现有调用者。旧 HTTP 用例已经覆盖：完整模板生成、role/profile 校验、现有 operator fragments 合并、写入失败不发 Ready、HTTP 单赢家、晚 CLI 不写另一份配置、取消后拒写、Host／Origin／JSON 边界。`bootstrap/setup.rs` 另外覆盖实际 listener 排空、已接收但未完成 body 的取消、真实 PTY 清理；`tests/config.rs` 覆盖递归排序的 fragment 优先级。本轮不复制这些基础用例。

审计发现两个必须由生产契约修正的行为：CLI 对所有保存失败都删除新建 fragment tree，不适用于 base 已发布后的 sync 失败；旧 completion 对所有失败都允许新写入，不适用于未知发布结果。另外，启动发现 base 已存在会直接走 `ServerConfig::load`，所以仅测重开 setup 不能证明实际启动恢复。Root 负责把 pending marker 接入真实 loader／discovery／needs_setup。

## 新接口迁移

既有 route fixture 从 `watch<Option<SetupResult>>`／`finish_setup` 改用一个 `SetupCompletion` 与 `SetupState` watch。prepublish 失败应为 Idle；Pending 不等于 Ready 或 Cancelled；成功只一次 Ready。

原 CLI 拒覆盖用例每次使用新的 `SetupCompletion::new().generate(...)`，确保仍验证磁盘已有 base／fragment tree 的拒覆盖，而不是被旧 owner 的 Ready 终态提前拒绝。HTTP winner 后的晚 CLI 测试实际请求另一路径，并断言 base、`.d` 都没有创建。

## 四条新增回归

1. `http_prepublication_sync_failure_preserves_old_files_and_allows_retry`：临时目录已有完整 base 和 operator fragment，通过真实 HTTP 保存更改后的配置。在目标隔离的 FileSync 故障后，响应仍为 HTTP 200／success=false，watch 保持 Idle，旧 base／fragment 原字节不变，无 Published 事件。随后真实 HTTP 重试成功，加载仍尊重 operator fragment，Unix base 为 0600。
2. `http_postpublication_failure_reserves_the_original_intent_until_sync_retry`：Published 阶段故障使 base 已替换，但响应明确 unknown，watch 为非终态 Pending。另一路径、同路径不同内容和 CLI 安装方法均被拒绝，不能创建第二套配置。同一原路径和原内容重试后 Ready，Published 事件仍只有一次，证明只完成同步，不重复发布。
3. `cli_postpublication_failure_keeps_its_fragments_and_blocks_http_until_retry`：CLI 首装在 Published 后失败，base 及完整递归 fragment 文件字节全部保留。HTTP 不得切到另一目标，原 CLI 意图重试后各文件字节不变、只出现一次 Published，并可由真实 loader 校验全部例示角色。
4. `loader_recovers_the_original_publication_without_the_previous_setup_owner`：分别在 MarkerDurable（base 尚旧）和 Published（base 已新）中断。前者还取消 completion 并证明新写入被封门。随后有界关闭 HTTP，完全丢弃旧 completion 和 watch，仅保留临时目录；直接调用实际 `ServerConfig::load` 恢复原 publication。两种情况均只产生一次 Published，最终内容为原提案；后续没有 marker 的第二次 load 不产生同步／发布事件，保持普通只读加载。

注入来自 `config::publication::testing`，按目标目录隔离，不使用进程全局环境变量、权限竞态、固定 sleep 或真实存储故障。底层文件读写、原子发布和正常同步仍发生在实际临时文件系统。测试用例内的断言捕获 panic 后关闭并等待 HTTP fixture；保留目录的恢复场景使用独立清理 guard，避免丢弃旧 setup 状态时先删文件。

最终静态对齐已读回实际 `publication::{save,recover,complete}`、`testing::{fail_once,Stage,Guard}`、`SetupCompletion`、HTTP routes 和 `ServerConfig::load`。`PendingPublication` 的错误声明需要恢复且持久性未确认；HTTP 回归断言这一语义，并不把普通失败当作未写入。HTTP fixture 明确断言状态为 200。`complete` 在原 base、候选、fragment 或 recovery record 身份改变时拒绝恢复；成功同步 base、依赖及祖先后才清理候选和 marker；marker 已删除但最后目录同步失败时，同 owner 可以按保留的 descriptor 补同步。

## 分工与证据边界

精确的 fragment 文件→深层目录→祖先目录同步次序、底层 marker 校验和各种阶段失败矩阵由 storage agent 的 publication 内部测试负责；本文件只通过可观察阶段核对原发布没有被重复执行。实际 `run_setup_on` 在 unknown 后继续服务、同意图重试成功后响应排空与 listener join 的回归由 flow agent 负责。

已独立读回 storage 的 9 条内部回归：tree／祖先次序使用实际事件位置比较；MarkerRemoved 用例区分无 marker 的普通纯读与原内存 intent 补同步，并覆盖补同步再次失败；target／candidate／fragment 内容或集合变化的拒恢复矩阵保留 pending；固定 marker 竞争使用阶段屏障，释放后等待 writer；相邻 descriptor temporary 仅在匹配原记录内容时清理；Unix base symlink 保留 canonical overlay，候选被替换成 symlink 后拒恢复；发布前失败遗留的目录在重试时仍同步完整祖先链。以上是静态交叉审查，不替代 Root 的实际运行证据。

V1 冻结后的补充审查发现：发布前 FileSync 失败留下新建空目录，随后 save 把目录当作既有目录，会使旧 `ancestor_count` 缩短，遗漏前次未确认的祖先 entry。storage 在 V2 删除该字段，保存、恢复和最终清理均从 canonical 目标父目录同步至完整祖先链。新增 `retry_syncs_all_ancestors_left_visible_by_an_earlier_preparation_failure` 实际先触发 FileSync 失败，再对同一路径重试，并在 MarkerDurable 之前检查全部祖先的 DirectorySync 事件，能够捕获 V1 缺口。已只读核对修复及对应测试，没有新增阻断发现。

普通无 marker 加载仍在 `recover` 的 marker 存在检查后直接返回；不会调用 `sync_ancestors`、打开祖先目录执行同步、创建文件或改权限。既有无 marker 加载的零事件断言继续适用。完整祖先同步只发生于明确保存或存在恢复记录的恢复路径；测试不把注入事件等同于物理断电持久性证明。

这些测试证明合作式文件系统调用与状态恢复契约，不证明物理断电、磁盘控制器缓存、任意网络文件系统或多进程并发操作者。关闭 HTTP 并丢弃 completion 是进程内状态重建，不声称执行了进程 kill 或硬件断电。仍信任本机操作者和现有目录权限；未扩展平台、硬件、依赖或授权范围。

## Checklist 与恢复

- [x] 只读核对发布／加载／HTTP／CLI 调用链与既有回归。
- [x] 既有 route fixture 与单赢家／来源保护断言迁移到 SetupCompletion。
- [x] 真实 HTTP prepublish 和 Pending 原意图重试回归。
- [x] CLI Published 失败保留完整 base／fragment tree 回归。
- [x] 丢弃旧 owner 后实际 ServerConfig::load 恢复与后续纯读回归。
- [x] 存储层最终 API 落盘后完成静态交叉核对与冻结前格式检查。
- [x] Root 在统一冻结源码上编译、运行定向及完整测试并记录实际结果。
- [x] Root 同源最终验收、资源清理、checklist／恢复点与正常提交。

实施交接时为 **6/8**，现经 Root 最终验收为 **8/8**。测试源码已经写入，publication API 静态对齐和两份所属源码的 scoped rustfmt 已完成，已通知 Root source ready。Root 报告 V1 库验收为 357 passed／0 failed／3 ignored；V2 完整 workspace 正在运行，本 subagent 没有执行测试，也不沿用 V1 结果证明 V2 通过。最终数字由 Root 在本文件追加实际证据。

## Root 最终验收补记

源码 `095499676496fba79853532b2ad4f570b368d37c`，V3 manifest `11373f598d31148c27b15bfa1fc5bb015e3b2726f877347560b23bbcc1b9343f`，650/650 Git blob 一致。

macOS workspace **1108/0/5**；Linux ARM64 Server 默认并行完整回归 **826/0/5**。两端无默认 feature 全目标检查、架构边界正反自测、变更文件格式检查、计数器 8 例自测通过。数字为 passed/failed/ignored；嵌套子进程摘要不重复累计，最终阶段同源、无源码漂移。

全部 Cargo 阶段结束；关闭前其他数据库连接为零，最终测试 schema 为零，遗留 schema 清理数 0。本轮 V1/V2/V3 Linux holder 与独立 PostgreSQL 已停止；用户 5433 未访问。空闲 holder 的停止码不作为 Cargo 测试结果。缓存、只读快照保留在 ignored `run/validation-20261011-config-durability/`。

上文 subagent 实施阶段的进度数字与未运行说明保留历史时点；对应 Root 验收事项已勾选，现已完成同源整合验收、资源关闭与源码提交，本轮总 checklist 为 10/10。

未运行真实断电、独立进程 kill/restart、Linux 全 workspace、release/native release link、部署 smoke、真实 PTY、外部或付费模型、Pi/声学/执行器及长稳验证。故障注入与丢弃旧 setup owner 后加载证明软件恢复语义；不代表任意存储设备兑现 flush，也不提供其他本机进程的文件系统锁。
