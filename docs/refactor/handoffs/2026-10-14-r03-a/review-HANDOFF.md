# R03-A 独立审查与回归设计 HANDOFF

日期：2026-10-14。分工：`settings_auth_review`。Root 指定基线 `2b30dd6`，承接已完成的 R03-0 / R02-A，不重新实施这两轮。本分工仅修改本文，不修改生产或测试、不运行 Cargo、数据库、Docker、SSH 或硬件，不提交 Git。测试与冻结同源证据由 Root 统一执行。

## 审查 checklist

- [x] 读取当前 R02-A 恢复检查点和 Body / Motion 设计及使用说明。
- [x] 核对 Core command、raw-byte Ed25519、Server intent/ledger 与 Embedded prepare/submit 实际链路。
- [x] 区分实际同名内容缺口与 prepare 后已保持的不可变动作保证。
- [x] 核对现有依赖、路径读取 TOCTOU 与准备任务所有权。
- [x] 向 Root 和两位 owner 提供双端清单、版本、取消/重试和配置写入边界。
- [x] 独立读取 Root 的实际 RED，核对确定性见证。
- [x] 审查 Core / Server 最终内容身份、清单与原意图恢复实现。
- [x] 审查 Embedded 同 buffer 验证/解析、读取资源与本地信任边界实现。
- [x] 核对 Root 集成、默认政策、旧契约拒绝和文档一致性。
- [x] 核对最终同源 GREEN、运行限制和资源收尾。

当前 **10/10**，是本分工进度；没有声称 R03 硬件、安全或真实执行验收通过。两位 owner 及 Root 附带提示音写入修复已完成静态审查、未见阻断；最终同源默认回归 **1207/0/5**、实现提交及本任务数据库清理已核对。共享环境存在已明确记录的变化，不能声称全部恢复 Stopped。本分工停止编辑，最终文档和证据归档提交由 Root 收口。

## 已确认的目标与信任来源

Root 转达用户本轮选择 **B：双端本地受信清单**。受信操作者审核后，Server 和 Embedded 本地受控配置各保存相同的 canonical file、version、SHA-256 与精确 byte length；默认清单为空，历史 `.anim` 文件不自动受信。清单授权内容身份，不替代现有表达政策、执行批准、设备 pin、绑定、Response/owner/voice scope、控制 lease 或本地机构校准。

本轮不启用硬件、不修改校准数据、归中/停止保持政策或精确取消；不新增依赖、设备协议、平台或资产上传/自动批准服务。缺失/未知/不匹配资产必须拒绝，不能把模型自报 digest、路径/mtime 或临时对磁盘文件求 hash 当作批准来源。

## 原实现真实边界

Core `BodyMotion::Animation` 只有 `file/fps/loops`；Server `BodyIntent::parse` 从 expression action params 直接生成它。请求和响应 Ed25519 域分离签名覆盖原始有界 JSON bytes，所以目标身份、参数与命令不能任意变造，但没有覆盖文件内容。Server 当前并无 animation approval catalog，只根据动作名称及设备 available 发布 `play_animation` capability。

Embedded `plan::read_animation` 当前 canonicalize 目录/文件，检查 canonical parent，然后 `File::open`、metadata regular、一次最多 65537 bytes 读取，限制 64 KiB、600 source frames、有限角度、循环和动作时长。parser 跳过首行 header。现有路径/数值/总量校验应继续保留，不能用新 hash 校验替代它们。

实际缺口是：同一个通过签名与 canonical 检查的 name-only command，在首次 prepare 前同名文件被换为另一份合法不同轨迹，仍会得到不同动作。最小 RED 可在私有 plan 测试中用相同 command bytes、同一文件名和两份有明确不同轨迹的合法 bytes 验证，不需要硬件。

prepare 后动作已经保存在 `PreparedBodyCommand.actions` 的 Vec 中，writer 不再读取文件。此后替换文件不会改变这份已准备动作，因此不能把这一点错误地报告为现有漏洞；GREEN 应明确保留这个保证。

