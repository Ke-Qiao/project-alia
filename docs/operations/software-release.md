# 软件发布资料与真实模型链前置

## 数据库恢复修复与候选准备（2026-11-02）

本轮针对 V03 完成了有界隔离复现和最小 Rust 修复。Execution 的空账本巡检遇到数据库期限后会永久取消 worker，Delivery 的周期回收也会永久关闭 supervisor；数据库恢复本身不能重启这些已取消的实例。现将安全的扫描／过期状态收敛与真实派发分开：Execution 扫描失败暂停派发并报告 unhealthy，后续成功扫描恢复同一 worker；Delivery 回收失败暂关新 admission，保留原 owner 的监督，两个回收查询成功后恢复。Execution 原 submit 期限和容量仍有效，扫描期间可持久排队；不能将其写成所有 admission 已关闭。

claim、外部执行、receipt 持久化、真实输出控制和取消结果未知等错误仍保持原失败边界，扫描成功不会消除它们，也不会获得重派许可。Memory 则将 `rebuild_scan_error` 与投影写入 `last_error` 分开：成功空扫描只清自己的扫描错误，实际投影成功也不清尚未重新验证的扫描故障。原数据库 2 秒、Execution 500 ms 巡检、Delivery 5 秒巡检、Memory 30 秒扫尾、worker 并发和测试并发均未修改。没有增加依赖、配置开关或迁移。

Mac 验证使用 Rust 1.96.0、offline／locked、jobs 2、默认 test threads 和专属 PostgreSQL 17.10／pgvector。原实现的 Execution、output-control 反例分别验证“数据库恢复后 worker 不恢复”和“原 supervisor 已停止”；Delivery 修正测试观测后单独回退原服务实现，原用例同样因 supervisor 停止失败。保留[首次反例](../../run/software-recovery-20261102/database/mac/red.log)、[反例范围说明](../../run/software-recovery-20261102/database/mac/red-assessment.json)及[Delivery 单条反例](../../run/software-recovery-20261102/database/mac-recovery/red-delivery.log)，随后恢复修复源码并逐文件核对摘要。

首次 11 个目标的实际结果为 **157 passed / 21 failed / 4 ignored**。其中 1 条新 Delivery 测试用 SQL `prepare` 观察同一被锁账本，导致观测本身等待并错过门禁变化；改用既有同步入口的非法 nil request 检查门禁，要求精确错误、不创建 owner，保留原 10／8 秒断言预算及后续真实投递断言。另 20 个失败来自本轮测试脚本额外添加的 512 MiB tmpfs：原 SQL 明确报 `pg_wal/xlogtemp.649: No space left on device`，测试 PostgreSQL 随后 exit 1 并被 auto-remove；两次 fixture 清理也超过原 10 秒，均保留。该测试基础设施问题不属于生产 V03。见[数据库失败定位](../../run/software-recovery-20261102/database/mac/database-failure-assessment.json)和[观测修正](../../run/software-recovery-20261102/database/mac-recovery/delivery-observation-assessment.json)。

补验回到仓库既有的一次性 Docker 数据层，不加小 tmpfs 容量上限，并在清理前保留日志。仅重跑受影响的 `delivery_service`、`memory_embedding_provenance`、`output_control`，实际 **34 passed / 0 failed / 0 ignored**；已通过目标没有重复运行。两轮之间生产 Rust 源码完全相同，唯一差异是 Delivery 新测试的观测代码。下表为各目标最后一次实际结果的来源，不将两轮计数相加，也不写成一次全绿的 11 目标运行。

| Mac ARM64 目标 | passed / failed / ignored | 证据来源 |
| --- | --- | --- |
| `execution_service` | 31 / 0 / 0 | 首次定向运行 |
| `execution_repository` | 19 / 0 / 0 | 首次定向运行 |
| `delivery_service` | 14 / 0 / 0 | 观测修正后补验 |
| `delivery_repository` | 15 / 0 / 0 | 首次定向运行 |
| `delivery_text` | 6 / 0 / 0 | 首次定向运行 |
| `delivery_text_service` | 4 / 0 / 0 | 首次定向运行 |
| `output_control` | 13 / 0 / 0 | 测试库恢复后补验 |
| `duplex_playback` | 40 / 0 / 4 | 首次定向运行，保留 4 条原 R1 现场忽略 |
| `memory` | 13 / 0 / 0 | 首次定向运行 |
| `memory_embedding` | 16 / 0 / 0 | 首次定向运行 |
| `memory_embedding_provenance` | 7 / 0 / 0 | 测试库恢复后补验 |

逐目标来源、原始两轮计数与源码差异见[Mac 验证矩阵](../../run/software-recovery-20261102/database/mac-validation-summary.json)，最初 8 个变更源码摘要见[恢复修复源码身份](../../run/software-recovery-20261102/database/final-source-hashes.json)。第二个专属测试库在清理前确认其他连接 0、非默认 schema 0，保留日志后仅删除本轮容器与匿名卷，见[清理记录](../../run/software-recovery-20261102/database/mac-recovery/database-cleanup.json)。本轮没有重跑完整 workspace，历史 A／B／C 的计数保持其原范围。

Linux ARM64 使用同一恢复修复生产源码 D，实际出现新的 Resume 回归异常，按有界诊断保留三次独立运行：

| Linux ARM64 运行 | passed / failed / ignored | 实际范围与结论 |
| --- | --- | --- |
| 首次 11 个定向目标 | **177 / 1 / 4** | `duplex_playback` 为 39 / 1 / 4；其余 10 个目标通过。原 3 秒 receipt 观测超时，没有阶段／命令诊断 |
| 仅完整 `duplex_playback`，增加失败元数据 | **39 / 1 / 4** | 同一用例再次失败：Resume 应有 2 条持久回执，实际 1 条；native 仍 Held、sequence 1、剩余约 21.45 秒，wire 仅 Pause、没有媒体 Push，随后 Delivery 清理也失败 |
| 仅完整 `duplex_playback`，再补失败时 SQL 观察 | **40 / 0 / 4** | 单次通过；失败分支未执行，未取得 blocker 身份，不能解释前两次失败 |

该新异常独立记为 **V05**，失败用例为 `held_before_first_start_survives_handshake_idle_and_resumes_one_original_output`。第二次 PostgreSQL 日志有 `delivery_responses` 行锁的 statement timeout，但未记录 schema／case 归属，不能确定它是 Resume 的阻塞来源还是后续清理结果。源码审查没有找到持有 Response SQL 锁跨 `wait_open` 或 RPC 的路径；仍不能据此排除回归。两次新增诊断只涉及测试：原 3 秒观测、5 ms 轮询、默认线程、3 case 并发和 45 秒总预算不变，失败原 panic 保留；最后一次准备的独立 SQL observer 上限 2 秒，成功路径不执行它。原始计数见[首次 11 目标](../../run/software-recovery-20261102/linux/targeted-results.json)、[第二次失败与构建](../../run/software-recovery-20261102/linux-diagnostic/summary.json)和[最后一次诊断](../../run/software-recovery-20261102/linux-diagnostic-sql/summary.json)。不把三次结果相加，不把最后通过写成根因已解决，也不认定与 V01／V02 同因。

Mac 的 fmt、边界检查及自检、Server no-default／all-targets、Clippy、workspace release、Mach-O／系统动态库及三份 Embedded 配置预检通过；Clippy 保留 276 条 warning header。Linux 在第二次诊断后继续完成 PDK、Server no-default／all-targets、workspace release、ELF／loader／链接与三份配置预检，均通过。原始构建分别见[Mac](../../run/software-recovery-20261102/mac-build/checks.json)和[Linux](../../run/software-recovery-20261102/linux-diagnostic/summary.json)。Mac 构建后的第一版纯失败诊断另通过定向编译；最终 SQL 诊断版在 Linux 实际执行，未重复 Mac 测试。

最终源码记为 **D2：D 恢复修复加两份测试诊断增量**，本地代码提交为 `e2831e4408b868adb3e61ff1b90a7417703ef5c4`。相对 C 共 10 个 Rust 文件：5 个生产文件（Execution service／worker、Delivery service、Memory service／embedding worker），3 个回归测试文件，以及 `duplex_playback.rs` 和 `delivery_support/mod.rs` 两份诊断文件。所有运行与双平台构建的生产输入一致，较早测试／构建与 D2 的差异仅为明确记录的测试文件；逐文件关系见[最终源码对照](../../run/software-recovery-20261102/source-identity-final.json)。构建字节分别保留在 `run/software-recovery-20261102/mac-build/artifacts/` 和 `linux-diagnostic/artifacts/`，**当前仍是暂不放行的验证产物，没有作为通过候选打包，没有安装到生产。** 原 `8481b0f` 包及所有失败证据不覆盖，旧产物不能冒充包含新 Rust 修复。

