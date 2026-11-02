# Server／Embedded 成套升级与失败恢复

更新：2026-11-02。本页是现有软件改动的成套升级步骤，对应 [S03](../todo.md)。本轮已在隔离环境验证数据库巡检恢复修复，但 Linux Resume 两次失败的原因仍未确定，新源码／二进制暂不放行；实际生产恢复使用既有 `8481b0f`，并完成用户选定的主机名 endpoint 同身份迁移。生产休眠／数据库暂不可达的底层触发仍未证实，真实效果未知结果不自动重派。各恢复点保留其实际验证范围，不能代替实际双端恢复；Pi 保持全 noop，10 月 31 日新增 R1 claim 必须保留。实际产物和验证见[软件发布记录](software-release.md)。完整语音链、AEC 仍分别待验，ROS 和 R03 仍暂缓。

新安装的版本目录、Mac LaunchDaemon／Pi systemd 模板、配对和日志入口见[无前端部署操作](server-embedded.md#mac-与-pi-的安装和常驻入口)。当前 Mac 实际安装采用用户 LaunchAgent，见[Mac 正式启用](server-embedded.md#mac-正式启用2026-10-28)；通用系统域模板仍为备选材料；Pi 当前无音频配对与真人窗口边界见下节。

## 数据库恢复修复与地址迁移准备（2026-11-02）

V03 的隔离复现已确认：Execution／Delivery 的安全巡检第一次触及数据库期限就会永久取消本实例，数据库重新可达不能恢复已取消的 worker。最小修复允许安全扫描和过期状态收敛在原周期内继续：Execution 失败时暂停派发并显示异常，成功扫描后恢复；Delivery 回收失败时拒绝新 admission、保留原 owner，两个回收查询成功后恢复。初始启动仍要求账本可用，关闭时仍报告未恢复的扫描错误。Memory 将扫描错误与投影写入错误独立记录，成功空扫描不清未确认的投影写入故障。

这些恢复查询只能读取意图、将过期记录收敛为 unknown／cancelled／retired，不能产生新的派发许可。claim 提交结果未知、已派发效果回执未确认、真实输出控制和取消失败继续保持原失败状态；按原 effect／response／command 身份查询和对账，不用“扫描恢复”作为重发外部效果的依据。Execution 的持久 submit 仍受原期限／容量约束，扫描暂停期间可入队，不能推断所有 admission 都已关闭。详细反例、逐目标验证与首次失败见[本轮软件记录](software-release.md#数据库恢复修复与候选准备2026-11-02)。

真实 PostgreSQL 锁定测试保持原 2 秒数据库期限和既有并发，验证解锁后同一实例恢复、原身份重试只有一次派发。生产库、休眠和设备均未用于这些故障注入；本轮不据此关闭 10 月 31 日底层触发问题，也不把后续健康成功等同未知 Delivery／output-control 账本已全面对账。双平台构建和影响验证有实际结果，但 Linux 同一 Resume 用例先后两次失败，独立记为 V05；第三次只加诊断后通过，没有提供根因。新 D2 本地代码提交 `e2831e4408b868adb3e61ff1b90a7417703ef5c4` 和构建产物因此保留待查，不安装、不打包为通过候选，生产仍使用旧 C。

本轮还确认 Mac 的旧字面 IP `.201` 已变化，用户选择固定使用 `http://Ke-QiaoMac.local:3000`。Pi `.221` 已启用系统现有 resolver 的 mDNS **resolve** 模式，只加两份指定 drop-in，没有安装额外 daemon、广播服务或重启网络／Embedded；实际解析为 `.191`，见[解析变更](../../run/software-recovery-20261102/network/mdns-applied.json)。这项地址漂移与数据库巡检故障分别记录。

Pi 在 `/home/ke-qiao/.local/share/alia/backups/mdns-before-20261102` 保存变更前私有恢复点，原 identity／binding／SQLite 和完整 claim 保留。准备时原 noop PID1648、disabled／Restart=no、PCM closed，binding endpoint 尚为旧 `.201`，见[准备时检查](../../run/software-recovery-20261102/network/pi-noop-prepared.json)。其 SQLite 备份副本已在 Mac immutable／read-only 模式核对 integrity、state、绑定和音量，设备身份摘要／已存公钥半部及完整 claim 摘要一致；未将副本还原到 Pi，也未启动第二个设备身份实例，见[Pi 副本核验](../../run/software-recovery-20261102/restore/pi-summary.json)。不能用 10 月 30 日的空 claim 集合覆盖现有初始化记录。

Mac 的配置只改 `server.host` 为 `0.0.0.0`、新增上述 `server.public_endpoint`，原 channel／owner／device pin、Server key、模型角色与预算、数据库和关闭 discovery 的设置保持。Mac 自身解析主机名会先取得回环地址，因此不能直接把主机名当 listener；`0.0.0.0` 实际监听所有 IPv4 接口，保留现有认证和双端公钥核对，不新增公网转发。见[配置语义差异](../../run/software-recovery-20261102/network/server-config-prepared.json)。旧组件诊断限定无 channel，检查此生产副本时被其范围门禁拒绝，见[原检查结果](../../run/software-recovery-20261102/network/server-config-only.json)；随后用实际 `ServerConfig::load` 核对，原失败仍保留。

旧 Server 停止实际 exit 1，不能记为正常关闭；本轮在停止前后分别核对 Execution effects／outbox、Responses／Deliveries、output-control scopes／commands 六表均为 0，旧进程退出且数据库其他连接归零，未重派任何效果。私有 `/Users/ke-qiao/Library/Application Support/Alia/backups/pre-recovery-mdns-20261102` 保存该停服点，见[原停止与恢复点](../../run/software-recovery-20261102/network/old-server-checkpoint.json)。其 dump 已在 network-none 临时 PostgreSQL 实际恢复，67 张非系统表的 count 与排序 JSON 内容摘要逐表一致，生产表摘要前后不变；本次使用 no-owner／no-acl，未另外比较序列状态和角色／ACL，不能借用旧演练的更广验证范围，见[本次隔离恢复](../../run/software-recovery-20261102/restore/summary.json)。

随后恢复的是既有已验证 `8481b0f` Server 和新名称配置，未安装 D2。首次启动门禁因 LM Studio 停止而拒绝；恢复原 loopback1234、选定 Qwen embedding／8192 context 后通过，Server PID50935 约 5205 ms 就绪，账号、profile双pin、33迁移和 shadow 保持；overall degraded 仅为三项有意关闭的 LLM live probe，见[实际恢复](../../run/software-recovery-20261102/network/existing-server-mdns-resumed.json)。这次手动修复依赖不等于自动登录／进程故障恢复已通过。

既有签名 PrepareHandoff／CompleteHandoff 各执行一次并成功，让同一 Server／设备／owner 更新 endpoint；没有直接改 SQLite、删绑定或生成新 key。Handoff 按协议更新 bound_since 并撤销旧 output generation，不能要求该时间戳等于迁移前。迁移后及一次普通 noop 重启后，持久 endpoint 均为 `http://Ke-QiaoMac.local:3000`，签名设置读取成功，缺失／伪造认证均拒绝 code16，claim 摘要保持，见[一次移交](../../run/software-recovery-20261102/network/handoff/execution.json)、[重启后认证](../../run/software-recovery-20261102/network/pair-after-noop-restart.json)及[迁移汇总](../../run/software-recovery-20261102/network/pair-migration.json)。Pi 二进制未变，音频／初始化请求为 0；mDNS 的整机重启持久性未测。以后 Prepare／Complete 若超时，先查原身份和持久状态，不盲目重试或以旧数据库覆盖未知结果。

迁移后两端另建 `backups/paired-mdns-20261102` 私有快照，保存当前 C 二进制身份、`.local` 配置、SQL／SQLite、设备身份和完整 claim。Mac dump 在隔离 PostgreSQL 还原后，67 张表的行数／排序 JSON 摘要与 dump 前、后及还原后观察完全一致；Pi 副本只读检查为 Bound `.local`，原 claim 保持，见[迁移后恢复点核验](../../run/software-recovery-20261102/paired-restore/summary.json)。这是相邻时间分别取得的在线快照，不是跨端原子快照；角色／ACL、序列状态和恢复副本的应用启动未验，也未覆盖当前生产数据或现场恢复 Pi。

Pi 服务单元与两份 mDNS drop-in 另存私有 `backups/paired-mdns-network-20261102`，与应用快照成套恢复，见[网络与服务材料备份](../../run/software-recovery-20261102/network/network-materials-backup.json)。Mac 私有管理员登录材料仅更新 URL，密码不变，原冻结恢复点未改写，见[登录入口协调](../../run/software-recovery-20261102/network/admin-login-endpoint.json)。这些恢复点描述当前运行的 `8481b0f`，不包含尚未启用的 Rust 恢复修复。

若需撤回 resolver 变更，只删除本轮两份新增 drop-in，按其记录 reload 网络定义并恢复 resolver 状态；该操作不自动回滚已经提交的 binding。endpoint 的恢复必须按实际持久绑定和同身份移交流程处理，旧 `.201` 未重新取得可达证据时不能直接换回。所有回退均须保留后来新增的完整 R1 claim；**数据库迁移后不能只换回旧二进制**。下一次用户在场确认 R1 电源事实后，可先在现有 C 上执行受控短句验证，不以安装 D2 为该有限窗口前提；完整链路与正式发布仍受 V05 等未验门槛约束。实际双端升级／恢复、登录及异常恢复、24 小时混合故障／资源上界、AEC 仍分别待验。

## 真人窗口后的恢复边界（2026-10-31）

这次实际使用了生产 voice 配置并完成唯一一次已确认板端冷启动初始化。新增 claim 为 `state/embedded.sqlite3.r1-initialization/d9b21420-fa34-43ee-bc55-2406fc696442.jsonl`，SHA-256 `45103ccdf47cc0ea1227e04db8145ede5feff349307e4ac0b6c13646abfe95ee`，含先行保留与 command_accepted 两条记录。Pi 私有 `backups/voice-window-20261031` 保存该 claim、语音配置、身份与 SQLite；`backups/pre-voice-20261031` 是窗口前备份。此新快照未做还原演练，不是已验证可替代整套双端恢复的产物。

窗口两次会话都在 turn model deadline 结束；停止语音进程 exit 0，恢复原 all-noop 配置后正常运行，PCM 全闭、硬件 FD 为 0，claim 仍在且摘要一致，见[实际恢复检查](../../run/voice-acceptance-20261031/pi-restored-noop.json)。恢复的是配置／进程模式，没有回退 SQLite 或清除防重记录。下方 10 月 30 日快照中的空 claim 只是当日历史；**任何较旧快照恢复都须保留后来新增的完整 claim 集合，不能用普通进程重启、新 UUID 或换 state_path 重发初始化。** 若板端再次真正断电，须另行确认并记录新的真实冷启动事实。

Mac 本次故障恢复有明确失败边界：数据库直接查询／登录已恢复时 Execution worker 仍处于 fault；SIGTERM 实际 exit 1，包含 Delivery 数据库 deadline／提交结果未知。原始结果见[停止记录](../../run/voice-acceptance-20261031/server-stop-outcome.json)。关闭日志记录 Execution `pending_effects=[]`，不等于 Delivery／output-control 账本已全面对账；保留未知结果后仅做一次受控重启，健康恢复；这不是“正常关闭通过”，也不证明数据库暂断或合盖后可自动恢复。不能仅凭后续 health 为绿消除未知提交结果，未来存在真实业务记录时须按持久状态查询／对账，不能盲目重派外部效果。

下一步隔离验证暂不可达／恢复／退出流程，保留现有数据库期限，并区分 worker 真故障与过期健康错误。之后才在合适维护窗口验证实际登录、休眠／唤醒和成套升级恢复；不为验证中断当前生产数据库或重复现场语音。**数据库迁移后不能只换回旧二进制**的边界保持；24 小时混合故障负载、AEC 与具体插件验收仍分别开放。

## Pi 无音频配对后的恢复点与真人窗口（2026-10-30）

本轮已安装同批候选的 Pi Embedded，保留原设备身份，以全 noop 配置完成真实生产配对和已绑定的签名设置读取；缺失／无效鉴权均被拒绝。配对证据见[本轮记录](../../run/pi-production-20261030/pairing-checks.json)。语音待用文件 `/home/ke-qiao/.local/share/alia/config/embedded.voice.pending.toml` 仍未选为运行配置，不把配对成功写成 PCM、模型或播放通过。LAN、无音频普通重启和最终运行状态由本轮部署证据另行记录。

已建立以下成套恢复点；它们含私有身份／配置，不纳入 Git 或公共发布包。路径与摘要见[配对恢复点](../../run/pi-production-20261030/paired-checkpoint.json)：

| 端 | 已有私有恢复点 | 本轮核验范围 |
| --- | --- | --- |
| Mac | `/Users/ke-qiao/Library/Application Support/Alia/backups/pi-paired-20261030` | config／secrets／services／PostgreSQL manifest 与在线事务 `pg_dump`，保留当时配置和 dump 摘要 |
| Pi | `/home/ke-qiao/.local/share/alia/backups/paired-lan-20261030` | active／voice.pending 配置、identity，以及 SQLite backup API 副本；integrity 为 ok，state 行摘要一致，当前 claim 为空 |

**这份 10 月 30 日恢复点尚未执行还原。** 10 月 28 日初始恢复点的隔离 SQL 还原结果不能覆盖新增绑定和 LAN 配置；后续在隔离环境还原、核对绑定／账号／policy／设备状态后再补本节状态。更早的 Mac `backups/pre-pi-20261030` 和 Pi `backups/paired-pre-lan-20261030` 是变更前保留点，不能当成本轮最终配对状态。恢复必须继续成套保留双端身份、数据库、SQLite、模型与服务配置；数据库迁移后不能只换回旧二进制。

[真人窗口准备与配置审核](../../run/pi-production-20261030/human-window/README.md) 对比了 10 月 24 日 R1 系统控制基线：模块、串口、16 kHz mono 采集、48 kHz stereo 播放、线性增益、独立时钟和 system trigger 一致；生产身份、持久路径、LAN 地址和 channel 为明确变更，Silero 文件摘要一致。旧现场二进制与当前候选不同，不能写成同一产物已现场验收。USB mixer 的 −14 dB 是历史证据，本轮未重新读取，须在真人窗口前只读确认；不自动改音量或重复旧硬件基线。

进入语音窗口须先确认本次**真实板端冷启动**，再记录该次启动唯一的新非 nil UUID；当前没有预先声明 cold-boot ID。空 claim、Linux／设备 boot ID、配对或进程重启均不能代替物理冷启动确认。选定已审核的 voice 配置后，由同一个长期进程执行一次 `--initialize-r1`，保留原固件检查、beam 0 与期限。按[初始化操作](server-embedded.md#mac-与-pi-的安装和常驻入口)移除临时 override，但不重启已保持同一采集流的进程；失败或结果未知也不自动重发。

生产 claim 路径为 `/home/ke-qiao/.local/share/alia/state/embedded.sqlite3.r1-initialization/`。**回滚不能删除恢复点之后新增的 claim。** 还原旧配置／SQLite 前先独立保留当前完整 claim 目录；同 UUID 的冲突先保留两份证据并诊断，不用旧备份覆盖、换 state_path 或新 UUID 绕过防重。

现场顺序为生产启动 → 签名 Inspect／Start → 短句与正常换气长句 → 模型／回复播放 → 打断／恢复 → 当前输出取消 → Inspect／Stop → 新 Start 与下一轮对话。准备的控制诊断默认只做离线参数检查，未执行真实 RPC；Start／Stop 要显式现场开关、前次成功 boot／generation，保留原 5 秒和零重试。公开 BindingStatus 比对不是 challenge-response，成功 Start 也不等于 Server Ready。输出 Cancel／Hold／Resume 与 Capture Stop 是不同操作；口头“结束对话”尚未映射到底层 Stop，不能用模型文字回复代替控制验收。

输出控制的现有入口另见[暂停／恢复／取消材料](../../run/pi-production-20261030/human-window/output-control.md)：本人 `POST /api/v1/responses/{response_id}/cancel` 配合原Response／Delivery／events读回；Hold／Resume走真实VoiceSession语义链，无独立HTTP控制路由。现场按当前boot／SessionReady／owner取得动态response_id，不能用Capture generation或检测request_id代替。材料中的只读身份SQL已在当前生产schema以2秒期限完成PREPARE／DEALLOCATE校验，未执行SELECT或任何控制请求。

生产初始化后的真人语音、完整中断／取消链、这份新恢复点的隔离还原、实际双端升级／恢复及至少 24 小时混合故障／资源上界仍待验证。AEC 在商家确认后单独验收，通用 MCP／插件测试不替代首版具体接入，Companion 仍保持 shadow。

## 新生产数据来源决定（2026-10-28）

用户已决定弃用并删除两套旧 PostgreSQL，**不从旧库或备份恢复新生产初始数据**；执行状态及准确范围见[当前部署决定](server-embedded.md#当前部署决定2026-10-28)。用户随后明确授权临时启动引擎；两套容器和专属数据卷已删除并核验不存在，删除阶段曾将 OrbStack 恢复停止；后续用户授权 Mac 正式启用后，已新建 `alia-production-pgdata`／`alia-production-postgres-1`，引擎随生产数据库保持运行。首轮采用 `ke-qiao` 登录会话；minor 已定官方 DeepSeek Flash＋none。

因此下面 10 月 25 日“检查旧库 pin → 显式切换 → 重建历史投影”的步骤保留为已演练的未来迁移流程，不再作为本次新库首次启动的必经步骤。新生产库已使用选定的本地 1024 维 embedding，首次迁移／bootstrap 后核对了实际 SQL descriptor、Memory／Cache pin、`ke-qiao@alia.local` 管理员 UUID 和 Companion shadow；不导入旧账号或测试 fixture。保留 `alia-main` 密钥不等于保留已删数据库里的应用账号／绑定。

新空库流程已在独立 PostgreSQL17.10／pgvector0.8.2 中用冻结 Mac release 实际验证：33 个 migration journal checksum 与源码逐项一致；Memory／Cache 均直接初始化到选定的本地 Qwen1024 descriptor、policy version1；四张 canonical／cache／projection 表为空；真实 HTTP 首个注册为 admin、第二个为 user，两用户 Companion shadow revision1。正常关闭后同身份重启，登录、角色、shadow、profile、journal 和空表保持，两个进程均 exit0、DB其他连接归零，临时容器／网络／私有状态清理，OrbStack恢复停止。见[结果](../../run/fresh-start-20261028/database/summary.json)。没有模型调用；health 中 embedding 因故意不提供真实凭据而 unhealthy，不能作为模型就绪证据。

两次准备失败保留：internal Docker 网络没有按请求发布宿主端口，Server尚未启动；修正为独立bridge＋明确loopback发布。随后新隔离配置关闭两项routing开关，被校验器在 **33条迁移提交后**拒绝；改为开启noop minor路由，不更改应用、期限或断言。后者再次说明启动失败不表示数据库迁移未发生，不归入V01／V02。最终成功没有覆盖首次日志；本轮不重复旧profile切换／备份恢复演练。

本次不使用旧备份作为数据来源，不取消上线后的备份与恢复要求。首次新生产 Server 正常 SIGTERM 退出、数据库其他连接归零后，已在私有目录 `/Users/ke-qiao/Library/Application Support/Alia/backups/initial-20261028` 建立新的恢复点，包含 custom-format `database.dump`、配置、身份／凭据来源、服务文件和 PostgreSQL manifest。该目录含敏感数据，0700／0600，仅记录摘要，不纳入 Git 或公共交付包。

该新恢复点已在**无网络、无发布端口、临时 tmpfs PostgreSQL** 中实际 `pg_restore --exit-on-error` 成功，67 张 public 表的行数及排序 JSON 内容摘要逐表一致，包括迁移、管理员、shadow 和 profile；检查期间生产表摘要未变，临时容器已删除。见[恢复点](../../run/mac-activation-20261028/checkpoint.json)和[隔离恢复](../../run/mac-activation-20261028/restore-checkpoint.json)。这证明本次初始数据恢复点可读且 SQL 内容一致，未在恢复副本运行应用、连接模型或设备，也未将备份还原到生产。

后续每次实际模型／配置／二进制变更及业务写入后仍需建立对应的新恢复点。初始恢复点记录的是首次软件候选，不自动包含之后的 TTS 健康检查修复二进制；运行版本按[发布记录](software-release.md#mac-正式启用与健康检查修复2026-10-28)对应。**数据库迁移后不能只换回旧二进制**；双端现场升级／恢复、设备状态与至少 24 小时验收仍未完成。

## 选定 embedding 的切换与隔离恢复（2026-10-25）

生产候选配置、覆盖关系、Settings PATCH 和只读 SQL 已准备，并在 Mac ARM64＋专属 PostgreSQL 17.10／pgvector 0.8.2 中完成一次合成数据演练。运行时代码仍为软件候选 `6123691`，本轮只增加操作材料与验证证据，未切换生产配置／数据库，也未部署 Pi 或启用音频。完整结果见 [summary.json](../../run/production-config-20261025/summary.json)。

以下为 10 月 25 日历史过程，数据来源建议已被上方最新决定取代。后续只读盘点找到了两套保留的 OrbStack PG17 数据：当前 Rust 项目 `project-alia_pgdata`／宿主5433，历史 Python 卷／5432；compose 和旧 Server 配置存在端口漂移。两者仍停止，未读取实际 SQL。进入生产恢复前先按[本机盘点](server-embedded.md#本机数据与常驻准备盘点2026-10-25)确认权威数据，优先用停服副本隔离查询；不要直接 compose up、新建空库或把历史 fixture 账号写成生产 owner。

### 配置与身份准备

- 使用新目录中的[待部署配置](../../run/production-config-20261025/config/server.production.pending.toml)，逐项填实服务账户、私有目录、数据库、原账号、设备和网络身份。当前文件保留 loopback、关闭 discovery／channel、DB port 1 和空 JWT 的准备状态，**不可直接部署**。[隔离配置](../../run/production-config-20261025/config/server.isolated.toml)用于派生无设备演练，不是生产启动结果。
- base → 排序 `.d` 深合并 → 环境覆盖／补空 → 持久 settings → SQL policy 的区别见[配置说明](../../run/production-config-20261025/config/configuration-notes.md)。不得把旧 `server.d`／`settings.toml` 整体复制到新目录；本轮实际 preview 证明，旧 settings 会把选定的本地 1024 维 adapter 改回 OpenAI `text-embedding-3-small`／1536，而期望 role 文字还可能显示 `local_embedding`。生产副本应逐键保留必要业务设置、剔除冲突模型键，不能全量删除旧策略。
- GPT-SoVITS 初始装配早于末尾 settings 热应用，base 必须直接指向 Alia 独占 `127.0.0.1:19880`，不能靠晚到的覆盖避免误连旧共享实例。Companion 是每用户 SQL 政策，开放前核对实际 revision／shadow，不能由 TOML 默认值推定。
- 精确固定 embedding descriptor：provider=`openai`、endpoint=`http://localhost:1234/v1`、model=`text-embedding-qwen3-embedding-0.6b`、dimensions=`1024`、revision=`request-config.v1`、cosine/vector；Rust 得到的 ID 为 `alia.embedding.d07c6780f61ebf58ba106d7e1cee0a5cfd4405ba6e2c4c62f04690811978aa19`。`localhost` 与 `127.0.0.1` 是不同请求身份；SQL pin 不包含实际权重字节，仍须另核对已固定 GGUF／LM Studio 资产。
- 受控环境注入的 provider token 会在正常 Settings PATCH 后由应用保存到私有 `settings.toml`。该文件也属于密钥文件：新文件 0600，父目录 0700，备份仅存受控私有位置；不要归档完整 Settings GET。本轮只保存临时私有文件摘要，原件已删除。

### 切换与观察

1. 在原入口隔离、写入者归属清楚并完成停服点备份后，核对实际数据库及[只读 profile 概况](../../run/production-config-20261025/sql/inspect-profile.sql)。当前旧配置指向 `localhost:5432/alia`，本轮探测为连接拒绝；**真实生产 SQL 当前 pin、数据量和账号尚未取得**，不以合成库替代。
2. 用原管理员 GET `/api/v1/settings` 取得 ETag，然后 PATCH 同端点，带最新 `If-Match`，提交[三字段 payload](../../run/production-config-20261025/config/embedding-switch.patch.json)：

   ```json
   {
     "core.providers.llm.openai.base_url": "http://localhost:1234/v1",
     "core.profiles.llm.embedding.model": "text-embedding-qwen3-embedding-0.6b",
     "core.profiles.llm.embedding.dimensions": 1024
   }
   ```

   即使当前文件已经是这些值，也须显式执行已审阅的 profile transition。不要直接 SQL UPDATE pin、删除 descriptor 或改 policy version。descriptor 登记与 Memory／Cache 两个 pin 的 CAS 同事务；已有库仅启动新配置不会自动切 pin。
3. 同时检查 HTTP 状态、`success`、`context_policy_version`、`module_reports`、`applied`、`ignored_runtime`、`errors`，再核对 SQL、adapter preview 和持久设置。SQL 提交先于模块 apply／文件保存；HTTP 200 `success=false` 或请求超时不能证明没有改变。409 应重新读取、审阅后再操作；部分成功时入口继续关闭，针对原意图修复并读回，不通过重启或恢复旧 settings 猜测状态。
4. 保持 Memory 可读，使用原 worker 重建；不靠禁用 Memory／设置 pause 隔离入口。canonical 行就是持久工作来源，没有独立 job 表要迁移。每批 16、inline／后台共享并发 2，inline 2 秒、后台 30 秒、DB 2 秒、扫完等 30 秒，均保持原值。旧投影保留并按当前 profile／revision／输入摘要隔离，规范内容更新会使旧投影失效。
5. 运行[缺投影查询](../../run/production-config-20261025/sql/missing-projections.sql)并逐类解释差额。SQL 候选还须经过 Rust 的敏感文本、来源根／撤销、长度等准入，不能要求所有 canonical 行都有向量；健康状态 available 也不代表全库完成。核对可重建记录的维度、revision、输入摘要及实际 `profiled_vector` 检索，私有／撤销记录保持排除。

**Memory 重建不等于 Cache 全量重建。** 旧 active Cache 没有后台 backfill；其新向量由原 pending Insights review→approve 流程逐步生成。切换后旧内容／向量保留，但旧 ContextLease／profile 不可复用；不能重标 pin 或清空内容制造完成。本轮初次语音配置关闭 Insights，故不承诺自动补齐旧缓存。

### 本轮实际演练与恢复边界

| 检查 | 实际结果 |
| --- | --- |
| 新配置＋旧 SQL 身份 | 仍保留旧 pin，非空 canonical 关键词查询可用，不混用旧向量 |
| Settings CAS | 预设错误版本收到 409 且 policy 不变；合法 PATCH 成功，Memory／Cache pin 同步 |
| 原 worker 重建 | 3 条可投影合成记忆在 30.251 秒内取得选定 1024 维投影；4 条 canonical（含 1 条 private_runtime）不变，私有记录无投影；seed fixture 记录其文本未发送 |
| 同维 endpoint／前向回切 | localhost→127.0.0.1 生成独立 profile，不借用旧向量；用新 ETag PATCH 回选定 localhost profile，保留 canonical 并复用有效投影 |
| 重启补建 | 正常停服后仅删除独占演练库的一条投影；重启原 worker 在 104ms 观察内补齐，持久设置胜过故意陈旧的 base，SQL pin 保留 |
| 切换前备份／还原 | 源库 67 表、121 行、33 条 migration journal；恢复新库逐表行数和稳定 JSON 摘要一致 |
| 切换后备份／还原 | 源库 67 表、126 行、33 条 journal；恢复新库逐表一致；两次均另核对 owner／ACL、public schema／扩展版本、序列值及 is_called |

旧 1536 维来自本轮确定性 fixture，只有新 1024 维调用固定本地模型；同维回切并未启动历史 Nomic 或云端 OpenAI。前后恢复分母独立，不相加。两次都在任务连接归零后执行 `pg_dump -Fc`、`pg_restore --exit-on-error --single-transaction`，没有关闭 owner／ACL 检查。owner／ACL 检查覆盖 public 关系和 namespace，不是全部 DDL／函数权限或集群角色恢复验证。恢复副本做了只读核验；重启补建是在原隔离库验证，**没有启动恢复库、旧二进制或真实生产数据**。原始记录见 [rehearsal.json](../../run/production-config-20261025/rehearsal.json)、[transition](../../run/production-config-20261025/sql/transition/result.json)、[前备份](../../run/production-config-20261025/database/before-restore.json)、[后备份](../../run/production-config-20261025/database/after-restore.json)。

若只需退回旧 embedding 配置，优先保留当前数据库与 canonical 增量，在旧服务仍可用时，以最新 ETag PATCH 经核对的旧完整 descriptor，再由原 worker 补缺；这不是倒退 policy version，也不能复活已删除内容或撤权。需要退回旧应用 schema／二进制时仍遵守下文成套恢复规则：有新业务／外部效果先保留增量并前向修复；**数据库迁移后不能只换回旧二进制**。

本轮未改变生产、模型资产或软件候选；自建 PG／临时凭据目录已删除，新增 embedding 实例已卸载，原 Instant 保留。准备阶段一次 sandbox `lms ps` 查询超时 exit 1，原始错误保留，随后认证 HTTP 目录核对成功；它不是推理失败，不能据此解释历史 V01／V02。两个配置解析、两份只读 SQL、有限切换／重建及两次恢复均通过；未重复完整 workspace 或前轮已通过的组件调用。

## 本次升级边界

| 对象 | 新版要求 | 不能沿用的假设 |
| --- | --- | --- |
| 双端 Motion | `alia.motion.v1.MotionControl/Exchange`，`SignedMotionPayload`，JSON version 2；签名域分别为 `alia-motion-v2-request`／`alia-motion-v2-reply` | 旧 Body RPC 和签名域没有兼容入口；Server 与 Embedded 必须同批切换 |
| PostgreSQL | 新增 `m20261019_000001_rename_body_delivery.sql` 和 `m20261019_000002_embodiment_operations.sql` | 迁移后只换回旧 Server 二进制不能恢复服务 |
| Server 表达插件 | `plugins.wasm_dir`，包 ABI 1、表达 ABI 3，显式执行 grant 和独立 Appraisal pin | 旧 JSON-lines／Python 入口、`resources.external_expression_plugins_dir` 及旧 manifest 字段不再接受 |
| Embedded 插件 | WASM processor 或经额外授权的 `trusted_native` 设备驱动；启动传入 `expected_asset_sha256` | 旧 processor 不能通过改标 native 取得宿主执行权，安装成功也不等于运行获批 |
| WASM 身份 | 模块、manifest、限额、运行器标识和宿主可执行文件内容共同决定 pin | 包没有变化也不能在宿主字节变化后复用旧 grant；旧 Appraisal pin 不批准新实现 |
| 长句 VAD | `modules.vad="silero"`，现有 Silero ONNX 路径；移除旧 `[vad].max_speech_duration_ms`／`buffer_size_sec` | 旧 `sherpa_onnx` VAD 模块名及旧参数明确拒绝；wakeword 模块名不变。模型文件仍需部署，配置预检不证明文件可加载 |
| R1 系统触发 | 独立 `embedded.r1-system.example.toml`、`speech.trigger="system"`、签名 CaptureControl；自然唤醒模板单独保留 | Start／Stop 控制 Alia 会话，不控制物理麦克风电源；普通进程启动不发送 UART 初始化，不能把 boot ID 当作板端冷启 ID |
| R1 初始化防重状态 | `<device_runtime.state_path>.r1-initialization/` 目录及持久 claim 与原配置／身份一并备份，例如 `embedded.sqlite3.r1-initialization/` | 不删除 claim 来重试，不因进程重启生成新冷启 ID；显式初始化只用于已确认的真实板端冷启动，固定 beam 0，不自动重发 |
| 绑定 handoff 恢复 | 普通 Bind 只接受 Unbound；handoff token／目标仅在原进程存活时有效 | HandoffPending 不能用普通 Bind 覆盖。Embedded 重启会恢复并持久化原 Bound 身份，原 Server 须重新授权 handoff；不能凭旧 token 或新自签 Server 接管 |

详见 [Motion 契约](../architecture/motion-contract.md)、[Embodiment 契约](../architecture/embodiment.md)、[WASM 部署](wasm-plugins.md)和 [R1 启动契约](../../packages/alia-embedded/docs/audio-and-voice.md#r1-system-controlled-capture)。Expression 仍只产生候选计划；升级不会授予 ROS、导航或其他设备控制能力。

## 升级前准备

1. 固定同一源码快照构建的双端产物、目标平台、构建 features、`Cargo.lock`、Rust PDK 产物和 SHA-256；保留原版本完整产物。按 `scripts/verify-software.sh release` 取得对应平台证据。Mac 产物不能替代 Linux ARM64 产物，旧 Harness release 不能替代 Embodiment release。
2. 按下表登记实际路径及服务归属，在私有目录准备新版配置副本。Server 的主文件、排序 `.d` 分片、环境来源、`settings.toml` 和 SQL 政策共同决定有效配置，不能只检查主文件。核对实际 `[[channels]].id` 与 Embedded `speech.channel_id`、原账号、绑定、设备公钥 pin；不要重新生成身份来绕过不匹配。
3. 从 Server 配置及分片移除旧表达子进程配置，按 [WASM 迁移说明](wasm-plugins.md#从旧插件配置迁移)转换包。Embedded 删除退休的 `[audio_debug]`，复核新的插件 manifest、校准文件和双端 `approved_animations` 内容身份。保留原文件作为私有备份，不把旧目录重新接入新执行器。
4. 对 Embedded 配置副本执行新版二进制的 `--check-config /absolute/path/embedded.toml`。该入口不启动设备；普通 Embedded 启动会打开所选采播／设备模块。Server 当前没有仅迁移或只读配置检查的 CLI，普通启动会写密钥／持久状态、执行迁移并继续初始化，不能作为生产库的无副作用预检。
5. 先在隔离数据库和独立目录演练恢复。使用[全 noop Embedded 模板](../../config/embedded.staging.example.toml)和明确的 Server noop 配置，关闭外部 MCP、模型和设备连接；不要直接启动复制来的生产配置。副本包含绑定凭据时，网络也必须与真实设备隔离，不能让第二个实例冒充原 Server。

| 需要成套保留 | 配置位置或内容 |
| --- | --- |
| Server 配置和身份 | `ALIA_CONFIG_PATH` 指向的主文件、同名 `.d`、相邻发布恢复记录（若存在）、配置目录下 `settings.toml`、`auth.server_keypair_path`、认证 secret 的受控来源和服务启动环境；实际 launchd plist／systemd unit／override 一并留存 |
| Server 数据 | PostgreSQL 应用数据、`schema_migrations`、账号／绑定／Execution／Delivery／Embodiment／政策；记录 PostgreSQL／pgvector 版本及所需角色权限 |
| Server 扩展 | `plugins.wasm_dir` 整个包目录（含资产和 grant）、MCP 配置和凭据来源；执行 grant 与 SQL 中 Appraisal 批准分别保留 |
| Embedded 配置和身份 | `ALIA_EMBEDDED_CONFIG_PATH`、`identity.keypair_path`、原 device ID、`device_runtime.state_path` 的 SQLite 状态及仍存在的伴随文件；R1 的 `<device_runtime.state_path>.r1-initialization/` claim 目录也必须完整保留 |
| Embedded 资产 | `device_runtime.plugins_dir`、本地校准／批准动画和 `direct_response_defaults.audio_dir`；外部绝对路径指向的文件也需登记 |

相对路径均以各自主配置目录解析。备份应限部署操作者读取；设置 GET 可含 secret，不能把完整响应放进公共日志或 Git。恢复前核对备份完整性及隔离还原结果，只有 dump 文件存在不算完成恢复演练。

部署清单还应列出外置 Silero ONNX、Easy-Turn、TTS 等服务的版本／权重摘要和实际路径。大模型可以按已固定摘要从原受控位置恢复，不要求塞入应用源码包；但备份必须足以恢复这套配置所引用的资产。保留 R1 claim 目录并不能自动证明板端仍处于同一次冷启动，恢复后仍按实际电源状态判断。

## 停止、备份与切换

1. 暂停新业务和管理写入，包括自动触发与外部调用方；当前没有可一键隔离全部入口的 maintenance API。保留必要的查询／取消通路，记录原 Effect、Response、Motion delivery、Operation 和插件 command ID。未知效果先查询原身份，不能用新 ID 再做一遍。
2. 对已有活动工作走原取消／停止入口并等待原 owner 清理。MCP 使用 [stop 接口](mcp-tools.md)，Embedded 插件先 Stop 并核对清理完成。取消 ACK、软件 writer quiescent、进程退出都不是现实机构停止的证据；有未知物理结果时保留原记录，按设备现场步骤确认后再恢复输出。R03 未恢复时不做执行器试动作。
3. 请求 Server 正常关闭，保留 Embedded 供原通道收尾；Server 退出后再关闭 Embedded。使用归属明确的 PID／服务管理器，不按进程名批量停止。核对双端退出码、子进程及监听端口、数据库连接和清理错误；超时或强杀必须作为失败记录保留。
4. 在双端及其他同库写入者停止后制作一致备份：PostgreSQL 使用部署方的完整逻辑备份或一致快照，SQLite 复制停服后的完整状态目录；配置、身份和资产取同一停服点。保留备份时间及原未决效果清单。不要在活跃 SQLite 上只复制主文件、遗漏 WAL。
5. 新版产物放入独立版本目录，旧进程退出后再切换启动路径；不原地覆盖运行中的可执行文件。确认数据库目标、配置路径、工作目录和环境均指向本次部署。暂不恢复新业务入口。
6. 启动新 Server。迁移由启动流程自动执行，无需手动逐条跑 SQL；随后用下节查询核对结果。再启动配置已预检的新 Embedded，核对原持久 device ID、公钥、Server 身份和绑定。每次进程的 boot ID 变化是预期，不能拿新 boot 的事实解释旧动作已完成。
7. 在受控入口内完成下节重新授权和只读核对，再开放已验收的业务。实体设备的普通启动可能采音、播放或初始化机构，只有真实配置和现场条件已确认时才执行；无设备软件验证继续使用 noop。

### PostgreSQL 备份与隔离还原入口

以下仅在上述停服点、确认全部写入者已退出后执行。先由操作者在私有 libpq service／passfile 中登记 `alia-production` 和 `alia-restore` 两个连接别名；后者必须指向隔离还原环境。命令不包含密码，`/absolute/private/backup/CHECKPOINT` 和 `alia_restore_CHECKPOINT` 是本轮独立路径／新库名，执行前逐项替换。使用与数据库主版本兼容的 PostgreSQL 客户端，并预先准备原 owner 角色与 pgvector 扩展依赖。

```sh
umask 077
mkdir -p /absolute/private/backup/CHECKPOINT
PGSERVICE=alia-production pg_dump --format=custom \
  --file=/absolute/private/backup/CHECKPOINT/database.dump
pg_restore --list /absolute/private/backup/CHECKPOINT/database.dump \
  > /absolute/private/backup/CHECKPOINT/database.contents
PGSERVICE=alia-restore createdb --template=template0 alia_restore_CHECKPOINT
pg_restore --exit-on-error --single-transaction \
  --dbname='service=alia-restore dbname=alia_restore_CHECKPOINT' \
  /absolute/private/backup/CHECKPOINT/database.dump
```

`pg_dump` 不包含集群角色／权限定义和客户端 secret 来源；由数据库部署清单另行保留，不把凭据导入公共交付包。还原时保留 owner／ACL，若角色或扩展缺失先修复隔离环境，不用 `--no-owner`、`--no-acl` 或忽略 SQL 错误伪装成同配置恢复。源库保持停服，逐表核对行数与稳定排序内容摘要、迁移 journal／checksum、原 Unknown 和绑定记录；SQLite 在完整停服副本上执行 `PRAGMA integrity_check`，并核对配置／身份／资产／claim 文件摘要。单独成功的 `pg_restore --list` 只说明归档可枚举，不证明实际恢复。

复制私有文件时按上表逐路径保留权限与整个状态目录，包含仍存在的 SQLite WAL／SHM、R1 claim 和外部资产；先恢复到新的私有目录，在隔离网络用 noop 配置核对，不启动复制来的生产配置。原备份、失败现场和已迁移数据库都保留。只有核对成功并满足下文恢复条件，才把完整旧双端与停服点数据成套切回；数据库迁移后不能只换回旧二进制。

## 迁移核对

迁移执行器对**每一个 SQL 文件**分别提交 SQL 与 journal，持有事务 advisory 锁；不是整次升级一个事务。某文件失败会回滚该文件，但前一文件可能已经提交，提交应答丢失时也须查询数据库。因此“新 Server 启动失败”不代表数据库仍是旧版。

用受控数据库客户端连接实际部署库，先确认 database／schema，按应用的 search path 执行以下只读查询：

```sql
SELECT current_database(), current_schema();
SELECT version, checksum, applied_at
FROM schema_migrations
WHERE version IN (
  'm20261019_000001_rename_body_delivery.sql',
  'm20261019_000002_embodiment_operations.sql'
)
ORDER BY version;

SELECT to_regclass('body_deliveries') AS retired_delivery,
       to_regclass('body_delivery_late_checkpoints') AS retired_checkpoints,
       to_regclass('motion_deliveries') AS motion_delivery,
       to_regclass('motion_delivery_late_checkpoints') AS motion_checkpoints,
       to_regclass('embodiment_operations') AS operations,
       to_regclass('embodiment_operation_resources') AS operation_resources,
       to_regclass('embodiment_operation_cancellations') AS cancellations,
       to_regclass('embodiment_operation_events') AS operation_events;
```

两项 journal 应存在，checksum 对应本次二进制所嵌入的 SQL；退休表名应为 null，新表应存在。核对停服备份中的原 delivery ID、dispatch ID、命令／回执 JSON、Unknown 状态和迟到检查点均保留。审计事件仅从 `body_*` 改为 `motion_*`，envelope 中 `body_delivery_id` 改为 `motion_delivery_id`；不能要求这些外层字段字节不变。迁移不重签历史记录、不制造新派发许可、不把 Unknown 改为成功。

若只完成 rename、第二项失败，保持旧 Server 停止，保留原错和当前数据库，修复具体环境／权限问题后用同一新版重启。已提交且 checksum 相同的迁移会跳过。checksum 不匹配时停止并核对产物，不编辑旧 SQL、删除 journal 或手工改表名来迫使启动。

## 插件重新授权和开放前核对

1. 新 Server 启动后读取 `GET /api/v1/plugins/wasm`。宿主内容变化时，同一包应具有新 `implementation_pin` 且旧 grant 不会恢复启用；完全相同的宿主、包和配置重启才可恢复同 pin grant。检查实际状态，不手写 grant 文件或按包名继承批准。
2. 管理员审核当前 manifest、限额、导出和新 pin 后，以 `POST /api/v1/plugins/wasm/{name}/enable` 及 `{"expected_pin":"当前 pin"}` 授权。包更新先使用带旧 `expected_pin` 的 PUT，更新后仍未启用。撤权或更新发生持久化错误时先查询 pin／enabled，再处理原操作；HTTP 失败不证明磁盘未变化。
3. 需要私有 Appraisal 输入的包另行更新 `core.policies.appraisal.approved_interpreters`。先 GET settings，带最新 ETag 作 `If-Match`，保留其他仍批准的条目；检查 PATCH 的 `success`、`module_reports`、`errors` 和政策版本。启用工具执行不会顺带批准私有输入，旧批准也不能迁给新 pin。
4. Embedded 新包先安装、核对实际资产摘要，再以该 `expected_asset_sha256` 启动；原生设备驱动还需明确 `allow_trusted_native=true`。停止旧 instance 并等待清理后才替换；新 instance 不继承原 command 的执行许可。WASM processor 不授予 GPIO、USB 或网络能力。
5. 如部署启用了 MCP，确认各服务账号、地址／程序和凭据来源，再检查 `/api/v1/tools/mcp/services`。新连接 epoch 和目录身份使旧执行批准失效。服务 ready 仅证明连接／目录就绪；之前 unknown 的外部效果仍需查询原账本和真实接收端。
6. 验证账号和受保护接口、原绑定、Motion 查询及历史操作读取。`/health` 的 HTTP 200 不足以确认所有模块可用。旧 Body RPC 或旧签名应被拒绝；这是预期的版本边界，不能添加 fallback 绕过。新的实际动作仍须走既有确认、校准和 Execution 门槛。

## 失败恢复选择

| 观察到的事实 | 恢复动作 |
| --- | --- |
| 仅预检／配置转换失败，未启动新版 | 保留错误，修正候选副本；原版本配置及状态未被写入时可继续使用原部署 |
| 新版启动失败，数据库状态不明 | 双端保持停服；查询 journal／对象及原错误。迁移已部分提交时优先同版前向修复，不先试运行旧 Server |
| 必须回到旧版，且能证明停服备份后没有新业务或外部效果 | 先保存失败现场；把停服点的数据库、双端配置／身份／状态／资产和旧双端产物恢复为同一套，在隔离环境核对后切换。新库保留至核对完成；不要把备份直接覆盖为唯一副本 |
| 新版已接收业务、改变授权／隐私状态或可能产生外部效果 | 不直接回滚数据库到旧快照。先保留当前账本及所有增量并逐项核对；回退可能丢失去重身份、复活已撤销批准或已删除数据。优先前向修复，旧版恢复需单独的数据与授权处置方案 |
| 插件撤权／保存报告 durability unconfirmed | 当前运行器关闭不代表重启后 grant 必然已删；排除具体存储错误，按当前 pin 重试撤权并核对持久结果后再重启，不能以复制旧 grant 作为恢复手段 |
| Motion／Operation／外部工具状态 Unknown | 读取或观察原身份；需要取消时使用原目标及授权控制效果。保留未知记录和资源占用，不清空账本／占用表，不自动重派，也不凭下位机重启或“查无此 ID”认定安全 |

本仓库没有本次 schema 的自动 down migration。数据库还原不会撤销已发生的声音、运动或外部服务效果，也不会让旧设备 boot 的反馈重新有效。即使回到旧二进制，其历史 WASM grant 可能再次匹配；重新开放前仍要依据最新撤权／隐私决定复核，不能把备份中的批准当作当前批准。

## 软件证据与下位机需求

2026-10-20 的 Mac 隔离软件演练使用当前 release 产物和全 noop 配置：两轮首次绑定／正常关闭／同身份恢复共四个进程均 exit 0。第二轮停服且任务 PostgreSQL 的其他连接归零后，使用 `pg_dump -Fc` 备份，并在同一专属容器的新数据库中执行 `pg_restore --exit-on-error`。源与恢复库的 67 张 public 表、共 95 行数据逐表行数及排序 JSON 行流 SHA-256 一致，其中 `schema_migrations` 为 33 行；9 个私有配置／key／credentials／settings／SQLite（含 WAL、SHM）文件复制前后摘要一致，复制的 SQLite integrity 为 ok、binding 一致。未启动复制状态或旧二进制。

证据位于本地 ignored 目录 `run/release-20261020/lifecycle/`：`round-1.json`、`round-2.json` 和 `restoration.json` 记录已通过步骤。总入口原 `result.json`／exit 1 仍保留：唯一收尾失败是验证器未匹配 Docker 的小写 `no such object`，不是实际还原失败。随后独立只读 `cleanup-reconciliation.json` 确认容器已删除、四个 PID 已退出、三个端口连接被拒绝；没有重跑已通过步骤。本演练仅证明当前 schema 的隔离软件数据可还原，不证明真实部署、旧版回滚、断电恢复、Unknown 物理效果处理或现实设备停止。

现有实现和统一回归入口还覆盖以下契约，具体执行结果见软件发布记录：

| 检查 | 现有来源 |
| --- | --- |
| 历史 JSON／Unknown／dispatch identity 保留、迁移失败回滚及幂等恢复 | [PostgreSQL migration tests](../../packages/alia-server/tests/migrations.rs) |
| 旧表达配置拒绝 | [Server config tests](../../packages/alia-server/tests/config.rs) |
| 旧 Body／旧 Motion／反向签名域拒绝 | [Transport tests](../../crates/transport/tests/transport.rs) |
| 包升级失效、旧 Arc 撤权、grant 持久化错误 | [WASM runtime tests](../../crates/alia-plugin-runtime/src/tests.rs) |
| 执行授权与私有 Appraisal 批准分离 | [Server WASM lifecycle test](../../packages/alia-server/tests/server_runtime/wasm.rs) |
| Embedded 资产摘要、WASM 回执及 native 单独授权 | [Embedded plugin tests](../../packages/alia-embedded/src/plugin/registry/tests.rs) |
| 恢复仅观察原操作、Unknown 不释放资源 | [Embodiment operation tests](../../packages/alia-server/tests/embodiment_operations.rs) |

整理配置、构建、隔离 PG 迁移测试及全 noop 双进程演练不需要连接下位机。验证真实 Pi 的部署、原绑定恢复、音频设备或实际停止时才需要对应 Embedded 在线和现场条件；接入之前应告知操作者所需设备、连接方式及会启动的模块。该步骤不自动恢复 R03，也不扩大到 ROS 或真实模型验收，现场输入见[硬件部署资料](hardware-deployment-readiness.md)。