路径上另有 `canonicalize(path) → File::open(path)` 窗口。leaf 或父目录可在检查后替换；FIFO 还可能在 open 阶段阻塞，使 regular-file 检查根本未到达。只在绝对路径 open 上加 O_NOFOLLOW 不会锚定父目录。Embedded 已直接依赖 libc，可在现有 Unix 范围复用 dirfd + openat、O_NOFOLLOW / O_NONBLOCK / O_CLOEXEC 和 fstat regular，以已打开目录定位单一 leaf。成功 fd 应立即交给 RAII File；验证 digest、byte length 和 UTF-8 / frame parsing 必须使用同一次有界读取的 buffer，不能检查后重开。

dirfd 锚定后替换目录名/leaf 的用例可作为确定性见证。首次取得 root dirfd 前，受信本地配置和目录管理权限仍是前提，不宣称密码学消除了所有本地文件系统权限风险。并发修改 inode 可能使读取结果成为混合 bytes，但只要 buffer 不匹配固定受信 digest 就必须拒绝；mtime/inode 重查不是内容批准依据。

## 可复用依赖与契约建议

Core / Server 已有 sha2 0.10，Embedded 通过 Core 共享 digest helper 即可，不需新增直接依赖；serde、现有 Ed25519 和 libc 均已存在。候选 `BodyAnimationAsset { file, version, sha256, bytes }` 由双方本地清单提供，并进入签名的 `Animation { asset, fps, loops }`。

建议严格 canonical filename：单 leaf、明确 `.anim` 后缀、无路径或隐式别名；signed asset 不再补后缀。若模型侧保留 `neck` 这种选择方式，只允许 Server 在受信清单解析阶段一次映射为 canonical file，不能把它带入签名后由 Embedded 重新解释。清单最多 64 项，默认空，重复 canonical file / 无效 version / 非 lowercase 64位 hex / 0或超限 length 应拒绝。version 是操作者版本标识，必须和 digest、length 一起匹配，不构成机械安全认证。

Root 已确认 Body JSON contract 升为 2，并更新 request/reply 的 `alia-body-v2-*` 签名域，明确拒绝 v1 与缺少 asset 的旧 animation，不做旧命令自动补 hash。`alia.body.v1` proto package 保留为外层 envelope RPC namespace 并修正文档；Server 内部 `expression-command-v1` 的 action params 形态保持，不机械升版。最终源码不得保留绕过受信清单的兼容入口。

## 原意图、取消与资源边界

Server 的同一 Response phase 先查询 durable ledger，再决定是否分配 identity / 获得一次 dispatch permit；`same_intent` 比较 actions。受信清单仅用于新动作的内容授权，不能在 cache/ledger 重试、observe、cancel 时用最新清单重新构造旧 command，导致旧版本被改写或无法查询/精确取消。原 command asset tuple 一旦进入意图/账本必须不可变；新清单撤回/更新不授予旧命令重发权，也不能抹掉原事实。

Embedded 的 bounded preparation 并发上限为 4，保留 spawn_blocking handle；RPC 两秒等待超时或 observer drop 不遗弃任务。提交时再次验证绑定/token、voice scope、lease、水位和 writer 可用性。取消可越过 prepare 退休准确 sequence；迟到准备结果不能复活。资产验证只参与 prepare / 新接纳，不能要求 Cancel 先读取当前文件或通过当前清单。保留原 stop join 和在途 native owner，拒绝时仅为可证明从未接纳的原 identity 产生 Rejected。

## 配置可变信任边界：已提请 Root 裁定

当前 `EmbeddedSettingsService::update_settings` 经过 `require_auth_if_bound` 后把 JSON 整体交给 `EmbeddedSettingsManager` merge 并写回同一 config。若新 catalog 直接加入 MotionConfig 而不设本地边界，绑定 Server 可远程补齐第二份 pin，未绑定 provisioning 入口也能修改它。新字段应在远程更新中拒绝改变，包括直接字段和整体 motion 替换等形式，并证明拒绝不修改内存/磁盘；本地配置加载与受信 library 构造继续可维护。保存 unrelated settings 不应意外清空本地清单。