独立的地址漂移已完成处理：Mac 从旧 `.201` 变为 `.191`，用户选择 `Ke-QiaoMac.local`；Pi 仅启用系统 mDNS resolve。Mac 实际恢复运行的仍是已验证的 `8481b0f`，配置只改 `server.host=0.0.0.0` 和 `public_endpoint=http://Ke-QiaoMac.local:3000`，HTTP／gRPC 监听所有 IPv4 接口，原认证／公钥 pin 保持，没有新增公网转发或 TLS。首次启动门禁因 LM Studio 停止而拒绝；恢复原 loopback 服务和固定 Qwen 后才启动，见[旧二进制恢复记录](../../run/software-recovery-20261102/network/existing-server-mdns-resumed.json)。同身份签名 PrepareHandoff／CompleteHandoff 各执行一次，持久 endpoint、签名读取、缺失／伪造鉴权拒绝及普通 noop 重启后的身份保持均通过，见[配对迁移](../../run/software-recovery-20261102/network/pair-migration.json)。Pi 没有更换二进制、启用音频或再次初始化 R1；恢复点与边界见[恢复记录](upgrade-recovery.md#数据库恢复修复与地址迁移准备2026-11-02)。

本轮放行缺口是 V05 Linux Resume 异常仍未定位，故新 D2 暂不发布／部署。V03 的原生产休眠／数据库暂不可达触发原因也仍未证实；当前生产继续运行 C，不声称生产已具备此次恢复修复。Easy-Turn 的独立调度验证见[模型服务记录](model-services.md)，不与数据库回归混为真人整链。下一次用户在场并确认本次 R1 电源事实后，可以在现有 C 上先做一次受控短句验证；这不要求先安装 D2，也不解除 V05 或正式发布门槛。真实效果未知结果、旧 V01／V02、完整语音链、实际双端升级／恢复、24 小时混合负载、AEC 和具体 MCP／插件验收仍独立保留，Companion 保持 shadow。

## 真人窗口与模型诊断源码增量（2026-10-31）

本轮真人窗口的两个生产VoiceSession均以 `turn model deadline` 结束，原Rust Easy-Turn **5秒／并发1**没有放宽，尚无完整判句→回复播放证据。首会话未提示测试句，第二会话也在短句提示记录之前失败；不能将未知输入直接记为已识别的指定真人短句。原错误、短句无回复和现场收口分开保留，详见[模型记录](model-services.md#真人窗口失败与-easy-turn-诊断补充2026-10-31)。Pi已恢复同一配对身份的全noop运行，PCM关闭并保留本次初始化claim。

官方known fixture实际 **4021.590ms** 返回 `complete`，原诊断因期望参数误写 `Complete` 而exit1；原输出与[单独解释](../../run/voice-acceptance-20261031/easy-turn-known-sample-assessment.json)保留，未追加请求求绿。同一fixture临时QoS检查为 **3966.222ms**，差异不能证明超时根因或修复，已回退原进程策略。两份组件结果不抵消真人窗口失败，也不关闭V01／V02或扩大Companion shadow边界。

源码增量仅为Easy-Turn `app.py`、对应服务测试与README，增加不含音频／转写／prompt／凭据的请求时序和native完成元数据；模型、线程、协议、期限、并发和依赖锁不变。原超时／取消后native持有单槽直到实际结束的语义保持。现有服务测试 **14 passed / 0 failed / 0 skipped**，默认Uvicorn日志独立检查及定向空白检查通过，见[验证记录](../../run/voice-acceptance-20261031/easy-turn-diagnostics/summary.json)。这些是fake-detector／日志配置检查，不是新增完整workspace或真实模型通过。

原生候选仍为 `8481b0faf1b92bb39905d84af13bad2cd731e70e` 对应的已验Mac／Linux字节，原生源码、配置协议和迁移没有本轮增量，不因此重建或重复软件回归。10月25日／28日源码归档和双平台产物完整保留；新Python诊断源码属于后续提交，旧归档不冒充包含该变更。

Easy-Turn诊断已通过一次受控重启实际激活，旧PID879退出、新PID3949在 **18.162秒** 就绪。旧进程graceful shutdown后Uvicorn按既有行为重发SIGTERM15，首个编排器误要求exit0的[失败记录](../../run/voice-acceptance-20261031/easy-turn-diagnostics/live-activation.json)保留；核对源码及端口关闭后才继续一次kickstart，没有改plist／配置／模型／期限。唯一冷启动后固定 **3.256秒** 官方PCM在客户端 **5026.030ms** 超出原5秒期限；新元数据确认派发 **0.749ms**、native **5960.658ms** 后迟到返回 `complete`。见[激活与真实失败](../../run/voice-acceptance-20261031/easy-turn-diagnostics/live-activation-resumed.json)：`passed=true`只指诊断记录激活，`probe_passed=false`保留样本失败，不重试或预热求绿。此次native耗时已得到定位，前两次未知现场输入及其根因仍未确定。后续生产门槛仍包括原期限内整链、业务升级恢复、24小时故障／资源负载、AEC及实际插件目标。

## Pi 生产无采播部署记录（2026-10-30）

本轮实际安装并运行了正式身份的 Pi Embedded，Mac 使用相同候选的 Server。代码仍为 `8481b0faf1b92bb39905d84af13bad2cd731e70e`；Mac Server SHA-256 `cba48222a4aa888df452571c7750911302161c2e0a0a2ff7d3cb963a8e75e8cd`，Pi Embedded SHA-256 `6608a23d38cbad5696f308c883a217d3faf22b1b10c4148f71634978b626d80a`。原10月28日归档、源码／锁文件身份和分层软件回归不变，本轮不重建或重复workspace求绿。

目标 Pi 的原生链接、两份配置预检、systemd单元校验通过；保留旧真实设备 key／SQLite，以全noop active配置完成一次正式Bind。初绑使用严格主机校验的SSH隧道，随后两端正常停止、改为实际LAN监听并同身份重启；binding记录和bound_since不变。两阶段均验证本人管理员签名设置读取成功、缺失和伪造认证被拒绝；移除隧道后核对实际socket peer与Pi→Mac双端口可达。没有启用麦克风、播放、UART、Start／Stop或新增模型推理。详见[当前部署入口](server-embedded.md#pi-正式身份无采播上线2026-10-30)和[本轮证据汇总](../../run/pi-production-20261030/summary.json)。

生产 SQL 保留原33迁移、本人账号、Qwen1024双pin和shadow；新配对恢复快照已创建，尚未演练恢复。Pi保持running／disabled／Restart=no，Mac仍为登录会话受控启动。启动前发现依赖未就绪，经手动恢复原OrbStack／PG／embedding后通过，不能把该结果记为自动登录或异常恢复。首次完整真人模型链、升级恢复、24小时负载、AEC和目标插件仍未通过；V01／V02状态不变，没有推送、合并或正式Release。

## Mac 正式启用与健康检查修复（2026-10-28）

用户已授权正式启用Mac端，首次安装复用 `6123691516fae1c2b0e813652eaa1104c33da9af` 候选的已验证Mac release；此前操作文档提交不改变该二进制或原归档。本轮实际健康检查发现GPT-SoVITS就绪探测缺陷，已完成最小Rust修复、双平台影响验证与发布构建，并用新Mac二进制同身份重启。原候选及首错保留，不把10月25日源码A/B的验证分母与部署检查相加。尚未部署Pi。

修复仅涉及现有GPT-SoVITS health请求：GET `/tts`携带当前default voice的 `text_lang`／`prompt_lang`，保持 `text=""`，不发送参考音频或prompt文本，使上游完成语言归一化后以参数不完整返回预期400，在合成前结束。原5秒期限、权重gate、ACK／preloaded状态以及500判为unhealthy保持；新增真实本地HTTP回归同时核对精确query、preloaded为degraded、ACK后为healthy和远端500仍拒绝。没有修改商家Python服务或放宽健康断言。

修复提交为 `8481b0faf1b92bb39905d84af13bad2cd731e70e`，只改TTS实现与对应HTTP回归。其原生源码C在10月25日最终B之上仅增加这两处修改，锁文件／协议／迁移未变。本轮按影响范围执行，保留原测试并发与期限；本地HTTP fixture不请求模型、音频设备或真实数据库。

| 平台 | TTS单元目标 | `provider_adapters` | 检查／构建 |
| --- | --- | --- | --- |
| macOS ARM64，源码C | **21 passed / 0 failed / 0 ignored** | **6 passed / 0 failed / 0 ignored** | Server no-default、Clippy、workspace release、Mach-O／系统动态库及3份Embedded离线配置检查通过；Clippy保留原警告 |
| Linux ARM64容器，源码C | **21 passed / 0 failed / 0 ignored** | **6 passed / 0 failed / 0 ignored** | Server no-default、release、ELF／loader／链接及3份Embedded离线配置检查通过 |

Mac证据为[分目标计数](../../run/mac-activation-20261028/mac-fix/impact-results.json)、[构建记录](../../run/mac-activation-20261028/mac-fix/build-summary.json)与[原生／配置检查](../../run/mac-activation-20261028/mac-fix/native-and-config-audit.json)；Linux的[分目标计数](../../run/mac-activation-20261028/linux-fix/impact-results.json)和[构建／清理记录](../../run/mac-activation-20261028/linux-fix/verify-impact.json)保留独立分母。Linux使用固定只读源码、network-none、无设备／模型，输入未漂移且任务容器已清理。新Mac Server SHA-256为 `cba48222a4aa888df452571c7750911302161c2e0a0a2ff7d3cb963a8e75e8cd`，Linux Server为 `9641f9352cd7e3e376c5c78385f8a4e41e24b716d462ce77a35a1db18e2684a2`；双平台Embedded和PDK guest均与原候选字节一致，见[产物比较](../../run/mac-activation-20261028/artifact-comparison.json)。这些定向检查不构成第二次完整workspace，不与旧A/B或两个平台数量相加。

Mac私有安装位于 `/Users/ke-qiao/Library/Application Support/Alia`，使用 `ke-qiao` 的 `gui/501` 用户LaunchAgent。已登记Easy-Turn／独占Juno／Server，应用LM Studio回环和最小日志／登录策略并加载固定Qwen1024 embedding；实际模型就绪各分支通过。新的 `alia-production-pgdata` 空卷与固定PostgreSQL／pgvector镜像仅发布loopback5432，扩展与最终TCP就绪已核对；原Server key保留，不从已删除旧库或备份导入账号／数据。

候选Server约5988ms提供HTTP，首位管理员 `ke-qiao@alia.local` 已注册为active admin，Companion shadow revision1；33项migration journal与源码逐项一致，Memory／Cache为所选Qwen1024双pin、policy version1，四张canonical／cache／projection表为空。首次检查脚本误假设profile总数只能为1，迁移另含3个静态seed，已保留原失败并纠正核对方式。实际health的TTS项因GET `/tts`缺少语言参数出现500；原失败保留，Server正常停止exit0、约161ms、3000／3001关闭及DB连接归零。

停服后创建私有 `backups/initial-20261028` 恢复点，dump为262143bytes、SHA-256 `e2ed246869e11dc31dc7c1abdf36a00ac2713e9e209d3a7c0a8b9d61381f6335`。在network-none／tmpfs独立PostgreSQL真实restore exit0，67张public表逐表count及排序JSON行摘要与源库完全相同，未覆盖或写入生产库，临时容器已删除。见[恢复点记录](../../run/mac-activation-20261028/checkpoint.json)与[恢复核验](../../run/mac-activation-20261028/restore-checkpoint.json)。这证明首次初始化恢复点可还原，不等于之后真实业务升级／回滚和长期备份调度已验。

新Server安装在 `releases/8481b0f/alia-server` 后完成实际重启，约5219.626ms提供HTTP；本人账号UUID／admin／active、shadow revision1、迁移、profile双pin、四张空表及原Server key全部保持，settings覆盖文件仍不存在。最终health的TTS与embedding为healthy、没有unhealthy模块；整体degraded仅来自3项有意关闭在线probe的LLM，不为求全绿开启额外付费请求。见[安装身份](../../run/mac-activation-20261028/installed-fix.json)与[实际重启记录](../../run/mac-activation-20261028/fixed-restart.json)。Mac正式服务保持运行，本轮没有采集、播放或发起模型推理。

新的[源码候选](../../run/mac-activation-20261028/alia-source-candidate-20261028.tar.gz)、[macOS ARM64候选](../../run/mac-activation-20261028/alia-macos-arm64-candidate-20261028.tar.gz)和[Linux ARM64候选](../../run/mac-activation-20261028/alia-linux-arm64-candidate-20261028.tar.gz)位于独立目录，旧候选不覆盖。代码提交、后续文档提交、源码／锁文件、双平台产物与验证对应关系由[交付清单](../../run/mac-activation-20261028/delivery.json)及[本轮汇总](../../run/mac-activation-20261028/summary.json)固定，文档提交不改变已验原生输入。实际启动停止、登录域和配置身份见[Mac正式启用入口](server-embedded.md#mac-正式启用2026-10-28)。原准备检查、原隔离演练及首次失败分别保留，不能用后续就绪结果覆盖它们。已安装不代表登录恢复、进程故障自动恢复、真实讲话／播放、业务升级恢复或24小时资源上界通过。

接下来先通知用户准备Pi，核对目标身份、owner UUID、endpoint、候选产物和持久状态，并在不启动Embedded的条件下完成配置／服务单元核对；再安排真人窗口，集中验证生产启动→讲话→模型→播放→打断／恢复→取消与下一轮，包含换气长句、Start／Stop和生产初始化后真人语音。R1已通过基线直接复用；AEC待商家确认后单独验收，首版MCP／插件按实际目标验收，V01／V02与Companion shadow边界不变。本轮没有推送、合并或正式Release。

## 本地软件候选与最终源码验证（2026-10-25）

本轮在 10 月 24 日交付的未提交主线上进行范围审查、最小修复和软件验证，R1 的既有分阶段硬件基线继续复用。
候选记录位于 `run/software-candidate-20261024/`（目录按本轮开始日期命名，候选完成日期为 10 月 25 日）。
本节分别记录完成单次全量回归的源码 A，以及随后仅修正 Delivery `never_loop` 静态检查阻断的源码 B；最终提交及归档身份由本节链接的交付清单固定。不将下面历史记录中的“当前”或旧下一步当作本轮结论。

### 本轮修复与范围核对

- WASM 撤权：grant 已不存在时仍同步包目录，不能把首次 unlink 后的同步失败或目录缺失误报为持久撤权成功；扩充原生命周期回归，原运行器仍立即失活。
- Embedded handoff：普通 Bind 仅接受 Unbound，不能抢占 HandoffPending；重启时因临时 handoff token 不可恢复，持久回到原 Server 的 Bound 身份。新增真实签名／SQLite 回归，同时验证合法移交仍可完成。
- 统一验证入口：Rust PDK fixture 固定输出到测试读取的目录，避免自定义 `CARGO_TARGET_DIR` 时误用旧 fixture。
- 提交内容：保留既有成果，排除私有配置、凭据、模型与运行数据；根 `.gitignore` 补入 `/.pnpm-store/`，避免安装缓存进入候选。
- 前端构建：显式声明锁文件已有的 `@babel/runtime=7.29.2`，恢复 Metro 默认层级解析；pnpm 声明与 mise 固定为 10.28.1，未升级依赖版本或扩展界面。
- Delivery 静态检查：将每个分支都会退出、实际上不重复的 `loop` 改为 `'observation` 标签块，并为原 `break` 指定标签；保留原取消／期限／完成判定、错误处理及清理。该等价结构修正发生在单次全量回归之后，按影响范围验证并重建宿主，不冒用源码 A 的全量结果作为源码 B 又跑了一遍。
- 部署与模型文档：补 Mac LaunchDaemon／Pi systemd、停服备份和隔离恢复、R1 初始化同流及 claim 保留、WASM 宿主重新授权；集中模型角色／embedding 维度／Easy-Turn／Juno／独立 Whisper 待输入。

审查覆盖 Rig→ToolEntry→Execution 的身份与双重准入、MCP 目录／连接身份与撤权、WASM 管理和持久 grant、fetch 逐跳地址限制、Motion 协议／迁移、Embodiment 操作恢复，以及 R1 初始化／采播退休。旧 Body、旧 provider/SSE、ToolExecutor 和旧表达子进程无可用生产入口，历史迁移与拒绝测试保留。没有增加通用框架、模型或设备功能。

### 软件验证

使用 `bash scripts/verify-software.sh release`、Rust 1.96.0、offline／locked、真实独立 PostgreSQL／pgvector。Mac 清除 ALIA 私有运行环境和测试并发覆盖，Linux 采用原 4 CPU／12 GiB 容器边界、内部 PG 网络、只读源码、无设备映射；两端均保留默认 test threads 和原测试期限。

| 平台与源码 | 单次完整 workspace | 同一统一入口的其他检查与构建 |
| --- | --- | --- |
| macOS ARM64，源码 A | **1388 passed / 0 failed / 10 既有 ignored** | 两端 no-default all-targets、默认 workspace release 和 Rust PDK 构建通过，入口 exit 0 |
| Linux ARM64，源码 A | **1413 passed / 0 failed / 12 既有 ignored** | 两端 no-default all-targets、默认 workspace release 和 Rust PDK 构建通过，入口 exit 0；不代表 Pi 部署 |

源码 A 的双平台输入清单逐项一致，见 [Mac 输入](../../run/software-candidate-20261024/mac/source-inputs-before.json)与 [Linux 输入](../../run/software-candidate-20261024/linux/source-inputs-before.json)；清单 SHA-256 为 `c22ad207604344125ef1890956b1ab4e2c035478811f49d0b1c5ac1859b389bc`。
原始 [Mac 入口结果](../../run/software-candidate-20261024/mac/verify-release.json)保留 `source_unchanged=false`：前后差异仅为 `mise.toml`、前端 `package.json` 和 `metro.config.js` 三个非 Rust 文件；Rust、Cargo.lock、协议、迁移、原生配置和 PDK 输入未变。Linux 使用冻结只读副本，记录 `source_unchanged=true`。不改写原始判定。

源码 A 的 ignored 为 3 项真实 PTY、2 项真实 Easy-Turn、1 项本地 Silero 模型和 4 项远程 R1 现场目标；Linux 另有 2 项原生 R1 采播硬件目标。它们在本轮调用前已存在，未新增忽略、放宽断言或改期限。逐项名称见各平台 `ignored-tests.txt`；本轮不启动这些模型或硬件目标。

源码 B 的原生输入相对 A 只有 `packages/alia-server/src/modules/delivery/service.rs` 标签块修正；该文件最终 SHA-256 为 `4bae66dbe021ddc1614618485e6659f622e250922bc53061d13e0523050be173`。影响验证复用真实 PG、原期限和默认并发，使用 `verify-impact.sh` 的三个既有目标、Server no-default all-targets 和默认 workspace release：

| 源码 B 检查 | macOS ARM64 | Linux ARM64 |
| --- | --- | --- |
| `delivery_service` | **12 passed / 0 failed / 0 ignored** | **12 passed / 0 failed / 0 ignored** |
| `delivery_text_service` | **4 passed / 0 failed / 0 ignored** | **4 passed / 0 failed / 0 ignored** |
| `duplex_playback` | **40 passed / 0 failed / 4 ignored** | **40 passed / 0 failed / 4 ignored** |
| Server no-default all-targets／默认 workspace release | 均通过，入口 exit 0、源码未漂移 | 均通过，入口 exit 0、源码未漂移 |

Mac B 的[影响验证记录](../../run/software-candidate-20261024/mac-final/verify-impact.json)与 Linux B 的[原始日志](../../run/software-candidate-20261024/linux-final/verify-impact.log)和[退出／构建／清理记录](../../run/software-candidate-20261024/linux-final/verify-impact.json)保留独立分母；这三个目标不是第二次完整 workspace，也不与 A 的数量相加。
最终双平台均完成发布构建、原生架构／链接检查，以及 staging、R1 自然、R1 系统三份 Embedded 离线配置预检。Mac 只有系统动态库依赖；Linux loader／ldd 通过，无缺库。真实任务 PG 连接与临时 schema 归零，自有容器／网络已清理。`cargo fmt --all -- --check`、Server 边界检查及自测均通过。
源码 B 上 `cargo +1.96.0 clippy --workspace --all-targets --offline --locked` 已 exit 0，日志有 **291 条 warning header**（包括重复诊断，不等于 291 个独立问题），没有用 allow 或放宽 lint 掩盖原 `never_loop` 错误；可选 OpenCV feature 未开启。

非 Rust 源码增量单独验证：Node 24.15.0／pnpm 10.28.1 下，frontend typecheck exit 0、工具恢复 **4 passed / 0 failed / 0 skipped**、Web export exit 0。
前端依赖／Metro 和 mise 的 pnpm pin 修正不改变 Rust、Cargo.lock、协议、迁移、原生配置或 WASM；以逐文件输入核对把两平台原生结果与最终提交对应，不因此重跑已通过的目标。Delivery 标签块另按上述 B 范围验证。

保留的原始失败／限制：

- `mac-preparation-aborted/` 在编译阶段主动停止（exit 143），测试尚未开始，用于纳入随后确认的 handoff 修复；不是一次失败的 workspace 测试。
- 前端首次 export 缺 Babel runtime，补依赖后第二次暴露禁用层级查找的问题；两份原错都保留，最终按影响范围复验通过。
- 全量回归后的首次 Clippy 为 exit 101，唯一阻断是 Delivery 的 `clippy::never_loop`；原日志与统计保留。修正后的 Clippy exit 0 不覆盖首次失败记录，源码 B 的影响测试和构建单独记录。
- 既有 node_modules 由 pnpm 11.7.0 建立，10.28.1 定向安装先遭遇 store 不匹配。11.9.0 的离线安装尝试仍请求供应链元数据且裁剪一个 optional native 包；无关锁变更已撤回，原 11.7.0 frozen/offline 安装从缓存恢复。最终锁仅增加原版本 importer；**未验证干净冷安装**。
- 部署文档本地链接、5 个 shell block 和 Mac plist 静态检查通过。既有 Linux 镜像不含 `systemd-analyze`，目标 Pi 上替换实际账户／路径后的 unit 校验仍是部署前置；没有安装或启动服务。
- 提交空白检查完整命令 exit 2：唯一报告是 `vendor/extism/src/current_plugin.rs:154` 的既有文档行末空格，整文件与 10 月 24 日原源码包相同，保留 vendor 原状；其他暂存路径的检查通过。它不是测试失败，也不记为完整空白检查通过。
- V01／V02 根因仍未确定。当前软件通过也不能解释历史失败，不扩大默认 Companion shadow 边界。

原始日志、平台分目标计数、源码／锁文件和产物摘要集中在候选目录；不把历史数量、两平台数量或增量数量相加。

### 候选产物与下一轮入口

10 月 24 日原包和证据完整保留。共享 WASM 宿主与 Embedded binding 源码发生变化，因此旧双端宿主包不能冒充最终候选；源码 A 的双平台重建及证据保留，随后在独立 `mac-final/`／`linux-final/` 为 B 影响范围验证并重建。Rust PDK guest 和 Embedded 是否复用以内容摘要核对。
最终本地提交、Git tree、源码文件清单、锁文件、平台产物、验证记录和三份候选归档之间的关系由 `delivery.json`／`source-manifest.json`／`summary.json`／`SHA256SUMS` 固定。
本轮不推送、不合并、不创建正式 Release，不切换 Mac／Pi 运行环境。

候选标识为 `software-candidate-20261025`，证据目录仍保留本轮开始日期。交付文件：
[源码候选](../../run/software-candidate-20261024/alia-source-candidate-20261025.tar.gz)、
[macOS ARM64 候选](../../run/software-candidate-20261024/alia-macos-arm64-candidate-20261025.tar.gz)、
[Linux ARM64 候选](../../run/software-candidate-20261024/alia-linux-arm64-candidate-20261025.tar.gz)、
[交付清单](../../run/software-candidate-20261024/delivery.json)、
[源码清单](../../run/software-candidate-20261024/source-manifest.json)、
[验证汇总](../../run/software-candidate-20261024/summary.json)及 [SHA256SUMS](../../run/software-candidate-20261024/SHA256SUMS)。
源码包逐文件匹配本地提交的 Git blob；各平台包包含同一提交身份、锁文件、示例配置、部署／模型／授权材料、原生链接清单与验证摘要。原始日志保留在本地 evidence 引用位置，不伪装为远端已发布证据。

[源码验证关系](../../run/software-candidate-20261024/source-validation-graph.json)明确 A 全量 → 单一 Delivery 等价结构修正 → B 影响验证／发布构建 → 最终提交。
规范化原生输入摘要 A 为 `1f798e706dab797fcfc86a0d5523e7a1b4de964e54990263c70bc70dd022ce54`，B 为 `cd9fa91010851f6ba16a9505b5c98ef1b8fdb91b73a33be94682c445165bb9ca`；
与上文包含非 Rust 项的原始清单文件摘要分别标识，不混用。
[产物溯源](../../run/software-candidate-20261024/artifact-provenance.json)逐项比较旧包、A 和 B：B 的 Embedded／PDK 字节与 A 相同，直接复用；旧 10 月 24 日双端宿主不再匹配最终输入。Mac PDK 与旧包相同可复用，Linux PDK 采用统一入口重建后的已验证字节。


2026-10-25 后续模型方案、Mac 有限组件预检和生产配置／SQL 的隔离演练已记录在[模型出口](model-services.md#生产配置与隔离-sql-出口2026-10-25)。这些后续提交只更新操作文档，软件候选和原始归档不变；该隔离阶段未切换生产。2026-10-28经用户授权进入本页首节的Mac正式启用，Pi／真人现场与新业务备份恢复仍是独立出口。
按[部署入口](server-embedded.md)和[升级恢复](upgrade-recovery.md)集中完成：生产启动 → 讲话 → 模型处理 → 回复播放 → 打断／恢复 → 取消与下一轮对话，纳入正常换气长句、系统 Start／Stop 和生产单次初始化后的真人语音。
首次配对在受控网络人工核对身份和 pin；`require_physical_confirm` 当前没有设备按钮门禁，不能把配置值当作已实现的确认。
实际部署后另验成套升级／恢复与至少 24 小时混合负载，覆盖断网、模型超时、进程退出、设备重连及资源上界。AEC 等商家确认后独立验收；首版实际 MCP 服务／第三方插件按目标验收。
ROS、执行器现场、固件重写、全面前端重构与其他平台继续按原恢复条件；当前软件结果不替代真实模型、硬件声学、常驻或长稳验收。


## Alia 软件交付与 R1 阶段收口（2026-10-24）

**R1 当前软件接入与分阶段验证已收口，现有 Alia 软件改动整理为源码包和双平台发布包。**
本轮没有新增生产功能；重新构建了尚未更新的 Server／Mac Embedded 发布产物，并补齐配置迁移说明。
用户已确定：真实模型方案明确后，集中验收“生产启动 → 讲话 → 模型处理 → 回复播放 → 打断／恢复”；
原先零散的长句／控制和初始化／真人语音组合缺口并入该轮。AEC 等商家确认后单独验证，固件继续暂缓。

| 交付范围 | 当前内容与迁移入口 |
| --- | --- |
| 正确性修复 | 采集 owner、配置持久化、身份／权限／token 并发、来源与插件生命周期、采播取消及终态回执；具体回归保留原分母 |
| Rig／工具／MCP／fetch | Rig fork 固定修订，类型化工具进入 Execution，rmcp 客户端；旧自写 provider／SSE／文本调用回退已删除，fetch 逐跳校验并固定解析地址。见[工具契约](../concepts/skills-and-tools.md)、[MCP](mcp-tools.md) |
| Extism／WASM | Rust 宿主与 PDK、资源限制、精确 pin 授权和升级／撤权；旧 Python／JSON-lines 表达入口已删除。新宿主字节要求重新核对授权，见[WASM](wasm-plugins.md) |
| Embodiment | 物理观察、Motion 和持久操作复用 Execution，Expression 保持候选表达职责；旧 Body 入口删除，双端 Motion v2 与两项 SQL 迁移需成套切换，见[升级与恢复](upgrade-recovery.md) |
| R1／语音 | 现固件 Rust 主机接入、Silero 长句、自然／系统触发、CaptureControl 和初始化防重；保留独立模式、历史现场证据及声学边界 |

交付目录为 `run/software-delivery-20261024/`：
[源码包](../../run/software-delivery-20261024/alia-source-20261024.tar.gz)、
[macOS ARM64 包](../../run/software-delivery-20261024/alia-macos-arm64-20261024.tar.gz)、
[Linux ARM64 包](../../run/software-delivery-20261024/alia-linux-arm64-20261024.tar.gz)，
以及[交付清单](../../run/software-delivery-20261024/delivery.json)和[SHA256SUMS](../../run/software-delivery-20261024/SHA256SUMS)。
源码包保留现有代码、文档、示例和锁文件，包含尚未提交的新文件及旧实现删除后的状态；排除 Git 元数据、运行目录、私有配置／密钥、缓存和模型权重。
当前 Git 基点仍为 `d2b7b6f12000535a1896dfcf7a2f4be4a2866e97`，**该 commit 不代表本次全部源码**，交付身份以文件清单为准。

| 当前独立产物 | bytes | SHA-256 |
| --- | ---: | --- |
| macOS `alia-server` | 76011792 | `242f11873a2c7d660fbac517696f08418be74f01ac47899d4b1a6f0cb5f42793` |
| macOS `alia-embedded` | 55885136 | `be89a1c53fdef40ec041053d46e31b54473116b021a1dd21a3d3d7814e8c3b68` |
| Linux `alia-server` | 86995072 | `bac1567f74d96d2f2c06052823f2b083a719b75ce3034d4d2def49d1d3c583ae` |
| Linux `alia-embedded` | 70835072 | `de0de09f3b3e9349f1ea329e3f724f6264d48159e5cc1d365b689f32e155fc0e` |

两平台使用 Rust 1.96、锁定依赖和离线默认 workspace release，编译输入前后未变。
Mac 两端 Mach-O arm64 与动态链接清单通过；Linux 两端 ELF AArch64、loader、动态链接及无缺库检查通过，
staging／R1 自然唤醒／R1 系统触发三份配置 `--check-config` 均通过，未启动 runtime。
两平台 Rust PDK guest 也已构建并分别记录内容身份；新增宿主与 PDK 使用 Rust，音频／推理既有原生库仍是运行依赖。
构建记录见 [Mac](../../run/software-delivery-20261024/mac/build-result.json) 与 [Linux](../../run/software-delivery-20261024/linux/build.json)。

本轮只补构建、链接、配置和交付文件核验，**没有重跑全量测试，也没有将新包部署至 Pi**。
现有验证继续按原范围引用：10 月 20 日完整 workspace 为 Mac 1329/0/5、Linux 1337/0/5；
10 月 24 日 Embedded 完整回归为 Mac 389/0/1、Linux 414/0/3，最终 CLI 增量各 6/0；最新 Linux duplex 为 40/0/4，自动 Pi 传输／重启 1/0。
这些分母不相加，不称为本轮新产物重新完成整链验收。此前实机 Embedded 是 SHA `2601735…` 的单包 release；
本次默认 workspace 构建的 Linux Embedded 为另一内容身份，旧现场结果不改写为新字节的实测。
V01／V02 历史未定位异常继续保留；未启动外部模型、MCP 服务或设备，未改固件、音量或常驻部署。


## R1 系统控制传输与普通重启现场验收（2026-10-24）

**系统 Start／Stop 的真实 Pi→Server 传输、会话清理与同身份普通重启已通过。**
本轮复用已冻结的 Embedded release，生产源码没有改动；新增真实 Server 所有权回归
`speech::system_capture::signed_system_start_stop_and_disconnect_use_real_server_ownership`，覆盖精确 PCM 进入 Server 检测器、
Stop 等待原生输出退出、断线清理及显式重新 Start。该回归的 PCM／VAD／模型结果／回复音频／native 回调为 fixture，
与下方真实 Pi 的传输现场证据分开统计。
Linux ARM64 完整 duplex 为 **40 passed / 0 failed / 4 ignored**（45.79 秒），
最终显式运行 `r1_live::remote_r1_system_control_transport` 为 **1/0/0，29.00 秒**。
[最终现场日志](../../run/r1-system-auto-20261024/server/live-auto.log)和[退出记录](../../run/r1-system-auto-20261024/server/live-auto-exit.json)保留完整阶段。

| 最终现场阶段 | 活跃观测时间 | relay 转发样本（阶段 → Stop 收束） | 结果 |
| --- | --- | --- | --- |
| 第一次 Start／Stop | 12.023 秒 | 193280 → 193600 | 正常签名会话；Stop 后任务／forwarder 均为 0 |
| 同进程第二次 Start／Stop | 1.026 秒 | 17600 → 17600 | 旧 generation 拒绝；idle 不新增会话或上传 |
| 普通重启后第三次 Start／Stop | 1.016 秒 | 17280 → 17600 | 身份不变、boot 更新；旧 boot 的 Start／Stop 拒绝 |

三轮均为真实 **16 kHz／mono** PCM，峰值 **1／0／0**、speech 样本均为 0；本次只验证传输和控制，
不证明真人语音、声学质量或 Server 语义处理。未鉴权 Inspect／Start／Stop 均被拒绝，
同配置／状态的普通无参重启在原 60 秒 gate 内以 **3.417 秒**完成，没有调用初始化入口。
最后 delivery shutdown 成功、pending=false、runtime_tasks=0、failed=[]。

产物身份分别见 [Embedded 复用记录](../../run/r1-system-auto-20261024/build/embedded-reused.json)和[Server 构建记录](../../run/r1-system-auto-20261024/build/server.json)：
Embedded release SHA-256 为 `2601735ac0aa638dd4dfb5e6c72322bf0a4d1fd66ceae09066560606e5aed61c`；
Server 测试产物为 `2774228abc06aacaaedfa009eb21982a9538eb3cc750ae2ce02f2d0f0005d265`。
首轮 `server/live.log` 为 **0/1，79.77 秒**，因编排错过原 60 秒重启 gate；
下一次 `retry-first` 在准备时未正确解析带 ANSI 颜色的公钥日志，尚未启动 Server。
修正编排后最终通过，未修改 Rust 时限或重新构建；四个 Pi 进程不代表四次 live 测试。

[运行审计](../../run/r1-system-auto-20261024/pi/runtime-audit.json)确认 `first`、`retry-first`、`auto-first`、`auto-restart`
均为相同 binary／config、argv 仅 binary，clean exit 0，无强杀或 watchdog 超时；
原生 PCM 两端打开后正常退出，`openat/openat2` 对 `/dev/serial`、`/dev/ttyUSB`、`/dev/ttyACM` 的打开数均为 0。
无初始化 claim 或保存音频。最终自有两容器／网络／隧道已清理，删除前所有非默认数据库 schema 为空、其他连接为 0；
Pi UART／PCM 无占用、ALSA closed，Mac／Pi 的 45410／45411／45412 均关闭，原有 `astrbot` 未动，mixer 保持 −14 dB。
证据见[清理记录](../../run/r1-system-auto-20261024/server/cleanup.json)、[Pi 终态](../../run/r1-system-auto-20261024/pi/postflight.json)及[本轮汇总](../../run/r1-system-auto-20261024/summary.json)。

**生产单次初始化与真人语音的同轮组合证据仍未取得，已并入模型方案确定后的完整语音验收。** 用户已确认 R1 当前阶段收口；cold-03 的初始化、固定 beam 0、五分钟静默恢复及听感证据继续复用，不再单独安排重复插拔、讲话或试听。固件继续暂缓，AEC 等商家确认后单独验证；阶段收口不等于真实模型／声学整体验收通过。

## R1 系统控制发布准备与既有证据复用（2026-10-24）

以下保留当时仅准备与部署的状态；随后完成的传输／普通重启现场结果见[本页最新记录](#r1-系统控制传输与普通重启现场验收2026-10-24)。

用户明确不重复已在独立任务完成的插拔、beam 0 讲话和原幅度试听。已核对 cold-03 的协议／PCM 审计及用户确认，
沿用一次冷启动、单次初始化、395.56 秒同流 PCM 和 300.02 秒静默恢复的有限硬件结论；不再把该硬件基线作为重复现场任务。

生产 Embedded 的新 **Linux ARM64 release** 已构建、冻结并放入 Pi 的独立验证目录，未启动 runtime。
产物 SHA256 为 `2601735ac0aa638dd4dfb5e6c72322bf0a4d1fd66ceae09066560606e5aed61c`，
Pi 上 `--check-config` 与 `ldd` 通过，模型仅作为文件部署。另新增默认 ignored 的系统控制现场 fixture，
复用真实 Server／PostgreSQL 与签名接口，预留 Start／Stop、静默后 Start 和普通进程重启的阶段门控。
现场用例未执行，不能据此记为新版 Pi→Server 整链通过。

冻结 Server 测试产物的完整 duplex 回归 **39 passed / 0 failed / 3 ignored**；
新增检查覆盖 idle 期间额外会话／PCM，以及仅显式 Stop 阶段接受预期 `Cancelled` 传输终止。
新 fixture 同时避免第二次 Start 误读旧 Ready。Rustfmt 和定向 diff 检查通过。
首轮 fixture 编译因子模块路径缺少显式定位失败，日志单独保留；修正后重新构建并冻结，最终源码前后哈希一致。

本轮仅完成构建、只读设备预检和隔离文件部署，没有新的冷启动、UART 打开、初始化命令或音频采集／播放。
临时 Server／PostgreSQL 容器、网络及 SSH 隧道已清理，删除前数据库其他连接为 0，Mac／Pi 验证端口关闭；
Pi UART／PCM 无占用，mixer 保持 −14 dB。隔离产物保留，未安装常驻服务。
删除前的 schema 查询误用了与 fixture 不匹配的前缀，不能作为 schema 清空证据；临时数据库已随容器删除。
证据见 [`summary.json`](../../run/r1-system-live-20261024/summary.json)。
该准备阶段尚未取得新版生产启动／RPC 的现场组合证据；后续增量验收见上方最新记录，不再默认要求用户重复已有硬件试验。

## R1 系统控制采集软件接入（2026-10-24）

在独立 cold-03 实测基础上，按用户授权实现纯 Rust 主机接入。新增显式启动入口
`alia-embedded --initialize-r1 <config> <cold-boot-id>`：确认实际原生 16 kHz mono PCM 后，保持同一采集流，
严格核对当前固件，单次发送定向 beam 0 `manual_wakeup`。已持久 claim 的 ID 永不重试；普通启动、UART 重连及 capture reset 不发送手动命令。
claim 在 UART 打开前以 create_new 和文件／目录同步落盘，取消、断连、部分写入或 ACK 超时均保留 unknown；成功 ACK 只证明命令受理。
主机重启不自动产生新的冷启 ID，状态目录和 claim 必须随设备状态保留。

独立 `speech.trigger=system` 模式不消费自然唤醒事件来建会话。
新增 `alia.capture.v1.CaptureControl` 的 Inspect／Start／Stop，复用 `embedded:control` 绑定鉴权、原绑定身份、现有 VoiceSession 和原生退休机制。
设备 boot＋本地控制代次拒绝陈旧命令；已取消排队命令不执行；Start 丢弃已有预卷／发布队列但不声称裁切正在途中的原生采样块。
Stop 回执等待本地会话任务退出与原生提交收敛，失败闭锁；不证明远端已处理完所有样本、物理无声或麦克风断电。
保留自然唤醒配置，系统配置独立于已验历史路径；没有自动迁移现场配置、部署、启动麦克风或调用真实模型。

本轮软件验证已完成，汇总和源码哈希见 [`summary.json`](../../run/r1-system-integration-20261024/summary.json)。
完整 Embedded 默认 feature 回归 **macOS 389 passed / 0 failed / 1 ignored，Linux ARM64 414 / 0 / 3**，按平台分别报告。
最终 R1 诊断措辞修正后，macOS 配置／启动目标另验 **39/0**；最后修正显式 CLI 不被旧 KWS 环境诊断截走，两平台 CLI 增量均 **6/0**。
这两项增量与完整结果不重复相加。Linux 独立 `r1_manual_capture` no-default example **24/0**，
Server no-default 测试目标编译、Embedded 默认 dev 构建与 `--help`、最终 no-default binary 编译均通过。
本轮未执行 Server／PostgreSQL 整套测试，也未生成或部署新的 release 包。
Rustfmt、Server 所有权边界、diff 空白、相关文档链接及 4 个不启动 runtime 的非法 CLI 输入检查通过。

六项新增系统控制集成回归覆盖真实 tonic 回环、有效签名与错误 scope／签名、排队取消／绑定替换、旧 boot／generation、
原生写入等待和 Stop／断线后的清理超时闭锁；13 项 Linux 初始化回归含严格版本／ACK、持久防重、PTY 分包、命令发出后的取消与断连。
原生格式准入和初始化配置门槛另有回归。Linux 容器无网络、源码只读、无设备映射；完成与最终 CLI 增量的源码前后哈希一致，容器均已自动删除。

失败不覆盖：首轮 macOS sandbox 为 **249/46**，46 项均出现回环监听 EPERM；允许回环的环境完整通过。
首轮 Linux 在新测试 fixture 将 Probe 错用为 VAD 时编译失败；修正 fixture 后，下一轮仅新 runtime 断言失败，
原因是 R1Config 的旧错误文字只描述 wakeword 模式。已改为通用 R1 控制诊断，最终完整通过。
原日志分别为 `mac-embedded.log`、`linux-embedded.log`、`linux-final.log`，通过日志为 `mac-embedded-final.log`、`linux-complete.log` 与各 final/CLI 增量文件。

独立硬件证据见[调研报告](../architecture/r1-system-controlled-capture-research-2026-10-23.md#71-2026-10-24独立冷启动-cold-03-实测)：
cold-03 395.56 秒同流保存 PCM、300.02 秒静默后完整清楚语音、397 窗口／6 阶段复算一致；1 块20毫秒仅辅助 reference 副本缺失。
软件接入阶段尚未现场验收新生产启动／RPC 路径；随后系统传输与普通重启的现场结果见[本页最新记录](#r1-系统控制传输与普通重启现场验收2026-10-24)。生产初始化与真人语音同轮证据仍不能由 cold-03 替代；真实模型、固件改造、AEC、其他方向和 24 小时稳定性仍不在通过范围。

## R1 自然长句现场复验（2026-10-23）

**当前状态：真实长句输入与正常端点、修复后的采播控制与重新唤醒已分别通过，−14 dB 音量获用户确认。**
长句证据来自本轮 attempt-4，最终控制证据来自同轮次系列的 attempt-6 短句入口；
不宣称最终新产物在一条自然长句完整用例中全部通过，单轮组合证据可后续补充，不为此继续重复现场采集。
本轮长句 attempt-4 收到 **399680 个 active 样本／16 kHz = 24.98 秒**，最长连续 active 同为 **24.98 秒**，
模型窗口 **25.68 秒**，首次 ALSA 提交 **4032**，输出 backend 为 48 kHz／双声道。
用户确认此前调整后的 **−14 dB 音量合适**。随后在暂停短句之前出现 native underrun，
再发生控制回执缺失及 Server 终态 observation 错误；不能将输入阶段通过写成整套现场用例通过。
软件排查发现 fixture 每 100 ms 仅供给 100 ms PCM、初始余量仅 100 ms，且 `Skip` 会漏掉延迟 tick。
现场未记录 tick 延迟，因此不把 native underrun／EPIPE 唯一归因于 fixture。
现已改为 400 ms 首块（仅前 200 ms 为提示音）和有界 Burst 补供，超过 300 ms 停顿时重置以免暂停期间积压；
原 ALSA EPIPE 仍作为失败。Server 终态 observer await 竞态已独立审查、修复并通过确定性红绿回归。
第 5 轮修复后短复验完成控制，最后新唤醒超时；用户确认没赶上提示、没有说最后一句，两次提示音均听到且音量合适。
第 6 轮短入口完整 **1/0、37.96 秒**，包含新会话上传，未见 native underrun／ERROR。
六轮 Pi 均已 clean exit 0、无强杀／watchdog 超时、UART owner 为空、两路 ALSA 状态及 hw_params 均 closed；
两个隧道均正常 exit 0；旧隧道在第 4 轮结束后达到预设 1200 秒生存上限，不属于该轮失败原因。
本轮硬件验证结束，最终清理已核对：PostgreSQL 全部非默认 schema 为空、其他连接为 0；
仅移除本轮 `alia-r1-long-live-20261023-linux`、`alia-r1-long-live-20261023-pg` 及同名任务网络，
Mac／Pi 的 45311、45312 验证端口均关闭，Pi 采播 closed，共享 astrbot 保持运行、未改动。
最终 mixer 左右 PCM 为 **23/37、62%、−14 dB、on**，未用 `alsactl` 持久化。
证据见 `run/r1-long-live-20261023/cleanup.json`、`pi-final-state.json` 与 `summary.json`。

本轮仍使用冻结的 Embedded 产物、新私有状态、源音峰值 **24576／200 ms**、应用增益 **1.0**、
C-Media mixer **−14 dB**，通过 Mac 的 Linux ARM64 Server 容器与正常签名协议接真实 Pi。
音频只在内存处理，轮次标签和回复仍为 fixture，未发送 beam／手动唤醒；真实模型、固件和 AEC 的暂缓决定保持。

旧现场入口要求最长连续 active 至少 20 秒，实际换气会将连续统计分段。用户说明已经努力连续说 22 秒但需要换气后，
现场用例改为自然长句：模型窗口至少 **20 秒**且累计 speech active 至少 **320000 样本（20 秒）**，
继续保留最长连续统计和原 **30 秒**模型窗口上限，正常端点仍由生产路径形成。
入口由 `remote_r1_long_speech_real_server_duplex_and_rewake` 改名为
`remote_r1_natural_long_speech_real_server_duplex_and_rewake`；此前 strict 失败与既有公开语音连续压力证据保留，
不把新判据通过回写为原 strict 用例通过。新增换气／时长判据回归与统计／relay 定向验证为
**3 passed / 0 failed / 2 ignored，编译 27.84 秒、执行 0.07 秒**。
长句 attempt-4 的 Server 测试产物 SHA-256 为 `1c4fd5c8ab225f00a1921a368d524077ea8551869e00e7797ec3d4d61a15a65b`。

| 本轮长句尝试 | Pi 运行时间（北京时间）／首次自然方向 | 实际结果 |
| --- | --- | --- |
| attempt-1 | 03:49:48—03:51:38／330° | 旧 strict 入口最长连续 active **17.64 秒**，未满足 20 秒，保留断言失败。 |
| attempt-2 | 03:52:46—03:55:36／336° | 用户确认只说了唤醒词或短句，没有尝试连续 22 秒；最长连续 **5.70 秒**，旧 strict 断言失败。 |
| attempt-3 | 03:56:12—03:58:27／291° | 用户说明尝试连续 22 秒但需要换气；模型窗口 **24.72 秒**，累计 active **19.90 秒**、最长连续 **5.32 秒**，旧 strict 断言失败。 |
| attempt-4 | 04:03:42—04:06:25／4° | 新 natural-long 入口正常取得 **25.68 秒窗口／24.98 秒累计和连续 active**，数值也满足旧连续指标；随后控制失败，整例 **0 passed / 1 failed，89.47 秒**。 |
| attempt-5 | 04:17:01—04:20:07／332°→343°→349° | 修复后独立短句复验完成首句、暂停／恢复和 `Cancelled`；仅收到 3 次自然唤醒，最后新会话阶段 60 秒无新唤醒而超时，整例 **0/1，128.36 秒**。没有 native underrun／ERROR，清理无错。 |
| attempt-6 | 04:21:14—04:22:37／283°→312°→275°→313° | 最终短入口完成首句、暂停／恢复、取消、旧会话清理及新唤醒 session 2 上传，**1 passed / 0 failed，37.96 秒**；无 native underrun／ERROR，清理通过。 |

长句 attempt-4 的 native underrun 日志在北京时间 **04:05:15.968**，
早于暂停阶段的 **04:05:29.300、345°** 自然唤醒。暂停短句端点为 **3540 ms**，
随后等待控制回执超时；清理得到 `receipt cannot change a terminal delivery`，
`delivery_shutdown=error, shutdown_pending=false, runtime_tasks=0, failed=["delivery shutdown"]`。
第 1—3／5／6 轮 Server 清理为 `delivery_shutdown=ok, shutdown_pending=false, runtime_tasks=0, failed=[]`；
第 1—5 轮失败与第 6 轮通过分别保留。原始证据在 `run/r1-long-live-20261023/attempts/attempt-1/` 至 `attempt-6/`，
第 4 轮阶段结论与用户音量反馈另存 `attempt-4/result-summary.json`。

终态 observer 确定性红测在旧实现 **0/1**：已校验的 sequence 4 终态在 await 被取消后丢失，退回 sequence 3；
修复后完整 playback **13/0，测试 0.57 秒、编译与执行总计 42.873 秒**，保留仓库终态校验。
fixture 旧 Skip 红测 **0/1**，明确漏掉 300 ms 音频槽；修复新增长暂停、音调长度和取消覆盖。
最终完整 duplex **37/0/2，执行 45.69 秒、总计 70.766 秒**；原始红绿及完整日志在本轮 `server/`。
第 5／6 轮 Server 测试产物 SHA-256 为 `bd2b566bb399944fbab808a0d2a81ea10da8d49996aa6084e97e6c25309a8ded`。

修复后第 5 轮首句窗口 **3440 ms**、首次提交 **3360**；暂停窗口 **3140 ms**，
`Applied / Held` 与 `Applied / Running`，恢复提交 **367229→370973**。
取消窗口 **2880 ms**、结果 `Cancelled`，offered／accepted **753600**、submitted **745949**、
receipt sequence **3153**，`end_accepted=false, presented_frames=null`。
旧会话清理后等待重新唤醒超时，Pi 只记录 332°、343°、349° 三次自然事件，没有第四次，
该轮缺失的新会话步骤仍记为未完成。用户随后确认没赶上最后提示、未说最后一次唤醒，
且初次／恢复后提示音都听到、−14 dB 音量合适，见 `attempt-5/user-observation.json`。

最终第 6 轮首句窗口 **3140 ms**、首次提交 **3360**；暂停窗口 **3140 ms**，
`Applied / Held` 与 `Applied / Running`，恢复提交 **36768→40320**。
取消窗口 **3100 ms**、结果 `Cancelled`，offered／accepted **84800**、submitted **76608**、
receipt sequence **350**，`end_accepted=false, presented_frames=null`。
旧会话清理后成功建立 session 2，实际上传 **44800 样本**、active **33280 样本**；
四次方向为 **283°→312°→275°→313°**，beam=5 是板端自然上报，主机未发送 beam 命令。
六轮输入及最终控制的通过范围分别记录；完整自然长句与全部控制在最终产物中的同轮证据尚无，
不影响本次长句输入和合适音量目标收口，也不代表真实模型、AEC 或免固定口令方案完成。

当轮结束时独立任务只完成 [R1 离线调研](../architecture/r1-system-controlled-capture-research-2026-10-23.md)。
其后 cold-03 实测与用户授权的生产接入见 2026-10-24 顶部记录；不改写本节自然唤醒现场约束与结果。

## R1 新版现场采播验收（2026-10-23）

本节保留此前七轮及当轮结束状态；后续自然长句和 −14 dB 音量复核见顶部。

**状态：第 7 轮短句现场采播、暂停／恢复、取消和重新唤醒已通过，采用每阶段自然唤醒适配现固件。**
用户确认听到了提示音但仍偏小；按用户要求随后将声卡 mixer 再提高约 2 倍，未追加播放。
严格连续至少 20 秒且正常端点的长句现场完整场景仍未通过，R02 整体保持未完成。
第 1—6 轮失败均保留。Server 取消顺序缺陷已确定性复现、修复并通过第 7 轮实际取消验证；
原第 5 轮缺少完整取消 outcome，仍不认定该缺陷是当时所有失败的唯一原因。
保持自然唤醒方向、不发送 beam／手动唤醒；真实模型和固件继续暂缓，AEC 等商家确认。

七轮均复用 10 月 22 日冻结的 Embedded release（SHA-256 `36eee448958b215b7f4113eba111132113a0f1884383548e060e805cf080a7e2`），
该现场产物的生产音频源码未改变。每轮使用与既有部署隔离的新私有目录、SQLite 和身份；
第 1—6 轮正常签名 GetVolume 返回 `0.05000000074505806`（f32 的 0.05），第 7 轮为 `1.0`。
固定 R1 serial `2c000c70f5c2c761891`、CH340 稳定控制路径和独立 C-Media 均已核对。
实际 ALSA 输入为 16 kHz／S16_LE／mono，输出为 48 kHz／S16_LE／stereo；
新 Silero 已加载，R1 仍报告现有 `circle6:1.9.6.f8ef651`。

显式 ignored 的 Rust 长句入口 `r1_live::remote_r1_long_speech_real_server_duplex_and_rewake`
接真实远端 EmbeddedChannel、正常设备认证、生产 VoiceSession、Delivery 和 PostgreSQL。
透明 gRPC relay 原样转发 metadata／PCM，仅保留样本数、连续 active 长度和峰值等标量；
不留用户音频或识别文本。轮次标签和生成音频为 fixture：第 1—5 轮峰值不超过 2048、200 ms、
10 ms 包络的低幅合成音加静音。控制与提交证据要求 ALSA 48 kHz／双声道且无 Presented 声称，
不能替代暂停零参考、DAC 呈现、声学尾音或 AEC 验收。

原 Server 测试产物的 Linux ARM64 统计／relay 回归 **2 passed / 0 failed / 1 ignored**；
一次完整 duplex 默认并发 **34 / 0 / 1，45.49 秒**，唯一 ignored 是长句现场入口。
该产物 SHA-256 为 `1a2e3e2ce9855101225161ac20fa9fc43d04251f682c69edd62fb6f0214533ea`，用于第 1—4 轮并单独保留。
第 5 轮新增独立 ignored 入口 `r1_live::remote_r1_short_speech_real_server_duplex_and_rewake`，
只验短句采播与会话控制，不满足或替代长句判据；原长句入口及生产期限保持。
新增入口后的 Linux 编译 **27.59 秒**，统计／relay 定向回归 **2 / 0 / 2，0.06 秒**；
该产物 SHA-256 为 `35b5258b20801af0e3f0545d8cb8772855c42ef23c8111adb16c8457a0547253`，
不把此前完整 duplex 结果当作这一新产物的完整目标回归。
Mac 两次构建在宏动态库装载期间持续等待，没有输出编译错误；只终止本轮自有进程并保留日志，
没有修改系统安全设置。现场 Server 运行在同一 Mac 的 Linux ARM64 容器，
通过回环 SSH 双向转发访问 Pi；不能记作本轮原生 macOS Server 现场通过。

| 轮次 | Pi 运行时间（北京时间）／自然方向 | 实际结果 |
| --- | --- | --- |
| attempt-1 | 02:45:54—02:48:11／无唤醒 | 正常签名绑定完成；Server ready 后 60 秒未收到自然唤醒，未建立 VoiceSession、上传用户 PCM 或开始合成提示音。仅记录该窗口无唤醒，不认定模块故障。 |
| attempt-2 | 02:59:30—03:00:42／9° | 自然唤醒及会话成功；24.04 秒模型窗口输出时累计 active 21.96 秒，最长连续 active 12.32 秒，未达到连续 20 秒判据，长句断言失败。 |
| attempt-3 | 03:02:20—03:03:39／10° | 连续 active 的 15 秒、20 秒 marker 均出现；03:03:22 收到明确的 `utterance exceeds the bounded 30 second model window`，会话关闭，未完成长句场景。 |
| attempt-4 | 03:05:11—03:06:38／22° | 22.68 秒模型窗口、累计 active 21.64 秒，最长连续 active 10.94 秒，仍未达到连续 20 秒判据，保留断言失败。 |
| attempt-5 | 03:08:55—03:10:59／21° | 独立短句入口取得 4.22 秒真实输入窗口、ALSA 提交及暂停／恢复证据；取消结果为 `Unknown`，期望 `Cancelled` 的断言失败，随后 `delivery shutdown` 失败，未进入断连后重新唤醒阶段。 |
| attempt-6 | 03:25:11—03:27:02／336° | 修复后的 Server 取得 4.16 秒首句窗口、55360 个 active 样本及首次 3552 帧 ALSA 提交；后续 60 秒未检测到短句端点，测试超时，未执行用户取消或重唤醒。用户确认已在原位置说过“嗯／好的”，且声音太小；门控原因尚未证实。 |
| attempt-7 | 03:32:27—03:34:33／337°→333°→340°→355° | 独立短句入口全部通过：初次唤醒后首句 3 秒；同一会话内再次自然唤醒后完成暂停／恢复及取消；断连清理后最后一次自然唤醒建立 session 2 并取得新 PCM。测试 **1 passed / 0 failed，98.07 秒**。 |

第 5 轮首次 `AlsaWriteAccepted` 为 **3264 帧、48 kHz、双声道**；
暂停／恢复分别收到 `Applied / Held`、`Applied / Running`，恢复后提交计数从 **325152 增至 329280**。
这些是已经取得的阶段证据，不能将最终 `Unknown` 记为成功取消；取消与清理失败暂不归因于硬件或既有 V01。
用户报告第 5 轮没有听到提示音，首次提交不能替代扬声器可听验证。事后仅只读检查 C-Media mixer：
左右 PCM 均为 **17/37、46%、−20 dB、on**。它与应用增益 0.05、源音峰值 2048 叠加后音量很低，
可能解释听不到提示音，但尚不能认定为唯一原因；该次检查没有调整 mixer 或追加播放，后续第 6 轮参数另记。
原始输出在 `run/r1-live-20261023/speaker-mixer-readonly.json`。
第 1—6 轮 Server 测试均 exit 101，第 7 轮 exit 0；第 1—4 轮清理 `runtime_tasks=0, failed=[]`，
第 5 轮虽然 `runtime_tasks=0`，仍明确保留 `failed=["delivery shutdown"]`，不记为全部正常清理。
第 6 轮 `delivery_shutdown` 明确为 `outcome=ok, error=null, shutdown_pending=false`，
随后 `runtime_tasks=0, failed=[]`；该轮没有执行用户取消，后续第 7 轮取消结果另记。
第 7 轮同样 `delivery_shutdown=ok, shutdown_pending=false, runtime_tasks=0, failed=[]`。
七轮 Pi 均 exit 0、`clean_exit=true`、无强杀／watchdog 超时、进程已退出，
UART owner 为空且两路 ALSA 的状态和 hw_params 均 closed。未保存用户录音。
第 5 轮后的 SSH 隧道曾正常停止、exit 0；当时 PostgreSQL 检查的测试 schema 与其他连接均为 0。
这些后续资源检查不抹去第 5 轮的 delivery 清理失败。

七轮原始结果分别在 `run/r1-live-20261023/attempts/attempt-1/` 至 `attempt-7/`；
编译、回归和产物清单在 `run/r1-live-20261023/server/`，原 Server 测试产物另存为
`duplex_playback-before-short-entry`，修复前短句产物另存为 `duplex_playback-before-cancel-fix`。
短句控制轮次已通过；剩余严格连续至少 20 秒并正常形成端点的长句完整现场场景，后续音量调整的听感尚未复核。
Server 原 10 秒无帧 idle、3 秒 acoustic 准入及 30 秒待判定窗口保持，不以延长生产期限通过验收。

后续软件定位已确定取消顺序缺口：旧 playback 在取消或 producer 出错时先释放上传 sender 与回执流，再发 Fence，
可能使设备先固定为 `Unknown`。现由外层保留 sender 和 receipt stream，直到精确 Fence 返回；
及时停止 producer 与 observer 的数据库 future，正常 End 仍按 EOF 收尾，未改 Embedded 或生产期限。
测试最初两次因生命周期借用产生 E0505，保留 `cancel-streaming-red.log`、`cancel-streaming-red-2.log`；
修正测试生命周期后，`cancel-streaming-red-3.log` 的两项确定性回归在旧实现均失败（0/2），
修复后的完整 playback 目标 **12 passed / 0 failed，编译 42.55 秒、执行 0.57 秒**。
现场入口同时补齐取消的完整标量／outcome 诊断，并分开记录 shutdown error 与 pending；
最终同格式 PCM 加固的 streaming 三项 **3/0，编译 36.31 秒、执行 0.52 秒**，
完整 duplex **34/0/2，执行 45.78 秒**；两个 ignored 为独立长句／短句现场入口。
第 6 轮 Server 测试产物 SHA-256 为 `c11da245cf6a4be1ad5879c6002a4608a865f88594c88ab9d2d9328fefb624bd`。
前五轮源音峰值 2048、200 ms、增益 0.05 的历史结果保持；第 6 轮源音峰值为 **16384**，
时长仍为 **200 ms**、实际增益仍为 **0.05**，首次提交为 3552 帧、48 kHz、双声道，未声称 Presented。
第 6 轮未走到用户取消与重唤醒，用户确认后续“嗯／好的”已在原位置说过且声音太小，该失败保持。
第 7 轮按用户授权将源音峰值 **16384→24576（1.5 倍）**、应用增益 **0.05→1.0（20 倍）**，
mixer 仍为 **−20 dB**，单次仍 **200 ms**；这是数字幅度 30 倍，不保证声学响度 30 倍。
同时加入每 5 秒 PCM peak／active 标量统计，统计／relay 定向回归 **2/0/2，编译 25.32 秒、执行 0.06 秒**。
其 Server 测试产物 SHA-256 为 `980c338d3d72cd01c6965374cfb29deb6264aacc24813eb235dcf69ceebcc7b0`。
这一轮只改现场诊断／fixture，生产代码与此前完整 duplex **34/0/2** 一致；不将两次产物身份混写。

第 7 轮首句窗口 **3000 ms**、首次提交计数 **3456**；暂停短句窗口 **3120 ms**，
收到 `Applied / Held`、`Applied / Running`，恢复后提交计数 **279744→283584**。
取消短句窗口 **3160 ms**，完整 outcome 为 **cancelled / local software playback fence applied**；
源帧 offered／accepted 均 **588800**、submitted **581856**、receipt sequence **2596**，
`end_accepted=false, presented_frames=null`，backend 为 `AlsaWriteAccepted / 48 kHz / 2 channels`。
这验证修复后的活跃流取消，未把提交或软件 fence 当作物理尾音证据。
旧会话清理后，**355°** 自然唤醒建立 session 2，实际上传 **49600 样本**、active **37120 样本**。
同一 session 内的暂停和取消短句分别在 **333°／340°** 自然唤醒后进入；
输入恢复支持现固件唤醒门控的解释，但没有唯一证明其原因，验收范围是重复自然唤醒下的短句交互。
无需重复唤醒的连续对话不记为通过。主机未发送 beam，所有方向均来自板端自然事件。

用户确认第 7 轮提示音已听到但仍偏小，并要求再增大一倍；设备释放后将 C-Media 左右 PCM
从 **17/37、46%、−20 dB、on** 调到 **23/37、62%、−14 dB、on**，幅度增益为 **+6 dB ≈ 1.995 倍**。
这是第 7 轮通过之后的当前 mixer 状态；该轮实际验收仍使用 −20 dB。
没有追加播放，也没有用 `alsactl` 持久化系统 mixer 状态；新音量听感待后续使用确认。
操作前采播均 closed，证据见 `run/r1-live-20261023/speaker-mixer-adjusted.json`。

最终资源核对：PostgreSQL 全部非默认 schema（含 `delivery_test_*`）为空，其他连接为 0；
两个自有容器及任务网络已删除，自有 SSH 隧道 exit 0，Mac／Pi 的 45311、45312 端口均关闭，
Pi 采播均 closed；共享 astrbot 保持运行、未改动。详情见
`run/r1-live-20261023/cleanup.json`、`pi-final-state.json`、`tunnel-exit.json`，
七轮范围和用户听感汇总在 `summary.json`。最终清理不抹去第 5 轮的历史 delivery shutdown 失败。

## R1 长句与真实 Server 采播软件闭环（2026-10-22）

**长句 VAD 重构、真实 Server 软件闭环及两平台 Embedded 发布构建已完成；本轮没有连接 Pi。**
10 月 21 日的真实采集与播放记录仍有效，但使用旧版 VAD，不能当作本次产物的现场验收。
聊天／embedding／Easy-Turn／Juno 和固件重写继续暂缓，AEC 等用户与商家确认。

长句的根因是旧 native VAD 保留整段 active PCM；此前约 15 秒的保护会取消会话。
现已删除该实现，Rust `SileroVad` 通过 `ort` 直接运行现有 k2-fsa Silero v4 模型，
只保留 512 个输入 float、256 个 RNN 状态 float 和有界端点计数，不周期重置模型，
也不把缓存维护伪装成语音结束。启动校验模型身份、张量类型／形状及真实输出，
推理结果校验概率与有限状态，错误清理状态后返回；未强制制造 native 推理故障。

新增 `ort`／`ort-sys` 固定 `2.0.0-rc.12`，采用 API 24，关闭运行库下载和链接，
复用 Sherpa 已链接的 ONNX Runtime 1.24.4；启动先拒绝不匹配 API。
主机 VAD／端点逻辑为 Rust，推理仍使用既有 native ONNX Runtime，不宣称完全无 FFI。
两端 release 动态链接检查没有新增 ONNX 动态库要求，KWS 继续使用 Sherpa。

配置迁移：`modules.vad = "silero"`；删除 `[vad].max_speech_duration_ms` 与
`buffer_size_sec`，旧字段和旧 VAD 模块名明确拒绝。文件、Settings 和库装配统一验证
CPU、16 kHz、512 窗口及阈值。示例与本地 ignored 配置已迁移，原本地配置私有备份保留；
Pi 配置尚未更改。详见[语音配置](../../packages/alia-embedded/docs/configuration.md#voice-configuration)。

新增闭环使用公开生产 SpeechPipeline、Ed25519 签名认证、VoiceSessionService、
CaptureWindow、Delivery、EmbeddedChannel 和 PlaybackService，接真实隔离 PostgreSQL。
Linux 由真实 R1 UART owner／parser 读写 PTY，macOS 使用确定性唤醒槽；
PCM、VAD 分类、TurnDetector 结果、回复内容和 native callback 为 fixture。
覆盖 20 秒连续输入及 700 ms 尾静音完整上传，无中途假端点／换会话；
输出暂停、Backchannel 恢复原后缀、Stop 后同会话继续采集并输出新回复；
断连任务退出、scope 清理、新自然唤醒建新会话，迟到旧 Delivery／native 数据拒绝。
输出证据只到 Submitted，没有声称 Presented、物理尾音或 AEC。
非默认 `test-support` 仅复用 PTY helper 和 reset 完成观测，生产串口权限、配置与超时不变。

| 验证范围 | macOS ARM64 | Linux ARM64 |
| --- | --- | --- |
| 完整 Embedded 默认回归，14 个目标 | **373 passed / 0 failed / 1 ignored** | **391 / 0 / 3** |
| 最终 Silero 定向，显式包含本地模型测试 | **5 / 0 / 0** | **5 / 0 / 0** |
| 完整 `duplex_playback` | **32 / 0 / 0** | **32 / 0 / 0**，最终失败诊断补丁已覆盖 |
| Embedded no-default all-targets | 通过 | 通过 |
| Server no-default all-targets | 通过 | 通过 |
| Embedded release 与 native 链接检查 | 通过 | 通过 |

Rust 1.96.0、offline／locked；fmt、diff、Server 所有权边界检查通过。
各目标分别报告，不把重复测试相加；没有重跑整个 workspace。
默认 ignored 为本地模型，以及 Linux 两项需明确现场窗口的录放音测试；本地模型随后显式通过。
公开语音 fixture 两端均处理 **8512 窗口／272.384 秒**，最长连续 active 为
**401920 样本／25.12 秒**，41 个真实结束；逐窗口与固定原始阈值的 native oracle 一致，
任意分块及 reset 后重新唤起一致。概率回归另覆盖 1 小时弱语音迟滞保持。

保留失败：新 PTY fixture 首轮完整目标为 31/1，原因是异步 UART reset 的清输入
尚未结束就发新 wake；已用仅测试的明确 reset 屏障修复，没有加睡眠或延长期限。
随后完整目标又为 31/1，新 R1 场景通过，既有 Companion H1→H2 场景等 Resume 的
3 秒期限超时；同一 binary 单次定向通过。补充仅失败时的控制状态／剩余期限／数据库诊断后，
一次完整 Linux 目标 32/0。**该 Companion 根因仍未定位，V01 保持打开**，
不据后续通过认定与历史 25 秒失败同因或已修复。Mac 完整目标早于最后仅失败诊断补丁；
Linux 完整目标覆盖该补丁及诊断 SQL／隐私检查。没有修改成功路径的截止或并发。

产物已单独保存在 `run/r1-speech-20261022/{mac,linux}/alia-embedded`：

| 平台 | 字节数 | SHA-256 |
| --- | ---: | --- |
| macOS ARM64 | 54,256,592 | `d5dadbf059b1807f537448e8c861da25832be3376518267b710496758ca238b8` |
| Linux ARM64 | 69,429,176 | `36eee448958b215b7f4113eba111132113a0f1884383548e060e805cf080a7e2` |

命令、逐目标结果、输入摘要、链接信息和早期失败日志保存在本地 ignored
`run/r1-speech-20261022/`；Linux 最终完整日志为 `linux/duplex-full-with-diagnostics.log`，
`linux/duplex-full.log` 特意保留上一轮失败，不能混读。总表见 `duplex-summary.json`。
最终数据库检查为 0 测试 schema／0 其他连接；临时环境清理结果另存本轮目录。

剩余边界：Server 单个待判定 PCM 窗口仍最多 **30 秒，包含预卷及尾静音**，超限明确关闭会话；
10 秒上传 idle 仅表示无新帧，播放 acoustic candidate 的 3 秒准入期限保持。
绑定 endpoint 使用 HTTP 地址，语音端口按 HTTP+1 派生，部署映射须符合此关系。
下一步需提前告知用户，再安排新产物的 Pi → 真实 Server 同时采播与 20–25 秒长句现场窗口；
保持现固件、自然唤醒方向、内存音频和模型 fixture，不发送 beam／手动唤醒。

## R1 主机完备与现场验收（2026-10-21）

本节保留当日产物与实测事实；下述 15 秒 VAD 保护已由 10 月 22 日重构替代。

**现固件主机侧接入已完成软件与有界现场验证，固件不改，外部真实模型继续暂缓。**
本轮补齐独立采播时钟的 R1／C-Media 配置、VAD-only 模型准备、真实 PTY→speech→
正常签名 VoiceSession 回归，以及显式 opt-in 的采集和播放 fixture。纯 Rust 生产路径
复用现有模块、ALSA、Silero 与 libc，没有增加运行依赖；Python 仅用于本轮构建、SSH
和有界验证进程监督，不是 R1 生产服务依赖。

同时修复实测发现的主机缺口：串口独占的失败／退出释放，版本未确认时 capture reset
导致响应失效的有界重查；VAD 使用既有 `max_speech_duration_ms` 限制连续 active
保留，默认约 15 秒超限明确取消并等待新唤醒；播放每轮公平处理至多各音源一块后写入，
防止转换积压，同时保留混音及 Stop 优先；播放健康检查读取已运行 owner，不再重开
独占 PCM。删除了无用的播放探测 helper。没有把 VAD 内存维护伪装成正常语音结束。

| 验证范围 | macOS ARM64 | Linux ARM64 |
| --- | --- | --- |
| 完整 Embedded 默认回归，14 个目标 | **369 passed / 0 failed / 1 ignored** | **387 / 0 / 3** |
| 随后播放 owner 健康修复及音频定向回归 | **68 / 0 / 0** | **75 / 0 / 1**，优化构建 |
| 最终 VAD 单元回归 | **3 / 0 / 1** | **3 / 0 / 1** |
| 最终真实 Silero 保留超限／清空／重置 | **1 / 0 / 0**，显式运行 | **1 / 0 / 0**，显式运行 |
| 最终 no-default all-targets | 通过 | 通过 |
| 最终 Embedded release／优化实机 test binary | 未重建 Mac release | 两者通过，上传 Pi 的字节数和 SHA-256 一致 |

数字按范围分别报告，不累加重复运行。完整回归之后仅追加播放健康修复和 native VAD
测试素材修正，已对相关模块重新验证；不把 369／387 冒充最后一次修改后的全量分母。
默认 ignored 项是显式本地模型测试，以及 Linux 两个真实硬件测试。本轮没有重跑整个
workspace 或验证聊天／embedding／Easy-Turn／Juno TTS。Rust 1.96.0、offline／locked；
最终 fmt 和 diff 检查通过。

完整回归输入清单 SHA-256 为 `90d8f891aaedfb8b591fa76cb7de3625c9116ae171fc7cac2ec26e9838b756f2`。
最终构建冻结 **747 项输入**，清单 SHA-256 为
`c585aaeeca83cd9762eab41720931e990722eec141e19a6cda0bab771a999d9d`。
最终 Linux `alia-embedded` 为 **69,153,168 字节**，SHA-256：
`d74abb24070b107976051137a182498ad8016e005c63a54cf3d25f6030987a02`；
优化 lib-test 产物为 **73,187,336 字节**，SHA-256：
`e2c09a4928973125a8337a9dd19db9848ff564089b98992cbe0ca7037b9e67cf`。
最后核对仅配置说明文档在冻结后更新，其余输入一致；工作区既有未提交重构未被重置。

Pi 的两轮真人采集在北京时间 **23:37:31—23:38:46** 完成，使用真实 R1 UART、
ALSA 16 kHz／S16_LE／mono 和本地 Silero，`playback=noop`。自然唤醒分别报告
**344°／345°、beam 0、同一消息 ID 40864**；主机没有发送 beam 命令，两个事件
均被正常消费，分别建立不同的签名会话。两轮上传 **115520／86400** 样本，
语音帧 **245／220**，各取得语音后 16000 个静音样本，序号／样本游标连续。
本地 fixture 故意断连后旧任务 join、scope 清理，再次唤醒才创建新会话；这是认证和
音频传输验收，fixture 没有产生模型回复。采集只在内存处理，未保存用户录音。

该现场采集使用候选 test binary
`eccbde2e198eefe44842499e94830e432264bb23c78155919fdf6b4864989038`，
来源清单 `f1d6edd5591911a9b11b0ad28cd598806a5bd2987386b12d5e62c1f0b2390948`。
后续变化是播放调度／健康状态、诊断断言及 ignored VAD 测试，R1 控制与生产采集／VAD
保护代码未变；没有要求用户为这些输出修复重复说话，也不冒充同一产物完成全部现场项目。

优化播放 fixture 在 C-Media 上实际协商 **48 kHz stereo**：0.2 秒、24 kHz mono
合成音经过重采样和最大 0.05 的软件增益；已验证真实零值 held reference、4800 源帧
完整提交、取消的后续输出 0 提交和迟到输入拒绝。此前成功尝试输出 9600 个媒体帧，
参考峰值约 **0.003113**；最终健康修复复验通过，记录见 `pi/playback.log`：活 owner
明确报告可用，1920 个 held 静音帧、9600 个媒体帧，参考无丢块。`AlsaWriteAccepted`、参考静音与提交结束都不等于物理 DAC 呈现、
声学尾音或 AEC 通过。两块 USB 设备仍是独立时钟。

生产运行程序完成有界 **60 秒** UART／本地 VAD 启停检查，固件版本匹配；正常
SIGTERM exit 0，PID 消失，可见串口／PCM owner 为空，ALSA closed。原有服务未被
接管，未安装 systemd／开机启动、改固件参数、驱动执行器或接入 ROS。最终 Pi 私有目录为 `/home/ke-qiao/alia-validation/r1-complete-20261021-c4f20f`，
输出复验在 23:50:52 完成，最终 UART 观察为 23:50:53—23:51:53；
原始时间和身份见 `pi/deployment.json`、`pi/uart.json`，旧候选另存各 attempt 证据。

失败如实保留：首个 120 秒采集窗口超时，用户随后说明尚未开始说话；两次未优化播放
尝试分别暴露检查点超时和受控输出 underrun；补充审查发现的混音问题及健康误报已修复。
Native VAD 测试先因零阈值被 C API 回退到默认、再因极小阈值非法而失败，最终改用
合法默认配置和官方公开 speech WAV，仅缩短测试内的主机预算，两平台通过。
Mac 一轮因沙箱禁止 loopback 导致权限失败和等待，已终止本轮自有子进程，保留日志后
在允许本机测试端口的环境完成上述完整回归。早期 Linux fixture 缺少 `ensure` import
的编译失败也保留。没有通过延长期限、删除断言或忽略失败来申报成功。

本地模型来源可追溯：官方 Silero ONNX **643854 字节**，SHA-256
`9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6`。
边界测试使用[官方公开 speech fixture](https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/lei-jun-test.wav)，
不是用户录音；URL、格式和摘要均保存在 `models/` 的 provenance 文件中。

证据目录为 `run/r1-complete-20261021/`：`test-summary.json`、`mac/`、
`linux/evidence/` 保存各阶段结果，`summary.json` 汇总最终出口；`pi-attempt1`／`pi-attempt2` 保留早期失败，
`pi-attempt3` 保存真人采集成功，`pi-attempt4` 保存首次优化输出及 UART 成功，
`pi/` 保存最终健康修复的现场复验。任务构建容器已删除，无 OOM；共享 astrbot
保持运行。Pi 仅保留私有验证产物／配置／身份／标量日志，无本轮常驻进程。

当前固件仍需“小飞小飞”；AEC 继续由用户与商家确认。真实模型、AEC／双讲／
物理尾音、安装角度标定、物理热插拔及至少 24 小时混合负载尚未验收；R02 不整体关闭，
R02-F／E01／M01 的暂缓决定保持。配置与复现入口见
[配置说明](../../packages/alia-embedded/docs/configuration.md#opt-in-r1-hardware-fixtures)
和[硬件记录](hardware-deployment-readiness.md#r1-主机侧接入2026-10-21)。

## R1 现固件主机接入（2026-10-21）

新增 Linux Rust R1 UART wakeword，复用现有 libc、模块生命周期和 PCM／VAD／会话路径，
没有新增依赖。配置选择 `modules.wakeword="r1"` 并显式指定 `[r1].serial_path`；
预检、相对路径、运行配置保存和示例同步更新。事件携带来源、可选角度／波束和主机
观察时间，删除未使用且无法由 R1 score 推导的 confidence 字段。协议解析、独占 owner、
单事件／1 秒期限、最近一条唤醒的 ID＋完整载荷限时去重、版本健康、断线及重置隔离、退出 join 均已实现。
入口见[配置文档](../../packages/alia-embedded/docs/configuration.md#r1-with-existing-firmware)
和[示例](../../config/embedded.r1.example.toml)。

| 本轮验证 | macOS ARM64 | Debian 12 Linux ARM64 |
| --- | --- | --- |
| 完整 `alia-embedded` 默认测试，含文档测试 | **364 passed / 0 failed / 0 ignored / 0 filtered**，14 个目标 | **379 / 0 / 0 / 0**，14 个目标 |
| `alia-embedded --no-default-features --all-targets` 检查 | 通过 | 通过 |
| 默认 Embedded release | 未重建 | 通过；新产物在真实 Pi 通过 ELF、loader 与 ldd 检查 |

Rust 1.96.0，offline／locked；fmt 与 diff 检查通过。回归覆盖真实 UART 帧的分包／合帧、
损坏与噪声恢复、长度和超时界限、版本 ID 匹配、重复 ID 的独立唤醒、未知字段不造值、
Linux PTY 断线／握手重启／取消，以及 duplex 中采集缺口和会话退出后必须等待新唤醒。
这是本次 Embedded 变更的验证，不是重新运行完整 workspace 或真实模型链。

构建冻结 **744 项输入**，清单 SHA-256 为
`d8059f179905f41472c991753d21e388852e94b124bf5e783538a5cac420e554`。
新 Linux 二进制为 **69,225,024 字节**，SHA-256 为
`cf8b2158cdf25e1a5c0b1d2b272787db76d17aa249427c996c0896c4cae45a2e`。
验证结束、补记录前 744 项全部与工作区一致；之后仅补本文档记录。
工作区仍包含既有未提交重构，不以 Git 基点冒充本次源码身份。

早期失败独立保留：Mac 新增配置测试将 AudioBackend enum 误写为字符串比较；Linux
PTY 测试使用 i8 而 ARM64 `libc::c_char` 为 u8。两处测试类型修正后编译通过。
随后 Linux 完整测试因快照缺少已有 WASM processor guest fixture 而出现 **297/1** lib 结果；
核对 guest／PDK 源码与此前 release 相同后，补入已验证的同源 WASM
（SHA-256 `73a47c1859d0b03834d605bd50cc52e81e732aed3f1971afa10f3a408932f035`），
再完整运行得到上表最终结果。没有删测、忽略失败或改变生产实现来掩盖 fixture 缺失。

Pi 实际验证在北京时间 **22:26:31—22:26:35** 运行新二进制，使用独立目录与身份，
只有 wakeword 选择 r1，其余六模块 noop，运动／视觉／mmWave／发现关闭，监听 loopback。
`--check-config` exit 0 且未创建身份／状态；正常启动首次版本查询超时，自动重连后约
**3.1 秒**记录 `R1 UART protocol ready`，版本为
`UART_REAL, circle6:1.9.6.f8ef651@master engine:circle6.1004`。首次超时原因未单独定位。
运行快照仅见 `/dev/ttyUSB0` 和 `/dev/null`；SIGTERM **exit 0**，PID 消失，当前账户
可见串口占用为空。未录放音、加载 VAD 或调用模型，未更改唤醒词、beam、AEC 或固件。

证据说明：`pi/result.json` 的 `firmware_commands` 是验证脚本预填的允许命令范围，
不是实际串口收发记录；其中 handshake ACK 没有独立实机观测。本轮实际确认的是匹配
版本响应。原始字段保留，澄清另存 `pi/evidence-clarification.json`，不据该列表扩大结论。

证据统一在 `run/r1-host-20261021/`：`test-summary.json` 是上述分母，`mac/` 与
`linux/evidence/` 保留各次日志和命令结果，`pi/` 保存预检、产物核对、运行日志及退出结果。
任务构建容器已删除，既有 astrbot 容器保持运行。Pi 的
`/home/ke-qiao/alia-validation/r1-host-20261021-c92707` 保留本次私有二进制、测试配置／身份／
SQLite 和日志，没有常驻进程、systemd 或开机启动；此目录不是生产部署。

当前固件“小飞小飞”与音频门控仍生效；UART 验证不等于真实唤醒→PCM／VAD／Server
对话、AEC／尾音、热插拔实机或长稳验收。基础硬件采用用户已有实测，不重做录音。
R02 保留整链出口，真实模型继续暂缓，固件不改。详见[硬件记录](hardware-deployment-readiness.md#r1-主机侧接入2026-10-21)。

## Mac + Pi 实机无设备验证（2026-10-21）

用户确认 Pi 可用后，已完成真实 Mac Server 与 `ke-qiao@192.168.10.221`
上的 Embedded 两轮隔离生命周期验证。Pi 为 Raspberry Pi 5 Rev 1.0，
Ubuntu 24.04.4、aarch64、kernel 6.8.0-1065-raspi、glibc 2.39。
复用下节 10 月 20 日的 Mac Server／Linux Embedded 冻结产物；737 项输入复核中
仅既知 Embedded 配置说明文档不同，编译输入没有变化，两份二进制摘要匹配。
本轮没有生产代码或依赖改动，不重复已经通过的完整 workspace 测试。

| 检查 | 实际结果 |
| --- | --- |
| Pi 上传身份及原生环境 | SHA-256／字节数匹配；ELF、loader `--verify`、`ldd` 通过，无缺失系统库，无需安装依赖 |
| Pi `--check-config` | exit 0；七模块 noop，motion／vision／mmWave／设备发现关闭，`skip_binding=false`；检查后仅存在配置文件，未生成 key／state |
| 首次配对 | 新账号、新双端身份与数据库，经 HTTP 管理入口绑定；Server 固定实际 device ID／公钥，签名 Embedded settings 读取返回 200 |
| 第二轮恢复 | 同账号、device ID、公钥／配置摘要、绑定时间及设置保持一致，SQLite integrity 为 ok；Companion 保持 shadow |
| 访问边界 | 两轮未认证 settings 均返回 gRPC 16；已删除 Body RPC 返回 12 |
| 退出与隔离 | 两轮共四个 Alia 进程 SIGTERM exit 0，无强制结束；每轮数据库其他连接归零，最终 Mac 三端口和 Pi 两端口关闭，任务数据库容器与 SSH 隧道已清理 |

成功入口在北京时间 21:14:31—21:14:45 运行，exit 0，用时 13.890 秒。
运行期 `ss` 核对 Pi 的两个监听均为 `127.0.0.1`，Embedded 监听归属本轮 PID；
运行快照未发现音频、串口、I²C、GPIO、摄像头或 SPI 设备文件描述符。
两端均无 ERROR／panic；Server 每轮三条预期告警来自关闭的 Home Assistant
工具及 fixture 占位 discovery 名称被拒绝。共享 `astrbot` 容器保持原 ID 并继续运行。

首次尝试因验证脚本的 `ssh -F /dev/null` 同时屏蔽了既有 IdentityFile 配置，
在隧道认证处退出，尚未启动 Alia 进程；临时数据库和端口已清理。
根据该目标的 `ssh -G` 结果显式指定既有密钥后，以新本地私有目录执行了上述成功轮次。
原失败保存在 `run/pi-mainline-20261021/lifecycle-attempt1/`，成功结果、执行脚本摘要、
双端日志与清理记录在 `run/pi-mainline-20261021/lifecycle/`，没有覆盖原失败。

Pi 的 `/home/ke-qiao/alia-validation/pi-mainline-20261021-2ee32c` 保留本次二进制、
测试配置／身份／SQLite 与日志，目录为当前用户私有；没有安装 systemd 服务、
设置开机启动或修改原部署。Mac 测试数据库已删除，这些文件仅作本轮证据，
不是可直接使用的完整生产部署。

本轮采用 loopback SSH 转发和固定身份的直接绑定 fixture，
`require_confirmation=false`，设备绑定认证仍保留。
它验证真实 Pi 上的协议、状态持久化与正常关闭恢复，未验证生产发现／现场确认、
异常断电、实际升级回滚或 24 小时负载。没有打开 R1、录放音、改固件、调用模型或驱动执行器。
用户提供的 R1 实测继续采用，AEC 由用户与商家确认；R02、M01、R03、E01 和完整 R05 状态不变。

## 当前发布出口（2026-10-20）

本节的“当前”指 10 月 20 日当时的源码与产物；后续发布包及 R1 阶段安排见本页顶部。

本轮执行 [TODO S01—S03](../todo.md)，使用 Embodiment 重构后的实际工作区。
Rust 固定为 1.96.0；Git 基点为 `d2b7b6f12000535a1896dfcf7a2f4be4a2866e97`，
但工作区包含尚未提交的实现，**该提交本身不能标识本次产物**。
构建前冻结了 Cargo／配置／脚本／crates／packages／vendor／examples 下的
737 个文件，清单 SHA-256 为
`e411b72e90fb88535b02781e2b8b154285722b2b9a640fd391aa502145844f26`。
Linux 使用这一快照；Mac 构建期间仅 Embedded 配置说明文档发生修订，
编译输入、fixture、锁文件均未变化，文档差异单独记录。

| 检查 | macOS ARM64 | Debian 12 Linux ARM64 |
| --- | --- | --- |
| 单次完整 workspace，含文档测试 | **1329 passed / 0 failed / 5 既有 ignored / 0 filtered**，77 个顶层目标 | **1337 passed / 0 failed / 5 既有 ignored / 0 filtered**，77 个顶层目标 |
| Server／Embedded no-default all-targets | 两端通过 | 两端通过 |
| 当前默认 workspace release | 通过，独立副本与动态链接清单已保存 | 通过，ELF／loader／native 链接检查通过 |
| Rust PDK guest release | 通过 | 通过 |
| 独立 release 配对／正常关闭／同身份恢复 | 两轮通过，四进程 SIGTERM exit 0 | 两轮通过，四进程 SIGTERM exit 0 |

Mac 使用 `caffeinate -i bash scripts/verify-software.sh release`，退出码为 0。
Linux 使用原生 ARM64 容器、隔离 pgvector 17 数据库，以相同锁文件执行离线构建
和完整测试；编译并行度为 2，测试并发与业务期限未改。两个平台的结果分别统计，
不相加；Linux 多出的 8 项为 ALSA null／fault／close 及 PCM 名称识别测试，
不表示真实音频设备已通过。5 项既有 ignored 仍为 3 项 PTY、2 项 Easy-Turn。
Rust 格式、边界检查及边界自检均通过。

本次 Mac 独立产物位于 `run/release-20261020/mac/artifacts/`：

| 产物 | bytes | SHA-256 |
| --- | ---: | --- |
| `alia-server` | 76098464 | `a3cfc6b5a00f7c2d85f737f951859304f5ebffd7c201b2814d8a44933d6bd2e9` |
| `alia-embedded` | 55528496 | `1d96f8950aec891be9011512aa8f18e4f3c3fc051e2e42be98705de288bacf10` |
| `alia_wasm_demo.wasm` | 140289 | `f8299677e7501ca314c10fbb20c70100324b8c84f19082ebb35d5672ba011cf9` |

Linux ARM64 独立产物位于 `run/release-20261020/linux/artifacts/`：

| 产物 | bytes | SHA-256 |
| --- | ---: | --- |
| `alia-server` | 86870576 | `ab21919347a5b5ba474f393cd194190cd595a4e592d36b0fdde66aa8fb0ede27` |
| `alia-embedded` | 70262728 | `55c2584ae5aacb5b99b07db75e2af01a9494e468359f6964d3069dbc5a206128` |

两份 Linux ELF 均保留默认 features，Sherpa 原生静态库及符号身份已核对；
`file`／`readelf`／loader `--verify`／`ldd` 均通过，没有 RPATH 或缺失库。
动态依赖均为构建镜像中的 Debian 系统库，Embedded 包括 ALSA／DBus／C++
运行库；没有需要同行分发的额外 Sherpa `.so`。这不代表零原生依赖，也尚未
核验目标 Pi 上的实际 loader／系统库组合。

Mac 双进程演练从私有独立目录、全 noop 模块和新数据库开始，先经 HTTP 注册与
设备绑定，再检查真实签名的 Embedded settings 调用。相同账号、双端 key／配置、
device ID、绑定时间与设置在第二轮正常启动后保持一致；无认证 settings 返回
gRPC 16，旧 Body RPC 返回 12。两轮均先关 Server 再关 Embedded，四次 SIGTERM
退出均为 0，耗时 3.6—8.8 ms；每轮三个监听端口关闭、数据库其他连接归零。
这验证正常关闭后的恢复，没有模拟进程崩溃或断电。Unknown 不重派另由完整
workspace 的 Embodiment 操作恢复测试覆盖，此双进程演练没有物理操作。

停服后另外完成一次隔离恢复演练：`pg_dump -Fc` 导入新数据库，
`pg_restore --exit-on-error` 成功；全部 67 张 public 表共 95 行的排序行内容摘要
和行数一致，其中包含 33 行 `schema_migrations`。9 个私有文件（配置、key、
settings、SQLite 及伴随文件）复制前后摘要相同，复制后 SQLite integrity 和绑定
状态一致。副本未启动、没有尝试旧版二进制回滚，不代表实际部署还原。

Mac 演练首次运行的两轮和还原均通过，最终验证器却因 Docker 的
`error: no such object` 与其预期 `No such object` 大小写不同而退出 1。
原 `lifecycle/result.json` 和退出码保留；修正验证器后仅追加只读
`lifecycle/cleanup-reconciliation.json`，确认任务容器已删除、四个 PID 消失、
三个端口拒绝连接，没有重跑配对或还原。源码与产品行为未因此修改。
日志还保留 noop fixture 的预期告警：禁用的 Home Assistant 工具不可用及占位
discovery 名称被拒绝；没有 ERROR／panic。

Linux 使用同一生命周期验证逻辑与新任务数据库，两轮绑定／恢复、四进程正常
SIGTERM、监听关闭和数据库连接归零均通过，入口退出码为 0。两平台任务临时
容器均已回收，Linux 内部网络及任务匿名卷也已删除；共享 `astrbot` 服务未操作。

原始日志、每个顶层测试目标的最终计数、输入清单和产物身份保存在本地 ignored
目录 `run/release-20261020/`。`mac/workspace-results.json` 与
`linux/evidence/workspace-counts.json` 均排除了测试内部子进程的重复汇总。
这里新增的是当前源码的统一全量结果；下方 10 月 19 日的首轮失败与按目标汇总
继续保留，不能把本次通过倒推为 V01／V02 历史异常根因已解决。

R1 已在指定 Pi 上完成只读 USB／ALSA 枚举，结果见[硬件资料](hardware-deployment-readiness.md)；
后续获准的 10 秒采集因既有 `arecord` 占用而未取得 PCM，临时音频已删除；
用户要求保留现有录音并结束本轮，没有重试、播放或部署本次二进制。
真实模型、ROS 和 R03 执行器仍暂缓。
成套升级及失败恢复步骤见[升级与恢复](upgrade-recovery.md)。

## Embodiment 地基重构验证（2026-10-19）

本节对应 Harness 迁移之后的 Embodiment 重构，使用 Rust 1.96.0、
`aarch64-apple-darwin`、真实临时 PostgreSQL 与本地 HTTP/gRPC fixture。
实现和接入边界见 [Embodiment](../architecture/embodiment.md) 与
[Motion contract](../architecture/motion-contract.md)。本轮未新增第三方依赖。

旧 Body 的类型、RPC、签名域、持久投递和查询入口已迁为 Motion；传感器归入
Embodiment。新增独立物理操作复用 Execution 的权限、确认和效果身份，区分
持久提交、设备接受、任务终态和停止证据。默认没有物理适配器，只提供历史状态查询。

| 检查 | 结果 |
| --- | --- |
| workspace 首轮，含文档测试 | 77 个目标，1328 passed / 1 failed / 5 既有 ignored / 0 filtered；失败仅为下述 runtime fixture |
| 修复后的完整 `server_runtime` 目标 | **102 passed / 0 failed / 0 ignored / 0 filtered**，保持原并发和期限 |
| 各目标最新结果汇总 | **1329 passed / 0 failed / 5 既有 ignored / 0 filtered**；这是按目标合并的验证结果，不是另一次统一全量运行 |
| 新操作与迁移回归 | 操作 11/11、迁移 8/8；含跨服务排序、独立取消、超时 owner、Unknown 资源保留、旧 JSON 与迟到检查点保留；已计入 workspace |
| HTTP 与表达边界 | 新 Embodiment HTTP 2/2、外部插件不能恢复已禁用自动动作的回归通过；均计入上述目标 |
| Server／Embedded no-default all-targets | 两端均通过，无编译警告 |
| Rust 格式、边界检查及其自检、diff 空白检查 | 均通过 |

首轮统一入口为 `caffeinate -i bash scripts/verify-software.sh test`（指定本任务临时
`ALIA_TEST_DATABASE_URL`）。唯一失败是双 Server 配置策略测试关闭时记录了
output-control 恢复事务结果未知。定向计时发现，该当前线程测试内默认
`reqwest::Client::build()` 同步耗时 **7416 ms**，紧接着的 `no_proxy()` 构建不足
1 ms；这足以耗尽原 2 秒数据库期限。计时单例当次通过，因此该测量证明阻塞源，
不冒充同一次 shutdown 失败的完整重现。

修复统一了 `server_runtime` 中的回环客户端：使用已有无代理 helper，定制客户端
仅增加 `no_proxy()`，保留原请求期限。生产代理策略、业务期限和并发均未改。
之后完整执行 `cargo +1.96.0 test -p alia-server --test server_runtime --offline --locked`，
102 项全部通过；两端 no-default all-targets 再以最终源码分别检查。
首轮之后只有该测试目标下 8 个 fixture 文件变化，生产实现、迁移与其他目标输入未变。

完整日志和按目标计数保存在 ignored 目录 `run/embodiment-20261019/`：
`workspace-final.log` 是保留失败的首轮全量日志，`workspace-first-results.json` 为其计数；
`context-policy-proxy-diagnostic.log` 保留阻塞计时，临时计时代码已撤除；
`server-runtime-final.log` 为修复后的完整目标，`latest-target-results.json` 为按目标汇总。
另保留两端 no-default 日志与输入摘要、fixture 差异摘要。任务专用 PostgreSQL 容器已删除。

本轮没有接入 ROS、真实硬件或真实模型，也没有重新构建宿主 release。
下方 Harness 的二进制及摘要属于 Embodiment 迁移前源码，不能作为本次发布产物。
新 Motion 协议要求 Server 与 Embedded 成套升级；停止证据和共享执行器仲裁仍需由
后续具体适配器及设备端兑现，软件回归不能证明实际电机已经停止。

## Harness 迁移验证（2026-10-19）

本轮迁移使用仓库 `rust-toolchain.toml` 固定的 Rust 1.96.0。验证脚本显式选择该工具链，先构建 `examples/wasm-plugins` 的真实 Rust PDK guest，再运行边界检查、workspace 回归和两端 no-default all-targets；release 模式追加宿主 release 构建。依赖需提前按 Cargo.lock 获取，验证阶段使用 `--offline --locked`。

本轮已完成 Rig、工具构建、MCP、Extism 与 fetch 迁移。Rig 使用 [Ke-Qiao/rig 的 codex/alia-harness 分支](https://github.com/Ke-Qiao/rig/tree/codex/alia-harness)，Cargo 固定提交 `c2ff3e30e6b8d84fc6759eab030656a42cae14ca`；原三套 provider/SSE 实现、文本工具回退、旧 ToolExecutor 和 Python 表达插件入口已删除。当前配置与迁移说明见 [MCP 工具服务](mcp-tools.md)、[WASM 插件](wasm-plugins.md)和[工具契约](../concepts/skills-and-tools.md)。

最终统一命令为 `caffeinate -i bash scripts/verify-software.sh release`，在接电的 `aarch64-apple-darwin` 主机上以原默认并发和原时间预算完成，退出码为 0：

| 检查 | 结果 |
| --- | --- |
| 默认 workspace，含文档测试 | **1314 passed / 0 failed / 5 既有 ignored / 0 filtered**；76 个顶层目标，子进程结果不重复累加 |
| Server／Embedded no-default all-targets | 均通过，无编译警告 |
| 边界检查、自检与 Rust 格式检查 | 均通过 |
| 前端 | 类型检查通过；请求回执恢复逻辑 4/4 通过 |
| Rust PDK guest | `wasm32-unknown-unknown` release 构建通过；真实 WASM 的安装、授权、Execution、升级失效与撤销回归已计入 workspace |
| 当前宿主 release | Server 与 Embedded 均构建成功，Mach-O ARM64；已保存独立副本及动态链接清单 |

5 项 ignored 仍为 3 项真实 PTY、2 项真实 Easy-Turn，没有新增忽略项。最终整套中双向语音为 31/31、Server 运行时为 100/100、fetch 为 7/7、MCP 为 9/9，均已计入上述 1314 项。Rig 受改动的四个 crate 另有 **1677 项单元测试与 273 项文档测试通过**，记录见 fork 的 [ALIA_TEST_RESULTS.md](https://github.com/Ke-Qiao/rig/blob/c2ff3e30e6b8d84fc6759eab030656a42cae14ca/ALIA_TEST_RESULTS.md)，不并入 Alia 计数。

完整日志、按目标计数、3141 个输入的 SHA-256 清单及构建身份保存在本地 ignored 目录 `run/harness-20261018/`。主日志为 `logs/alia-harness-final-20261019.log`，计数为 `workspace-tests.json`，产物摘要为 `artifacts.json`。输入清单摘要为 `0863550a99377f6fe905aedfdc0eed0ff3903cca55f944e6f2bc08f4e5eff944`；从验证开始到 release 结束，输入逐项复核无变化。本节验证结果在构建后写入，其文档差异单独记录在 `build-identity.json`，不冒充受测源码变更。任务临时 PostgreSQL 已由脚本清理。

| 独立产物（位于上述目录的 `artifacts/`） | bytes | SHA-256 |
| --- | ---: | --- |
| `alia-server` | 75629680 | `ddd1b77ab5305537aa041cedeeda5a14312d624f7270c50d72c615e8be29b33a` |
| `alia-embedded` | 55523632 | `2d75c25f9ea64336efcd0a04ca3ac9931dd65fffc832b75ecb5b8e9c6593eef4` |
| `alia_wasm_demo.wasm` | 140289 | `f8299677e7501ca314c10fbb20c70100324b8c84f19082ebb35d5672ba011cf9` |

失败历史也保留在同一目录：修复了 JSON 对象顺序对持久摘要的影响、独立默认模型遗漏生命周期、旧 fixture 参数及健康状态期望；本机同步系统代理扫描会阻塞当前线程测试运行时，回环 TTS／health／skills fixture 因此显式禁用代理，生产代理政策未改。前一整套曾有两项语音超时，主机系统日志同时记录了 23:26:39 至 00:10:27 的低电休眠；原测试日志不能证明具体哪项被休眠中断，因此只记录相关环境证据。另两次独立诊断出现的时间准入／数据库截止失败未确定根因，临时诊断已撤除；最终原期限、原并发的统一整套通过，未以放宽条件处理这些失败。

这里验证的是本机软件构建与 fixture 运行时，包括真实 PostgreSQL、回环 HTTP/gRPC 和真实 WASM。没有启动真实聊天／embedding／TTS 模型，没有验证生产 MCP 服务、独立 release 进程部署、真实音频设备或 Linux/Pi 实机。新增宿主与 PDK 使用 Rust，既有音频／推理的原生依赖仍存在。下方 1.95.0 的 1324 项结果属于迁移前修复，不能替代本节结果。

## 迁移前正确性修复验证入口（2026-10-18）

在仓库根目录运行 `bash scripts/verify-software.sh test`。入口先检查模块边界，
再运行默认 workspace 测试和两端 no-default-features 检查，任何步骤失败均返回非零。
未提供 `ALIA_TEST_DATABASE_URL` 时，使用本机已有的 `pgvector/pgvector:pg17` 镜像创建
仅监听 loopback 的临时 PostgreSQL，结束时删除本次容器；不会下载镜像或使用开发数据库。
也可显式提供**可丢弃**数据库。真实 PG 测试缺少连接信息时必须失败，不再静默跳过。

`bash scripts/verify-software.sh release` 在相同检查通过后构建当前宿主平台的两端
release 二进制（`target/release/alia-server`、`target/release/alia-embedded`）。
这不代表 Linux/Pi 交付；修改过 Embedded 输入后，下面的历史 Embedded 产物也不能复用。
真实模型、硬件和 ignored 的现场测试仍需分别验收。

迁移前工作区在 Rust 1.95.0、`aarch64-apple-darwin` 上执行结果：

| 检查 | 结果 |
| --- | --- |
| 默认 workspace | 1324 passed / 0 failed / 5 既有 ignored；没有过滤测试 |
| 最后补充的插件参数契约回归 | 1 passed；明确拒绝非字符串 payload，单独运行于 workspace 检查之后 |
| Server／Embedded no-default all-targets | 均通过 |
| 当前宿主 release | 两端均构建成功，Mach-O ARM64 |
| 全 noop 配对恢复 | 真实签名 loopback 两端运行时首次绑定、关闭、同身份重启及受保护接口访问通过；已计入 workspace |
| 验证脚本失败路径 | 自动准备临时 PG 后注入失败，保留原退出码并删除本次容器 |

完整日志、按顶层 test target 汇总的计数及产物摘要位于 ignored 目录
`run/correctness-20261018/`。该计数不重复累加测试内部启动的子测试进程。
最后的 caller 修复已由独立回归覆盖，随后 no-default 检查和 release 均使用最终源码。
两端 release 独立副本位于其 `artifacts/` 子目录；SHA-256 分别为：

- Server：`12445313224cbe940d791de747ac9354e0ddd13afdb2d2b2bca08c924e691552`
- Embedded：`2ac4eab5abf42cf2f6539fa4ce183fac0a7ee2cb696532a68f371d6a82658e7f`

任务测试数据库连接归零后已删除临时容器。配对回归运行于同一主机的测试运行时，
不代表两个 release 可执行文件的跨机部署、实机断电恢复或真实模型链。
本轮同时修正了独立 default chat profile／noop 的实际装配；下文历史记录中的该缺口不再适用于当前源码。

## 2026-10-17 历史发布证据

以下记录按原验收时间保留，不能替代上述当前源码结果。

更新：2026-10-17。资料基线 `6a8b113`，当前已验应用源码 `6c62c84ee60e3a5fd387f2172ead432031907599`，分支 `codex/architecture-refactor`。本页汇总已有软件证据和后续模型验收所需输入；不是已完成真实模型部署或硬件发布的声明。本轮只读核对、修正文档与示例注释，没有探测端点、启用服务、运行模型或安装依赖。

## 当时可确认的软件出口

| 平台／层次 | 受测版本与结果 | 结论边界 |
| --- | --- | --- |
| Debian 12 Linux ARM64，Rust 1.95 | `6c62c84`：默认workspace 1287 passed / 0 failed / 5既有ignored / 0filtered；两端no-default all-targets、default release与native链接通过 | 当前Linux构建／测试／链接出口；不是Mac或Pi部署结果 |
| Linux全noop双端进程 | 同版本两轮启动→SIGTERM正常关闭→同配置／DB／身份／期望设置恢复；四进程exit0，无强杀，端口关闭、任务DB连接归零 | 未绑定的软件实例；不是绑定恢复、crash／断电或真实设备／模型验收 |
| 当前Mac | `6c62c84`两项取消归属回归通过，原RED保留 | 当前源码的定向证据，不是当前Mac完整release／整链出口 |
| 最近Mac完整workspace | 旧源码`4e2968f`：1277/0/5 | 仅历史；不能与当前Linux结果合称同源双平台全量 |
| 真实Easy-Turn | 2026-09-08固定CPU组件16/16；09-28两项live目标历史通过 | 历史模型组件／Easy-Turn软件集成，不认证当前部署或完整LLM/TTS链 |

当前5项ignored仍为3项PTY、2项Easy-Turn；本轮没有改变忽略状态或追加动态测试。原测试期限、默认并发和资源owner规则保持。详细原件见[上轮四份HANDOFF](../refactor/handoffs/2026-10-17-release-lifecycle/README.md)、[148份索引](../refactor/handoffs/2026-10-17-release-lifecycle/evidence-index.json)及[模型验收边界](../refactor/handoffs/2026-10-17-model-preflight/acceptance-HANDOFF.md)。上轮一处Cargo标题分块有独立派生行映射，原日志、原INVALID计数和失败结果均保留，不是重跑求绿。

## 发布产物、配置与恢复资料

以下为已经保留并重新只读核对的Linux产物，不能在Mac上直接作为本机可执行文件使用。实际Mac发布还需对应平台的构建、链接和运行证据；此处不扩展到其他平台。

| 文件 | bytes | SHA-256 |
| --- | ---: | --- |
| `alia-server` | 46092208 | `0374aa62602a3e6c2908459b7e8a4a574bd937071d182dbb18172eccb0695616` |
| `alia-embedded` | 49540200 | `159becdbd3783b77ad308d02096ed39158fc54cfef43f648091a2c0f015f4003` |

独立副本位置：仓库ignored目录 `run/validation-20261017-release-lifecycle/v2/artifacts/`。对应受测678输入manifest为 `0133b6f493b3c8b39b365abe03e3cfc035d88b0f21962e0ed6abd745796ed8c7`，逐Git blob匹配`6c62c84`；实际copy／链接审计／两轮进程摘要一致。本轮资料修订只删除Server示例中两行过期注释，TOML语义前后相同；不把注释修订说成一次新构建或篡改旧manifest。

- 配置结构与敏感值来源：[Server配置](../../packages/alia-server/docs/configuration.md)、[模型服务契约](model-services.md#configuration-and-acceptance-boundaries)。base、排序`.d`、env补空规则、persisted settings及SQL政策应分别核对，不能只看一份TOML。
- 无设备准备：[Embedded staging模板](../../config/embedded.staging.example.toml)，仅用于私有目录的全noop实例。通用[Embedded示例](../../config/embedded.example.toml)选择真实采播，不能当成无设备预设。
- 启停与正常恢复：[无前端操作入口](server-embedded.md#配置与启动)。首次设备ID写回配置是预期，保留key、state、配置及Server settings；设置已保存不保证模块应用成功。SQLite完整性与schema通过不等于文件字节或物理状态相同。
- 失败发布与重试：[配置发布恢复](../refactor/config-durability-2026-10-11.md)、[运行时设置部分成功](../refactor/settings-recovery-2026-10-11.md)。保留原阶段错误和持久身份，不以删除恢复记录或覆盖SQL政策代替核对。
- 旧任务容器／卷／network已回收，共享服务保留；ignored私有目录不是可直接复用的完整部署。新验收先确认独立目录、数据库及资源归属，不能凭旧容器名继续启动。

## 模型与部署输入登记

**2026-10-24 当前安排：真实模型方案继续待定。** 下方旧配置与本地资产盘点仅作为选型资料，不代表新方案已确认。方案确定后，集中做一次“生产启动 → 讲话 → 模型处理 → 回复播放 → 打断／恢复”的完整验收；将此前 R1 同轮组合缺口纳入该轮，AEC 等商家确认后另行验证，不提前重复硬件基线。

已确认目标仍为当前开发Mac运行Server，Easy-Turn位于Server端；Pi5不运行Easy-Turn，R03继续暂停。用户已答复三组问题：对话与embedding具体方案待定，拟使用`https://botcf.com/v1`测试；Easy-Turn与GPT-SoVITS在Developer目录，TTS使用Juno权重，Whisper在项目内，按旧配置核对；当前是开发验证，必要时可用容器。开发验证沿用当前账户与任务临时独立目录，不要求先建立正式服务账户／托管方案；正式Mac/Pi部署另行验收。

| 输入 | 已有契约／需登记字段 | 当前实际值 |
| --- | --- | --- |
| major／minor聊天 | provider ID、base URL、API family、模型ID及版本、认证来源；default选major或minor | 拟测试基址`https://botcf.com/v1`；精确模型／API能力／认证来源仍待定，未访问该地址 |
| 可选聊天角色 | memory_capture专用或minor fallback；sentence_expression专用或确定性规则；逐项核对是否远端调用 | 待用户确认，示例不代表选择 |
| embedding | OpenAI兼容端点、独立模型ID／版本、实际维度、认证来源及SQL active profile | 用户明确待定；不假定拟用基址支持embedding，不擅自关闭Memory/cache或重写政策 |
| Easy-Turn | 既有隔离环境、source/checkpoint/Qwen目录及修订，loopback端点；仓库固定Python3.10 CPU栈 | 用户指定本地Developer目录，目录盘点见下方交接；运行状态与实际端点未验 |
| GPT-SoVITS | 服务版本、完整`/tts`地址、voice、服务端参考音频/文本/语言与GPT/SoVITS权重身份；独立还是共享实例 | 用户指定Developer内服务及Juno权重；精确组合／参考素材／loaded identity仍需核对 |
| 独立Whisper转写 | 是否列入首轮显式HTTP转写范围；基址、服务版本、实际加载模型、样本语言 | 用户指定项目内资产；首轮范围未定，VoiceSession不需要第二次STT |
| Mac开发验证 | 当前账户，任务独立配置／key／state／输出路径，PG及服务归属 | 可沿用开发账户、必要时容器隔离；本轮不创建容器，正式服务账户不作当前门槛 |
| 认证及输入输出 | 受控文件位置或变量名；可发送的非敏感文本／离线PCM，私有输出目录，若远端计费则有限调用范围 | 待确认，不收集密钥值，不自动访问外部端点 |

只给模型ID或一个URL不足以证明加载了指定权重；记录服务版本／部署清单与实际加载身份。完整GET settings可能包含敏感原值，不应直接归档；TTS参考路径按TTS主机解释。依赖、资产缺失应标待准备，本轮不自动下载、升级锁文件或启用服务。

用户要求参考旧配置后，只读提取了`config/server.toml`与27个`.d`片段、`config/settings.toml`中的必要非敏感字段，没有归档完整私有配置或凭据。base/fragments中的major和minor均为DeepSeek `deepseek-v4-flash`，sentence_expression为Gemini 2.5 Flash，embedding为OpenAI `text-embedding-3-small`／1536；Whisper为旧localhost:8080基址和large-v3说明标签，TTS旧localhost:9880/tts但参考与权重路径为空，没有Easy-Turn配置块。persisted settings另有local embedding／Nomic768、不同minor模型以及STT/TTS noop等历史值。**这不是一份已确认的新方案，也不能仅按其中任一文件宣称有效配置**；下一轮先建立任务独立配置并按实际注册项／应用时序核对，避免直接启动旧配置触发不同供应商或不完整TTS配置。具体字段与路径由[配置交接](../refactor/handoffs/2026-10-17-model-preflight/config-HANDOFF.md)记录，指定模型目录盘点见[验收交接](../refactor/handoffs/2026-10-17-model-preflight/acceptance-HANDOFF.md)。

已找到的本地候选如下；这里只记录checkout／磁盘配置／文件存在，未执行或证明实际loaded身份：

| 组件 | 本地盘点 | 后续具体缺口 |
| --- | --- | --- |
| Easy-Turn | `Developer/Easy-Turn`和项目`run/models/easy-turn-source`的HEAD均为钉住的`fe175eaa…`；checkpoint字节数相符，Qwen小manifest修订相符；三个官方PCM的SHA匹配 | 未重新hash大权重或检查模型checkout全部dirty状态，运行版本／端点未验 |
| GPT-SoVITS | `Developer/GPT-SoVITS` HEAD=`08d627c…`，现有配置为CPU／v2ProPlus，指向`Juno-e30.ckpt`＋`v2Pro/Juno_e16_s80.pth`，文件均存在 | 项目未选参考音频与匹配文本；六个demo WAV存在但未读取／听取。`go-api.command`默认`0.0.0.0`，未来开发启动需明确loopback参数 |
| Whisper | 通过项目安装脚本定位至`/Users/ke-qiao/.local/share/alia/whisper.cpp`，HEAD=`968eebe…`；已有server二进制与`ggml-large-v3-turbo.bin` | 实际目录不在项目内；旧配置`large-v3`仅说明标签，需按实际启动参数确认turbo，未执行二进制 |

这些发现使后续可复用已有资产，不要求重新安装或容器化模型。只有需要PG等隔离资源时再使用必要容器，开发模型服务的具体启动参数与资源归属按有限验收步骤确定。

## 下一轮有界执行门槛

1. 基于本轮答复与目录／旧配置盘点，补齐待定的模型ID、协议和素材选择，再形成可审阅的任务配置及调用清单。包含所有可触发的角色、embedding、TTS初始化切权重、配置覆盖层和SQL政策身份；未选服务不能沿用旧配置或真实示例默认值。
2. 确认服务和数据归属、允许的启动/停止及有限请求范围后，分别验证真实组件。保留默认`health_probe_enabled=false`和shadow；health作为状态记录，真实输出和服务端加载身份单独核对。请求失败先保留和定位，不改原期限/并发求绿。
3. 复用既有Easy-Turn组件/live测试与固定离线PCM；正弦或静音PCM仅用于传输，不作语义准确性样本。两项现有live测试只证明Easy-Turn，并没有真实LLM或TTS生成。
4. 方案确定并完成必要组件预检后，在一个已通知用户的现场窗口完成“生产启动 → 讲话 → 模型处理 → 回复播放 → 打断／恢复”。复用当前 CaptureControl／VoiceSession／Generation／Delivery／Playback，记录原会话、输出和取消归属，验证真实回复及清理；音频保存范围届时明确，不能沿用历史 fixture 代替真实结果。原同轮长句与全部控制、生产初始化与真人语音缺口并入此轮，不提前另做重复采集。
5. 保存失败分母、源码／模型／样本身份、冷启动与后续延迟、请求和服务原owner清理证据。不能把HTTP取消或超时当作native推理已经结束，也不终止不属于本次验收的共享服务。

## 保留的后续范围

以下保留 10 月 17 日当时的资料缺口与状态；最新软件、R1 阶段收口及实际恢复条件以本页顶部和[当前 TODO](../todo.md)为准，不按这些旧缺口重新派工。

真实模型链尚未验收；Mac当前完整发布矩阵、服务账户部署和长稳也未由Linux证据替代。R02仍缺R1板/固件、输出链与AEC电气资料，R03在用户恢复前保持暂停，R05等待软件与现场门槛。历史V01根因仍open，default独立第三LLM profile的校验/实际选择差异只记录为后续需明确的边界，本轮不改实现。前端与通用Embedded适配仍延期；R01—R06仍2/6，不换算整体百分比。

本轮状态及下一次恢复入口见[checklist](../refactor/checklists/model-preflight-2026-10-17.md)和[恢复检查点](../refactor/resume-checkpoint-model-preflight-2026-10-17.md)。