另已向 Root 报告现有 direct-response 上传路径：`write_audio_variants` 接受绝对 audio_path 并写任意 bytes，可以绕过字段级检查覆盖 config 文件。这与“双端本地清单独立确认”的实际边界直接相关，但本分工不擅自扩展整个设备权限/文件服务重构。最终本地信任根位置、远程写入保护或明确限制需由 Root 决定并记录；未解决前不能声称本地 pin 已独立于远程 Server 可修改的文件系统。

用户随后已明确同意本轮关闭这条提示音任意写入路径，并禁止远端改变 `audio_dir`。Root 负责 `settings_service` / 新 `cue_files`，Embedded owner 负责 settings / config 回归及包文档。只读继续确认 relative `../`、现有 symlink 与 hardlink 也可造成越界/同 inode 覆写，不能仅禁绝对路径或使用字符串前缀判断。删除 direct-response 只删存储记录，没有文件删除路径。

新增范围限定为普通 Settings / 提示音上传对本地 pin 的保护。既有 PluginManager Install / Start 拥有宿主同用户代码部署/执行权限，未做 sandbox；本轮明确不隔离该权能，也不宣称清单能抵抗被授权部署代码的操作者或宿主文件系统管理员。该信任前提与已复现的提示音任意路径写入缺口分别记录，不把前者扩成未经确认的插件安全重构。

已向 Root 提供写入设计复核要点：全部上传 leaf 和 weights 应在写前验证；固定启动 root、dirfd 锚定、O_EXCL / NOFOLLOW 阻止覆写既有文件及 hardlink；同步新建阶段失败只清理本次创建文件。进入 SQLite 保存 await 后可能已 commit 再读回失败，不能因未知 DB 结果删除已关联文件，不可宣称跨文件/DB 原子性。逐组件 nofollow 对 macOS `/var` 等 symlink alias 以及本地 `..` 配置的处理需明确，不能静默回退到不安全路径。具体实现与测试尚待 Root。

Root 的附带修复候选已完整读回，以上写入边界均落实。`write_uploads` 在所有上传名称校验后才操作目录；空名称生成 UUID，其他名称只接受单 leaf。`write_new_files` 从固定启动 root 的 `/` 或 `.` descriptor 开始逐组件打开/创建并拒绝 symlink；可信本地配置中的 `..` 按 directory fd 定位，远端上传不能供应 traversal。slug 再作 single-leaf 校验。新增 leaf 只 `O_CREAT|O_EXCL|O_NOFOLLOW`，不覆盖任何既有 inode；guard 在批次 open/write 错误时删除本次已创建文件，完整成功后保留给后续 DB。无 data 引用仍只存路径，不执行写入。已静态核对空 variant 无 data 被跳过不会在成功创建文件后造成空 record 错误，有上传时必有对应 record。

`EmbeddedSettingsManager` 新 private startup resolved 目录副本通过借用 getter 供服务使用；remote patch 对 `audio_dir` 原始 OsStr 比较，保存前拒绝变动，包括等价路径写法。两项配置回归覆盖 full snapshot / unrelated patch 成功、对象后改隔离、拒绝时 RAM / 文件 / startup owner 不变。cue 回归覆盖绝对/parent/nested/NUL name 预校验、部分批次 existing 冲突回滚、leaf/slug/root ancestor symlink 和 hardlink、生成路径 / proto roundtrip、无 data 不写。

准确限制：创建目录后失败可能留下空目录，cleanup 仅针对本次新文件；完整音频写入后进入 DB await，失败或取消可能留下文件，不能把未知 DB 提交误回滚为缺失音频；没有新增 fsync/崩溃持久性或跨 FS/DB 原子保证。所有权限与写入结论以受信启动目录及宿主文件系统/插件部署操作者为前提，不作恶意同用户进程隔离承诺。当前静态候选无阻断，scoped / 最终运行尚待 Root。

## 候选源码分阶段审查（尚未当作 GREEN）

Embedded owner 已宣布候选停写。独立读回：`MotionConfig.approved_animations` 默认空，load/save/library resolve 及 runtime 构造均校验；远程 settings 比较 merge 后与原清单的完整值，在保存前拒绝变更。普通保存保留清单。运行中的 manager 持有启动快照，因此更新手册明确要求先停进程，再编辑双端清单并重启，避免人工磁盘改动被普通保存覆盖。

`motion/assets.rs` 使用当前 Unix `libc`，先打开目录 fd，再以 single leaf `openat`；成功 fd 立即进入 RAII，regular-file 检查后最多读取 `asset.bytes + 1`，同 buffer 验证与解析。测试 seam 使用真实临时 fd，分别见证目录换名后仍锚定、leaf symlink 拒绝、flags、带 guard 的 FIFO 非 regular 拒绝、打开后 rename 不重开路径和同 inode 内容变更拒绝。FIFO guard 仅避免测试永久阻塞；非阻塞 flags 由独立 fcntl 断言核对，未把此用例夸大为无 writer 的时延证明。non-Unix 明确拒绝安全资产打开，没有新平台实现。

新 gRPC fixture 以批准内容正常执行，之后替换/删除文件，重复 Submit 保持原 Submitted receipt，新 sequence 被拒绝；删除后 exact Cancel 和 Observe 仍保持原 receipt。旧 prepare 容量、owned handles、lease/绑定/voice/取消、水位和 shutdown 路径保持，由 Root 最终完整回归确认。

Server `lookup_origin` 生产调用已穷举：submit、observe、dispatch_and_observe 三处（另 trait 转发）。前两处进入 `BodyIntent::recover → verify_record`；第三处及 claim Existing 都进入 `restore_existing → verify_record`。recover 仅从存储的 command actions 构建旧资产 tuple 集合，再解析请求 raw，最终完整 command 等值检查。repository 保留 active owner、Response 所属 JOIN 和 channel 检查，增加 stored command owner 比较；旧事实仍可在 erasure 后查询，不误加新 dispatch 的 source policy 授权。新 claim / authorize 原事务政策保持。此阶段未见遗漏 same-intent 返回点。

Server 新回归静态覆盖 empty / replacement 清单恢复原 tuple、改变 fps/owner 拒绝、无 Open/Submit 且 claim 为 0，以及新 Response 才解析新清单；mock store 不代替 SQL 授权验证，Root 新 SQL 集成回归仍待实际结果。资产 tuple 在首次 Body admission 固定，不能声称更早的 ExpressionPlan 文件名已固定版本。

最终候选追加 `Entry.recovered` 保存已查到的原账本快照，owner 启动时直接恢复该记录；确定性 fixture 在 lookup 返回后删除记录，验证不会重新进入 Fresh / Open / Submit。静态核对确认所有 existing 返回点仍完整校验原命令。Core / Transport v2 与 protobuf namespace 说明一致，Server 清单由启动配置传入 Channel / runtime，未注册动态 Settings key。两位 owner 均已通知停源，本分工不运行构建。

## 最少回归建议

1. 相同旧 canonical command + 同名两份合法不同 bytes 的 RED；GREEN 在 expected digest/length 固定时拒绝替换，且原 approved bytes 成功解析。
2. 双端默认空、未知条目、版本/大小/hash 不匹配、重复/无效清单、模型自报摘要及旧 name-only wire 都被拒绝；可信清单不自动录入历史资产。
3. 同一打开 fd / 同一 buffer 做 hash 与解析，准备后替换不影响原动作；父目录换名、leaf symlink/FIFO、非regular、超限/截断/invalid UTF-8/角度/600帧/总时长保留限制。
4. 远程 settings 不得改变本地 pin，拒绝时内存/磁盘不变，unrelated settings 保存后 pin 不丢失；若 Root 纳入上传绕过，需针对实际写入路径验证阻止覆盖。
5. 原命令的重复提交/缓存恢复/ledger lookup/observe/cancel 使用原 tuple；资产删除、清单变化或文件不匹配不阻止精确取消已存在命令，不产生新的 identity/dispatch permission。
6. 保留当前取消在准备前/中/后、准备容量满、drop waiter、late prepare 水位、已进入 native write、stop join 和旧 lease/binding/owner 回归。所有测试仅使用临时 fixture/mock，不写真实硬件。

## 实际 RED 复核

已独立读取 `run/validation-20261014-r03-a/red-core.{log,json}`、`red-server.{log,json}`、`red-embedded.{log,json}`。三次运行均使用 manifest `aff613f253280c38dc44b35f420533153eb04c07f802d00b16d6f39de3a31771`，`source_drift=[]`，均编译完成后在预期断言失败：

| 用例 | 实际结果 | 耗时 | 见证 |
| --- | --- | --- | --- |
| Core `legacy_animation_without_content_identity_is_rejected` | 0 passed / 1 failed / 0 ignored，exit 101 | 16.33s | 旧 name-only animation 被接受 |
| Server `animation_without_an_operator_approved_catalog_is_rejected` | 0 / 1 / 0，exit 101 | 55.09s | 没有受信清单仍接受 animation |
| Embedded `unchanged_signed_animation_cannot_select_replaced_file_contents` | 0 / 1 / 0，exit 101 | 10.5s | 原签名选择了替换后的不同本地内容 |

以上是 Root 实际执行、独立读回的缺口证据，不是本分工自行运行。GREEN 尚待最终实现和冻结结果。

附带边界 RED 亦已独立读回 `red-cue-boundary.{log,json}`：`cue_upload_cannot_overwrite_a_config_outside_its_audio_directory` 编译后在“cue upload accepted an external config path”断言失败，0 passed / 1 failed / 0 ignored，exit 101，19.11s，manifest `412ba53ed2f412768299a0c47c704f175919f692e6e7de02f96b69609a0d11c0`，`source_drift=[]`。仅使用临时 fixture，不涉及部署配置或真实音频；当时额外生产修复尚待用户确认，后来用户已批准本轮收口。该失败不能被当前 Core / Server 候选 GREEN 抵消。

## 已完成的候选验证

已独立读取 `candidate-core-transport.{log,json}`：Core / Transport `--all-targets --offline --locked` exit 0，42.11s，33 + 3 + 5 = **41 passed / 0 failed / 0 ignored**；使用 boundary-red manifest `412ba53e…0d11c0`（Root 冻结 660 输入），`source_drift=[]`。旧 name-only 拒绝、完整资产 bytes/catalog 校验及 `body_v2_signature_binds_content_and_rejects_old_or_opposite_domains` 逐项通过。metadata 的 phase 仍标为 `boundary-red`，与该冻结版本一致；不改写成另一个不存在的最终源版本。

随后完成的候选结果已独立核对：

| 运行 | 实际结果 | 限制 |
| --- | --- | --- |
| `candidate-server-retry` | exit 0，350.07s；369 lib + 491 integration = 860 passed / 0 failed / 5 ignored | 以各 Cargo Running target 最后总结计数，排除嵌套 lifecycle child 的额外 1；真实 PostgreSQL `body_delivery` 8/8、config 5/5 与新恢复/内容用例逐项通过 |
| `candidate-embedded` | exit 0，108.12s；229 lib + 68 integration = 297 / 0 / 0 | 命令明确 `--skip` 尚未修复的 cue 边界 RED，lib 1 filtered out；不是最终不跳过全量验收 |

两次均为同一 `412ba53e…0d11c0` 冻结输入，`source_drift=[]`。Embedded 同 fd 读取、目录替换、清单字段匹配、篡改后原 receipt、late prepare/lease 和远端清单拒绝均逐项通过。附带提示音修复后的最终结果仍待 Root；Linux / 硬件 / 真实资产校准未因这些候选软件测试获得验收。

Server 首轮 `candidate-server.{log,json}` 已读回：exit 101，505.86s，同一 `412ba53e…0d11c0` 冻结输入且 `source_drift=[]`，日志末尾为唯一 lib 测试子进程收到 SIGTERM，没有完整测试统计。`database-interruption.json` 已独立读回：任务 PostgreSQL 于 08:48:23 UTC clean stop，先于 08:55:29 测试启动；不是 OOM，停止发起者未知，精确容器 events 查询无记录。连接超时后 Root 只终止测试子进程，重启同一任务容器，经 SELECT 1 返回 1、原 loopback 端口保留后执行同源 retry。本分工没有操作数据库，不把这次中止计作完整产品回归通过或捏造失败总数。

按 Root 要求追加只读可达性核对：`read_approved` 唯一生产调用来自 `plan::read_animation`，其上层 `prepare` 先完整校验 canonical command 和本地 catalog；因此 single leaf 与 1..=65536 byte bound 在 open/read 前已建立。`read_verified` 为私有 helper，生产只接受同次打开的 File；没有目录/leaf 验证后再以路径重开的旁路。此结论不覆盖尚未关闭的远程提示音任意文件写入。

## 最终冻结版本验证

Root 将用户确认的附带修复合入后冻结 `green-source-manifest.json`，原 bytes SHA-256 为 `76a90d0416ae60e8d316a27ff06a0a0457c785659feb8d5235b2d121dfcdf4b0`，661 输入。本分工独立计算 manifest hash 并逐个核对当前文件：**661/661 匹配，source drift 为空**。

已独立读取最终 `green-embedded.{log,json}` 与 `green-core-transport.{log,json}`，全部 `--all-targets --offline --locked`，均使用该 661 输入冻结版本：

| 最终运行 | 实际结果 | 重点证据 |
| --- | --- | --- |
| Embedded | exit 0，98.74s；236 lib + 70 integration = **306 / 0 / 0** | 没有 `--skip`，全部 0 filtered；cue 边界原 RED、cue_files 4 项、config 21 项、原 motion/lease/取消/owner 及本轮资产用例均通过 |
| Core / Transport | exit 0，2.42s；**41 / 0 / 0** | v2 签名域、旧 name-only 拒绝、精确内容身份验证再确认 |
| Server | exit 0，410.01s；369 lib + 491 integration = **860 / 0 / 5** | 真实 PostgreSQL body_delivery 8/8、config 5/5，原资产恢复、账本消失不重发与内容拒绝逐项通过；50 个 Cargo target 最后总结独立计数，排除嵌套 child |

三次合计 **1207 passed / 0 failed / 5 ignored / 0 filtered**，`validation-summary.json` 与原始日志、metadata 及本分工独立计数一致，全部 `source_drift=[]`。Server 因 dev-dependency 同时编译 Embedded 的最终组合回归也已完成；没有借用先前候选结果。

这些最终结果关闭普通提示音上传覆写与 remote audio_dir / catalog 改写的已审软件缺口；宿主插件部署权限、真实硬件与跨 FS/DB 事务仍按已声明边界。本分工已核对顶层新 checklist、恢复检查点、Root HANDOFF 和 package configuration/motion 文档，除 Root 预告的最终补记外，与实现及验证范围一致。5 ignored 为真实 PTY / 模型类未执行项；Linux 应用 / Pi / ALSA / I²C / UART / 真实录放音 / release / 长稳均不冒充本轮通过。历史候选的跳过、环境中止与原 RED 均保留，不被本节覆盖。

## 实现提交与资源终态

已独立读取 `commit-verification.json`：实现提交 **`e7265f0d0e9be8d6e663bfcabd53739b32206e27`**，Root 使用 `git cat-file --batch` 将已提交 blob 与最终冻结输入逐项比较，**661 blobs、mismatches=[]**；manifest 仍为 `76a90d…dfcdf4b0`。本分工此前独立对同一 661 工作区输入算 hash 匹配，不把未运行的文档提交当作新源码验证。

资源清理存在真实异常，已独立读取 `cleanup.json`、`inspect-cleanup.py`：首次 cleanup 脚本在检查第三个既有容器 astrbot 时断言失败（exit 1）。Root 报告该点之前已核对本任务容器身份/label，成功 stop 与 `rm -v`；首轮 stdout 仍在内存、未随异常写盘。后续只读复核持久记录确认本任务 `alia-r03-a-20261014-db` 已不存在，Home Assistant 与旧 alia-postgres 均停止，astrbot 于 08:48:26 UTC 再次启动且当前 running，发起者未知。Root 据此保留 astrbot 和共享 OrbStack，未再次停止无关工作。

因此本轮仅确认自己的测试数据库已移除、运行测试均退出；**不声称整个环境恢复原 Stopped，也不声称初始自动恢复容器没有其自身副作用**。匿名卷删除依据 Root 已执行的 `rm -v` 及脚本控制流记录，未另外执行宿主资源检查。没有 Pi / SSH / 硬件操作。当前审查无阻断，本分工文件到此停止写入；Root 负责顶层终态补记、归档哈希与文档正常提交。
