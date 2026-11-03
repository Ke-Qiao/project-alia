# Model Services

Project Alia treats models as replaceable services behind provider adapters.
This document lists the current service roles without going deep into provider
installation details.

The [software release record](software-release.md) separates verified binaries,
component checks and actual deployment evidence. The dated sections below retain
their original scope; the latest validation status is recorded first. Example
URLs and model names alone are not evidence of deployment or acceptance.

## 已听到回复，分段等待导致播放退出（2026-11-03 深夜）

**用户已明确听到完整回复；同一窗口随后暴露了第二个播放错误，不能把这次听感成功写成连续对话通过。** 普通重启后的138ee28／PID6358正确识别指定短句，minor生成43字回答，Juno返回HTTP200；窗口由用户反馈结束，约64.350秒进入精确Stop，无整场倒计时。本轮没有重复UART初始化或要求R1再断电，说明这一次普通程序重启后实际人声输入和可闻回复都已走通。用户确认、Mac／Pi原始日志和只读SQL分别保存在[本轮报告](../../run/voice-acceptance-20261103-fixed/playback-recovery-next-window/report.md)及[完整日志](../../run/voice-acceptance-20261103-fixed/playback-recovery-next-window/acceptance-complete.private.log)。

第一条Delivery最终仍为unknown：`audio worker closed; final device state is unknown`，源格式32kHz单声道，offered82702／accepted76302／submitted72830，End未收到。Pi明确报`native underrun lost controlled playback continuity`；之后另一个识别输入生成了57字回答，但注册遇到`playback backend is not running`。用户听感和程序是否正常结束是两项事实，均保留。Easy-Turn三次处理约1215／762／912毫秒，均成功；这次没有判句超时。SQL终止事件因高频progress挤满原查询上限而另做限定owner／两response ID补查，未改数据或重放unknown。

已找到源码中的具体问题：Juno分段合成之间会等待数秒，而ALSA只在有媒体或暂停时写声卡；正常等待下一段时不补静音，60毫秒缓冲耗尽后下一次写入触发致命underrun。旧代码在真实libasound null回归上失败，修复改为受控输出等待期间实际补静音。静音单独记录为ZeroFill，不增加回复帧数、不冒充End；暂停、取消、旧代次和真实设备错误保持原处理。此次EPIPE原因与更早缺少errno的drop错误分开记录，不倒推所有历史故障同因。

修复已普通重启安装为独立Pi版本**3bd88c8**／PID7162／start ticks4272042，boot `b496a261-8ed1-4251-9c5b-1e7e2c9c2c7f`。签名Inspect为generation1、inactive、available；capture7170 RUNNING，playback7169 PREPARED、hw／appl pointer均0。旧PID6358正常退出；绑定、SQLite、设备key、voice配置、五claims全部保持，未发UART初始化或CaptureStart。Mac仍8481／PID14258，模型只读就绪通过，未另做推理。见[部署记录](../../run/voice-acceptance-20261103-fixed/alsa-stream-gap/deploy/voice-activation-complete.json)、[启动日志](../../run/voice-acceptance-20261103-fixed/alsa-stream-gap/deploy/new-process-startup.log)与[软件验证](software-release.md#alsa-分段等待修复2026-11-03-深夜)。

**下一步只需用户在场做连续两轮短句。** [入口与记录要求](../../run/voice-acceptance-20261103-fixed/alsa-stream-gap/next-window/README.md)已准备；取得当窗Ready才讲话，由用户反馈结束。要同时确认两条回复可闻、Delivery收到End并正常结束、原播放线程仍在；不重选模型、不再次初始化、不放宽期限。修复版尚未执行真人窗口。随后再集中做正常换气长句、打断／恢复、取消与下一轮及Start／Stop；24小时负载、应用级升级恢复、AEC和具体插件仍独立未完成。

## 播放后端退出与显式 Start 恢复（2026-11-03 晚）

**用户反馈结束的 feedback04 持续约69.875秒，仍没有回复；这次已确认是 Pi 播放 worker 退出后，新回答无法注册播放。** 两次指定句均识别正确，minor分别生成47字和43字回答，随后两次注册收到 `FailedPrecondition: playback backend is not running`；新增Delivery和Juno合成请求均为0。没有整场倒计时，也不是仍在等待聊天模型。该失败发生于Pi `58bb11e`／PID4532；随后已普通重启部署修复版`138ee28`／PID6358，Mac仍为`8481b0f`／PID14258。见[feedback04结果](../../run/voice-acceptance-20261103-fixed/feedback-04-review/report.md)和[用户结束窗口](../../run/voice-acceptance-20261103-fixed/feedback-round-04/window/summary.json)。

Pi日志将退出定位到更早窗口02期间的一次ALSA `pcm.drop()`失败。旧日志只有外层 `stopping ALSA playback`，缺少当时errno与PCM state。后测Pi时钟比Mac快约3.947—5.311秒，更符合首次注册的前置Stop，但不能证明历史时差不变，因此**不能断定最终CaptureStop触发了退出，也不能仅用计时Stop解释窗口02无声**。独立C-Media输出声卡的USB reset／descriptor error -71有内核记录；R1采集是另一路USB。它支持旧输出句柄可能失效的解释，尚未证明该次drop失败的底层根因。见[时序与源码复核](../../run/voice-acceptance-20261103-fixed/playback-lifecycle-review/source-review.md)、[时钟与设备](../../run/voice-acceptance-20261103-fixed/pi-playback-failure/clock-and-cards.json)及[内核事件](../../run/voice-acceptance-20261103-fixed/pi-playback-failure/kernel-audio-events.log)。

随后在同一真实输出声卡上做零样本检查：新句柄open→prepare／drop→prepare／drop→close均成功，没有write或start，原R1采集PID／流未变。这证明当前输出设备可以重新打开，不等于旧worker已恢复，也不证明下一次USB重置已解决。隔离null PCM实验也未复现原故障，不能把空媒体或重复Stop直接定为底层原因。见[真实输出零样本结果](../../run/voice-acceptance-20261103-fixed/playback-regression-review/pi-output-probe-result.json)。

已实现仅恢复输出的修正：显式、已认证的CaptureStart先核对绑定、boot／generation与无活动会话；仅播放worker不可用时，在旧scope已退休、已进入的写入均结束后，调用现有播放模块start。恢复任务登记到既有任务集合，阻塞启动始终等待完成；返回后重新核对调用方取消、绑定、代次和播放可用状态，再创建新会话。采集流与R1初始化不动，网络播放入口不自行打开设备，旧unknown输出不重放。原生播放启动在ready通道断开或明确失败时也会shutdown并join线程；ALSA drop／prepare失败增加阶段、前后state、errno和完整错误链，继续返回原失败，不更改停止行为或任何期限。

本次更换Embedded二进制已正常停止旧PID4532（exit0）、普通启动新PID6358；这与修复生效后在同一进程内仅恢复播放是两个阶段。没有发送UART初始化或CaptureStart，不要求用户再次断电。新进程start ticks为4071196，boot为`905e3d8c-79d5-4c36-b005-22f04e1578d5`，签名Inspect为generation1／inactive／available；capture线程6366的PCM为RUNNING，output线程6365的PCM为PREPARED且hw／appl pointer均为0。原SQLite逻辑内容、设备key、配置和完整五份claim一致。原初始化进程的采集流已随正常停止而关闭，当前是新进程重新打开的采播句柄，不能写成仍保持旧流。见[实际部署](../../run/voice-acceptance-20261103-fixed/playback-recovery-deploy/voice-activation-complete.json)及[签名状态](../../run/voice-acceptance-20261103-fixed/playback-recovery-deploy/signed-inspect-after.json)。

下一次按[已准备的真人入口](../../run/voice-acceptance-20261103-fixed/playback-recovery-next-window/README.md)核对实际安装身份与新鲜Mac就绪，取得本窗Ready才提示讲话，由用户反馈结束。普通重启后实际人声质量、可闻完整回复及真实播放故障恢复仍待验证；这次部署没有打开语音会话。

macOS ARM64已各执行一次：`system_capture` **9通过／0失败／0忽略**，`playback_startup` **2通过／0失败／0忽略**；7文件格式检查通过，验证前后源码hash一致。使用fake后端，没有真实设备或模型调用，Mac不覆盖Linux ALSA分支。见[Mac定向验证](../../run/voice-acceptance-20261103-fixed/playback-recovery-validation/test-run.json)。Linux ARM64也各执行一次：`playback_startup` **2/0/0**、`system_capture` **9/0/0**、`runtime` **11/0/0**、`preflight` **6/0/0**；Embedded no-default all-targets及首份单包release的ELF／链接、三份配置预检通过。首份构建仅选择Embedded，与旧发布的workspace共享依赖feature集合不同，因此不选作部署件；原workspace release已另行完成，231.737秒通过，最终ELF／链接及三份配置检查通过；没有重跑已通过的测试。两次任务容器无网络、无设备、无PG、无真实模型且已清理；这些结果按目标分别记录，不与历史或Mac数目相加。

主线修复`ebb537d`仅移植到现有Pi源码`58bb11e`，得到独立提交`138ee2838a599f5271035bb4e9550254002edf65`。首份单包产物与[原清单](../../run/voice-acceptance-20261103-fixed/playback-recovery-candidate/candidate-manifest.json)保留作证据，最终workspace产物与[清单v2](../../run/voice-acceptance-20261103-fixed/playback-recovery-candidate/candidate-manifest-v2.json)另存，70,865,176字节、SHA-256 `2d2e8f3016e40458794fe8d836503f86bfc1b498c0d5e3c8088069aa85548704`；不覆盖旧结果。实际安装与运行文件已核对该最终产物，完整真人回复尚未通过。Server的V03恢复候选仍受V05限制，没有随本次Embedded修复部署；V01—V07、长句、打断／恢复、取消下一轮、24小时负载、AEC及具体插件各自保留。

## 修复后冷启动成功，回复在播放前取消（2026-11-03 晚）

**本次 R1 初始化成功；用户重复指定句三遍但没有听到回复，短句对话仍未通过。** Pi 使用 `58bb11e`，新冷启动只初始化一次，版本查询和 beam 0 命令成功；随后 PID4532、start ticks3513578及原生采集FD17保持不变。原四份claim未改，新成功claim另存，总计五份。没有在窗口之间重启、恢复noop或再次初始化。见[初始化记录](../../run/voice-acceptance-20261103-fixed/round/initialize.json)与[连续采集核对](../../run/voice-acceptance-20261103-fixed/round/window-01-continuity.json)。

这一窗口 Start→Ready 后立即提示讲话，30.004秒进入清理，精确Stop收到inactive回执。Stop关闭上传和本轮输出，本地原生采集连接继续保持，不能写成物理麦克风关闭。用户确认自己说了三遍；宿舍其他人的声音可能导致打断，但三条保存的转写尚不能逐一归属这些讲话。转写内容都明显不同于指定句，原文仅保存在本机私有记录中。见[窗口](../../run/voice-acceptance-20261103-fixed/round/window-01/summary.json)及[用户补充](../../run/voice-acceptance-20261103-fixed/human-window-01-clarification.json)。

这次无声发生在播放之前：新增两条Response都被取消，新增Delivery为0。第一次在新声音触发暂停约3秒后取消，当时下一段Easy-Turn请求尚未开始；第二次由新的完整输入替换旧回复，事件明确为`superseded_by_new_voice_turn`；第三个turn开始约1.393秒后恰逢窗口Stop，没有获得完整回答时间。前两次取消早于窗口结束，不能归因最终Stop。四次Easy-Turn推理全部成功，耗时分别992／620／660／324ms；一次Juno HTTP200不等于音频已送给Pi。见[同窗SQL汇总](../../run/voice-acceptance-20261103-fixed/sql-window-01/summary.json)、[模型记录](../../run/voice-acceptance-20261103-fixed/log-review-window-01/model-metadata-summary.json)和[取消时序复核](../../run/voice-acceptance-20261103-fixed/output-hold-review/review.md)。

第二窗口保存了准确入模PCM，**指定句已识别正确；测试程序过早Stop和播放worker退出是分别需要记录的问题**。目标片段7.36秒，原判句耗时790ms，文本为“你好艾莉亚请用一句话介绍你自己”；聊天模型生成51字回复。窗口从Start起30秒计时，提示动作记录已晚16.173秒，目标判句结束后仅剩3.945秒；音频输出任务创建约1.8秒后即被Stop取消，offered／accepted为0，本窗没有新声音触发Pause。后续Pi日志另确认窗口期间一次ALSA drop失败使worker退出，不能再把计时Stop视为唯一无声原因，更不能断定最终CaptureStop造成该退出。现有证据不支持归因R1、识别故障或宿舍背景声，也不能把本窗解释套回第一窗口。见[原时间线](../../run/voice-acceptance-20261103-fixed/window-02-review/report.md)与[补充时序复核](../../run/voice-acceptance-20261103-fixed/window-02-review/review.md)。

临时观察只保存了原服务已接收的两段PCM，未增加模型请求或STT；两次保存原PCM分别约0.626／0.426ms，完整保存开销另在日志记录。观察器已关闭，原Easy-Turn启动配置恢复，Pi仍是同一PID4532和原生采集流。音频留在私有目录，不进入公开文档或Git；没有为确认正确文本再运行转写模型。

用户明确要求由现场反馈结束测试，故停止采用20／30秒整场倒计时，也不采用“提示后再等30秒”的替代。流程改为唯一Start→新鲜Ready→提示→用户听到回复或认为等待过久后反馈→精确Stop；EOF／异常／终端退出也走停止逻辑。原模型、RPC和打断期限不改。该流程随后在feedback04实际执行，用户等待约69.875秒仍无回复，结果及播放修复见上节；不能将编排改正当作产品已恢复。完整短句、长句、打断恢复、下一轮对话和日常断电恢复仍待验。

## 免唤醒方案复核与真人窗口（2026-11-03 晚）

本轮软件、现场失败和最终关闭状态的统一索引见[收口记录](../../run/voice-acceptance-20261103-human/summary.json)。

**真人短句尚未通过；先修正免唤醒验收的采集生命周期，新的版本握手失败另行保留。** 用户确认Pi和R1均断电后，本轮按新cold UUID `3b6f378b-bb0a-45f3-91c5-803256b8ad0d`初始化一次，PID2170收到beam0命令ACK。第一窗口Ready后提示，但用户明确没来得及讲话，记为未执行。该窗口的30秒从Start开始；提示后的本地记录写入距Start回执约24.458秒，不能冒充用户界面即时送达证据。第二窗口调整为首次工具等待2秒直接读取Ready，原30秒、5秒RPC和模型期限均不变。没有把第一窗口零模型记录当成识别故障。

两次之间主流程错误地结束整个语音runtime并恢复noop，明确关闭了初始化时的原生UAC采集流（PID2170→noop2620、PCMclosed），然后普通启动PID2897进行第二次真人窗口。用户确认说了指定句但没有回复。虽然普通重启被软件允许、传输Ready有效，历史普通重启仅验证无语音传输，**没有证明重新打开采集流后免唤醒人声仍有效**。因此第二窗口不满足已验证“冷启动初始化一次、同一UAC流持续保持”的声学条件；这是已确认的编排／验收依据偏离，不能进一步认定关闭UAC必然重置R1或它是本次无回复的唯一原因。见[逐证据审阅](../../run/voice-acceptance-20261103-human/r1-scheme-readonly-review.json)和[原启动契约](../../packages/alia-embedded/docs/audio-and-voice.md#r1-system-controlled-capture)。

第二窗口实际有两次Easy-Turn请求，各为4.180／4.020秒PCM，native1582.878／535.268ms，均返回complete；有一次Juno POST HTTP200。它们不证明指定句保真、合成完成或播放可闻。只读SQL新增一条14字符用户消息（指定句精确／规范化比较均false，正文未返回）、一条interrupted Response和unknown audio Delivery，offered／accepted为0、submitted／presented为null；AcousticCandidate Pause取得Held，之后scope退休。旧两unknown保留，未重派。按用户要求，这些仅作旁证，优先核对R1调用方案，不据此认定模型根因。见[模型元数据](../../run/voice-acceptance-20261103-human/log-review-second/model-metadata-summary.json)与[同窗SQL](../../run/voice-acceptance-20261103-human/sql-after-window-second/summary.json)。

当时现场轮采用：依赖就绪→确认真实新冷启动→初始化一次并保持同一runtime／原生采集流→每个窗口仅CaptureControl Start／Stop→整个验收轮结束才停止runtime恢复noop。窗口间Stop停止上传和输出，并不关闭本地原生采集，不能同时向用户声称物理麦克风已关闭。该时点未验证初始化流关闭后的真人质量，所以没有把普通重启Ready计为声学验收通过；这是当时证据范围，不是产品每次普通重启都必须断电或重新发UART初始化的条件。后续播放修复的普通重启入口见本文最新章节。原[连续采集流程](../../run/voice-acceptance-20261103-human/r1-continuous-plan/README.md)作为当时操作记录保留。

用户随后按步骤重新上电，本轮只以新UUID `cf884e24-7c85-43e5-a488-e5cc1293171c`尝试一次，实际在原2秒版本阶段失败：`R1 version response timed out; manual_wakeup was not sent`。没有发出manual_wakeup，没有进入讲话窗口，保留第四份claimed_outcome_unknown记录且不换UUID重试。claim内firmware是UART打开前写入的预期常量，不能当作实读成功；此前口头推断已更正。原握手ACK、同ID一次版本重查机制与cold-03一致，但旧独立工具先开UART再开采集，该次d392401 runtime先开采集和claim后开UART，存在顺序差异；没有本次UART帧证据，尚不能证明该差异导致超时。见[本次失败](../../run/voice-acceptance-20261103-human/r1-continuous-attempt/pi-initialize.json)与[握手对照](../../run/voice-acceptance-20261103-human/r1-continuous-attempt/version-handshake-readonly-review.json)。

失败后恢复d392401全noop PID3810、PCMclosed、硬件FD0，四份claim完整保留；Mac原8481 PID14258运行。用户进一步要求实际修复，已将初始化改为先持久claim／准备UART，再启动音频并校验PCM，随后在同一UART执行原事务，避免晚清空丢掉采集启动期间的握手；原期限与一次manual约束不变。修复身份、离线机制验证和最终安装以[冷启动修复记录](software-release.md#r1-冷启动顺序修复2026-11-03-晚)为准。该修复不证明现场失败已全部解决，也不新增自动冷启动识别。下一次现场验收严格保持同一持续流，不能靠反复断电求成功。最终58bb11e已安装为noop PID4239，签名确认采播关闭，四claims及原绑定保持；软件验证通过，实际新冷启动还未执行。Mac原Server／模型服务的只读就绪检查保持通过。V06上传故障未在本日两个窗口复现，但未关闭；V03／V05、AEC、24小时负载和完整升级恢复仍独立。

## 诊断版无讲话启动核对（2026-11-03）

**独立诊断版已安装，Start→Ready→Stop 通过一次；指定短句仍未执行。** 用户在准备期间表示暂时不能讲话，因此本窗口只检查会话启动，取得新鲜 Ready 后立即进入原签名清理，不发讲话提示、不等待人工反馈。Mac 仍为 `8481b0f`，Pi 为仅增加 Full／Closed 元数据的 `d392401`；没有带入受 V05 限制的 Server 恢复代码，也没有扩大队列、模型／握手期限或重复 R1 初始化。安装前后设备 key、Bound `.local`、SQLite 逻辑行、完整两份 claim 均一致，见[诊断部署](../../run/voice-acceptance-20261103/diagnostic-deploy-plan/deployment-summary.json)。

一次 Start 使用 voice boot `4ff872eb-1642-4da1-baf9-fb82f344aff3`、capture generation 2；取得 session `a245d55a-9ba3-4228-8eec-21eda6aa2207` 的新鲜 Ready，并再次签名 Inspect 核对仍 active。约 **0.681秒**进入清理，以新 Inspect 发一次 Stop，generation 3／inactive 的精确回执成功，Start前至清理结束共 **0.795秒**。原RPC每阶段5秒、零重试保持；复用原window编排，仅跳过讲话提示与人工观察。新增三个fake分支检查通过，不冒充设备验收。见[实际窗口](../../run/voice-acceptance-20261103/diagnostic-readiness-window/summary.json)与[Stop](../../run/voice-acceptance-20261103/diagnostic-readiness-window/stop.json)。

本次日志有一条 Ready，没有 terminal／Full／Closed；Easy-Turn accepted与Juno合成POST均未观察到，现有日志不能精确计数云LLM／embedding请求。无讲话、无听感或完整回复播放结论。本次没有复现V06，**不能以成功替代其根因证据**，更早原始失败继续保留。日志见[有界差分](../../run/voice-acceptance-20261103/diagnostic-log-review/capture/summary.json)。 原5秒期限内的一次只读SQL核对耗时76.676ms，本次Start之后同owner新消息／Response／Delivery／control等均为0，六类账本数量和摘要不变，原两条unknown保持；未读取对话正文或重派，见[本次SQL观察](../../run/voice-acceptance-20261103/sql-after-diagnostic-ready/summary.json)。

之后语音进程PID5788正常退出，诊断件以全noop恢复PID6015，PCMclosed、硬件FD0、disabled／Restart=no；新签名Inspect为generation0、active=false、available=false。两份claim保持，无新增初始化。见[收口](../../run/voice-acceptance-20261103/diagnostic-window-preparation/restore.json)与[最终签名状态](../../run/voice-acceptance-20261103/diagnostic-window-preparation/final-noop-inspect.json)。

两项本轮准备错误单独保留：首次noop Inspect漏传helper本地必需flag，在读key／联网前拒绝；补参数后只做一次实际Inspect。首次语音准备误比较TOML原始结构，noop显式1000／5000ms关闭预算而voice省略相同Rust默认，发生于0stop／0start前；按既有默认归一后继续，未改变有效期限。原错误没有当作产品故障或覆盖。下一轮用[预备入口](../../run/voice-acceptance-20261103/next-human-window/README.md)，先核对当前状态，直接进入一个真人窗口；不要求用户再次选择模型、提供密钥或参考文本。仍必须等实时Ready才提示讲话。

## VoiceSession Ready 前上传终止（2026-11-03）

**本日较早窗口在 Ready 前终止，没有提示讲话；该次收口恢复全 noop。** Mac 依赖恢复与控制 helper 冷编译单独留证，运行版本仍为 `8481b0f`。新 wrapper 只发送一次签名 Start，约 19.762 ms 收到本地 admission 成功，capture generation 2／active=true；这不等于 VoiceSession Ready。随后 observer 记录 terminal-before-ready，原部署错误为 `voice upload queue full or disconnected; session cannot continue`。当前日志把队列已满与接收端已关闭合并，无法确定是哪一类，也不能据此归因网络、设备输入或模型期限；单列 TODO V06，不并入 V04／V05。见[窗口记录](../../run/voice-acceptance-20261103/window/summary.json)、[Start 回执](../../run/voice-acceptance-20261103/window/start.json)与[Ready 前终止](../../run/voice-acceptance-20261103/window/ready.json)。

wrapper 在 Start 前计时约 0.703 秒进入清理，Inspect 已显示同 boot 的 generation 3／active=false，因此按代次边界没有再发 Stop，也没有重试 Start；提示从未发出。该异常路径证明提前终止后的清理判定，不能冒充正常30秒窗口或真人采播验收。原控制RPC 5秒、零重试保持；当时尚无分型诊断；后续独立诊断件及无讲话结果见上节，仍未证明根因修复，见[清理状态](../../run/voice-acceptance-20261103/window/inspect-cleanup.json)。

以 Mac `start.json` 的实际 mtime `2026-11-02T16:37:06.054057Z` 为起点，对本人 owner 做原5秒期限的只读 SQL 观察：消息、canonical Response、Delivery、Delivery event、output-control scope／command 和 conversation event 均为 **0 新增**；六类执行／输出账本行摘要与本轮 Server 恢复后完全相同，旧两条 unknown Delivery 保留。首个诊断查询的 TEXT／UUID 比较错误及原语句保留，按现有 schema 修正后只执行一次成功观察，未改数据。已有 Server／Easy-Turn 日志增量为0字节，Juno仅一条 `GET /tts` HTTP400，不能算合成成功；空日志也不能证明从未有瞬时传输或原生设备活动。见[脱敏 SQL 与源摘要](../../run/voice-acceptance-20261103/sql-after-terminal/summary.json)。

随后恢复原 noop 配置，Pi PID4547、disabled／Restart=no、PCM closed、硬件FD为空，两份初始化claim保持且无初始化override，见[安全收口](../../run/voice-acceptance-20261103/pi-restored-noop.json)。本次不计短句识别、模型回复或播放验收；旧11月2日则是用户明确没有讲话，仍按未执行保留。该时点之后的诊断增量与实际安装见上节，后续通过不自动关闭原故障。

## 受控真人窗口与输出取消（2026-11-02）

**本次指定短句测试未执行，完整语音链仍未验收；窗口结束后 Pi 已恢复全 noop。** 用户在 11 月 3 日明确确认，旧 23:31–23:33 窗口内自己没有讲话；“可能是舍友聊天进入”是用户的声源推测，未由原始 PCM 确认。因此本窗口既不计指定短句识别通过，也不计识别失败，不再等待旧窗口听感作为下一轮前提。见[用户澄清](../../run/voice-acceptance-20261102/human-clarification.json)。此前用户确认板端真正断电后完成一次初始化；现有生产 `8481b0f` 未更换为暂不放行的恢复候选。第一次窗口因只读 observer 无权读取 root-owned 日志而停止，没有提示指定短句；保留原失败，修正为固定路径的只读 sudo 并验证实际可读后才开第二个窗口。第二次在新 SessionReady 与同 boot／capture generation 的 Inspect 后立即提示短句，原模型／RPC 期限未变。见[本轮汇总](../../run/voice-acceptance-20261102/summary.json)及[安全收口](server-embedded.md#真人短句窗口与安全收口2026-11-02)。

第二窗口日志有 17 个不同 Easy-Turn request：7 个 backchannel、5 个 incomplete、5 个 complete，native 最大 **1333.747 ms**，服务 accepted→finished 最大 **1334 ms**；没有观察到本轮 turn-model deadline。它们是一次现场窗口的输入分段，不能当作 17 条独立验收或完整 Rust 请求延迟，也不能证明源声音或识别准确。只读 SQL 的 5 条 user message 均不与指定句精确匹配，去标点／空白归一后也不匹配；用户已确认当时没有讲话，这些文本不是指定句的识别样本，不能据此评价该句的识别能力。没有保存可据此核对声源的原始 PCM。统计见[去重模型观察](../../run/voice-acceptance-20261102/model-window-statistics.json)和[脱敏账本](../../run/voice-acceptance-20261102/sql-analysis/snapshot.json)。

两次生成均进入 `tts_output`，两条 Delivery 最终为 unknown；offered／accepted 都为 0，submitted／presented 为 null，未取得设备媒体提交或完整播放证据。第一输出在 `15:32:29.526Z` 因 AcousticCandidate 被 Pause，随后收到 Held；约 3 秒后取消。当前源码的 `OutputGate` 会取临时语义 hold、普通 hold 与输出总期限的最早值：这与原 **3 秒临时语义 hold** 到期一致，不能误写为 SQL 中 `15:34:27.713Z` 的输出总期限已到。日志的 `original output deadline` 没有标明是哪一种 gate 期限；先前仅凭该文案作出的总期限判断已更正。没有因此扩大期限、关闭打断或重派 unknown 输出。第二次取消与本轮明确的 Stop 同时发生，详见[原身份时间线](../../run/voice-acceptance-20261102/sql-analysis/summary.json)。Juno 的两次请求有 HTTP 200 和合成阶段计时；第一组阶段合计 32.980 秒、第二组 4.088 秒，这不是 HTTP 首包／总延时。Rust adapter 在 headers 后返回惰性流，实际读取仍受 output gate 约束；零媒体计数不等于 Juno 没有合成，当前缺少 Alia 收到首 chunk／EOF 的证据。见[TTS 与 gate 源码核对](../../run/voice-acceptance-20261102/sql-analysis/cancellation-tts-readonly-review.json)。

中间三个 turn（VoiceSession generation epoch 3／4／5）分别出现 `stage=llm, original hold expired`，均未形成 canonical Response。源码确认每个 Respond 创建独立 producer／gate，没有复用已退休 epoch 2 的 hold；错误与各自后续输入的首个保留帧相隔约 3.302／3.302／3.338 秒，包含最多 300 ms pre-roll，支持各自新 AcousticCandidate 的 3 秒 hold 到期。日志缺少逐 hold／触发帧身份，因此精确触发帧仍未证实。`stage=llm` 是外层标签，epoch 3 甚至没有 `llm_call` 记录，不能写成三次模型超时。见[三个新 hold 的有界核查](../../run/voice-acceptance-20261102/sql-analysis/later-turn-holds-readonly-review.json)。

此次准备流程还有明确限制：30 秒是 observer 自己的等待上限，Start 与后续 Stop 分属不同工具调用，第二窗口实际约 **144 秒**；不能写成 30 秒采集已经受控验证。下一次必须让同一有界流程承担到时关闭和异常清理，不能依赖后续人工步骤才 Stop。Pi 时钟比 Mac 快约 **4.117–5.399 秒**，这是 SSH 往返界限，只支持因果顺序与同机时间线，不能跨机按 UTC 做毫秒级排序；本轮没有调时。见[时钟观察](../../run/voice-acceptance-20261102/clock-comparison.json)。

用户要求重来后，11 月 3 日使用独立证据目录和同一进程内定时 Stop／异常清理的 wrapper，实际在 Ready 前因上传终止而收口，结果见[新窗口记录](#voicesession-ready-前上传终止2026-11-03)。不重复断电初始化或恢复已收口的独立硬件测试。正常换气长句、完整打断／恢复、取消下一轮、24 小时混合负载、AEC 和具体插件仍未验收；本窗口也不解释 V05 的软件 Resume 故障。

## Easy-Turn 调度定位与实际修正（2026-11-02）

**已修正已复现的后台调度成本，完整真人模型链仍未通过。** 对同一份官方 3.256 秒 PCM，在两个新进程各自的单 worker 线程中各做一次冷请求：普通前台进程 **1544.967 ms**，`taskpolicy -b` 后台进程 **4583.239 ms**，两者转写 SHA 与 `complete` 标签一致。阶段记录显示音频 encoder 与 Qwen generate 均变慢，支持先修正常驻任务调度分类，而非改模型数学或依赖；这只有一对对照，不是性能分位数或 SLA。见[前台阶段记录](../../run/software-recovery-20261102/easy-turn/worker-foreground.json)、[后台阶段记录](../../run/software-recovery-20261102/easy-turn/worker-background.json)与[本轮汇总](../../run/software-recovery-20261102/easy-turn/summary.json)。10月31日临时 `taskpolicy -B` 未明显改善的原结果与回退仍保留，不能用本次新进程对照改写它。

实际部署仅把 `~/Library/LaunchAgents/com.alia.easy-turn.plist` 与 `~/Library/Application Support/Alia/services/com.alia.easy-turn.plist` 两份相同文件的 **`ProcessType: Background → Interactive`**，正常停止原 PID 3949 后重新加载，实际新 PID **41472**，就绪 **4.704秒**，launchd 分类回读为 Interactive。两份新 plist 的 SHA-256 均为 `946ad67af9971d7962c0037b8a231088920fcaa55571151658f27519ae644bd3`；[变更及回退计划](../../run/software-recovery-20261102/easy-turn/scheduling-plan.json)、[实际生效记录](../../run/software-recovery-20261102/easy-turn/scheduling-activation.json)分别保存。此次没有改变 Python 服务实现、依赖、权重、CPU／FP32、18线程、prompt、生成参数、单请求并发或任何期限，也没有预热请求。

随后通过当前 Rust Easy-Turn adapter 各发送一次固定 PCM：重启后的第一个真实请求 `complete` **1535.027 ms**，后续 `backchannel` **383.876 ms**、`wait` **436.418 ms**，三次均在原 **5000 ms** 客户端期限内通过。Python 原60秒 native响应期限仍保留；没有裁短输入／输出、扩大期限、重试求绿或启用麦克风／播放。它们证明本次调度配置下的三个组件样本，不证明10月31日两个现场未知输入的根因或生产长句已解决；`incomplete`、正常换气长句和真人输入仍待验，升级恢复、24小时混合负载与AEC继续独立。

旧源码包与发布包仍对应各自提交和清单。10月25日 `6123691` 源码归档不包含后来的10月31日诊断变更；本次11月2日用户域 plist 调整另由部署证据固定，不能把旧源码归档当作当前主机全部配置身份，也不重写旧失败或累计历史样本数量。真人窗口恢复还须处理当轮单独记录的网络／数据库条件，不能仅凭此处组件成功重新启用采播。

## 真人窗口失败与 Easy-Turn 诊断补充（2026-10-31）

**完整真人模型链仍未通过。** 本次生产初始化后的真人窗口中，两个 VoiceSession 分别在 `08:21:51.192Z`、`08:27:02.896Z` 以 `DeadlineExceeded: turn model deadline` 结束，保留原 Rust **5000 ms**、并发1。首会话尚未提示测试句；第二会话也早于 `08:27:37.607Z` 的短句提示记录结束。两个期限故障是真实故障，但指定短句未取得有效完整测试，不能推断该语句已进入模型。用户未收到回复，现有日志没有可核对的转写／Response／Delivery；触发声音及实际PCM窗口长度仍未知。见[首次会话分析](../../run/voice-acceptance-20261031/mac-log-deltas/startup-first-session/analysis.json)、[Pi 两次会话原始日志](../../run/voice-acceptance-20261031/pi-short-observation.log)及[短句阶段](../../run/voice-acceptance-20261031/short-human-stage.json)。现场随后收口，Pi恢复已配对的全noop配置，PCM关闭、初始化claim保留，见[终态](../../run/voice-acceptance-20261031/pi-restored-noop.json)。

同一固定CPU服务的官方known fixture实际 **4021.590 ms** 返回 `complete`，在原5秒内；诊断命令误传期望值 `Complete`，与协议小写不符，故原exit1仍保留，另做[结果解释](../../run/voice-acceptance-20261031/easy-turn-known-sample-assessment.json)，没有为改判定重复推理。一次临时QoS假设检查的同样本为 **3966.222 ms**，不足以归因或认定修复；[原结果](../../run/voice-acceptance-20261031/easy-turn-qos-sample.json)与[回退记录](../../run/voice-acceptance-20261031/qos-rollback.json)分别保留，未改服务plist或模型参数。这些组件样本不能替代失败的真人窗口，也不关闭历史V01／V02。

为补齐下次定位依据，仅在现有[Easy-Turn服务](../../services/easy-turn/src/alia_easy_turn/app.py)增加接受请求、native开始／完成及服务响应超时的元数据日志：已校验的request ID／epoch、UTC毫秒、PCM字节数／音频时长、线程派发等待、native耗时，以及合法turn state或异常类别。日志不记录音频、转写、prompt、密钥、异常正文或traceback；复用Uvicorn既有INFO logger，不全局开启第三方verbose。Rust调用协议、原5秒期限、Python原60秒响应期限、模型、线程和并发均未改变。原 `asyncio.shield` 及native结束才释放单槽的逻辑保持；`response_timeout`仅代表Python服务自身期限，不是Rust调用方5秒超时或断开的证明。

现有fake-detector服务测试 **14 passed / 0 failed / 0 skipped**，补充日志隐私、native晚完成和超时后占槽断言；独立默认logger检查确认本服务INFO可见、第三方INFO／DEBUG不输出。见[诊断变更验证](../../run/voice-acceptance-20261031/easy-turn-diagnostics/summary.json)。这不是神经推理性能修复，也没有为该变更重跑已通过的Rust workspace。

诊断源码已通过一次仅针对Easy-Turn的受控重启激活：旧PID879完成graceful shutdown后，已安装Uvicorn恢复并重发SIGTERM，记录signal15。首个编排器误要求数字exit0而停止，保留[原始失败](../../run/voice-acceptance-20261031/easy-turn-diagnostics/live-activation.json)；核对Uvicorn源码行为及8091端口关闭后，才执行一次kickstart。新PID3949在 **18.162秒** 就绪，固定模型身份、plist、配置及期限不变，见[实际激活记录](../../run/voice-acceptance-20261031/easy-turn-diagnostics/live-activation-resumed.json)。

随后唯一一次冷启动后官方固定样本为 **104192bytes／3.256秒PCM**，Rust调用在 **5026.030ms** 以原5秒期限失败，没有重试或warmup求绿。新日志记录该请求的接受、native开始及迟到完成：线程派发 **0.749ms**，native阶段 **5960.658ms** 后返回 `complete`。这确认**此样本**主要耗时在native阶段而非排队，且调用方超时后原推理仍继续；与native结束后才释放单槽的既有实现一致，但未另外探测槽位。该结果不能反推前两次现场的音频内容或故障根因。记录中的 `passed=true` 只代表元数据诊断已激活，**`probe_passed=false`** 是实际样本结果，晚到的 `complete` 不能改写为原5秒内成功。本轮不再调用模型或采播；后续仍需解决真实窗口失败，再验讲话→模型→回复播放→打断／恢复→取消下一轮，AEC、升级恢复、24小时负载和实际MCP／插件保持独立门槛。

## Mac 模型服务正式启用（2026-10-28）

用户已授权本机正式启用，使用 `ke-qiao` 登录会话。Easy-Turn／独占 Juno／Server三个用户LaunchAgent已安装到 `gui/501`；私有有效配置、Juno运行YAML和日志位于 `Library/Application Support/Alia`／`Library/Logs/Alia`，原准备配置与组件证据保留。首次启动使用冻结候选 `6123691`，实际健康检查发现下述TTS探测缺陷后，已切到最小修复版 `8481b0f` 并完成同身份重启；本轮结束时Mac服务保持运行，不增加provider框架。实际启停命令、身份和后续条件见[Mac正式启用](server-embedded.md#mac-正式启用2026-10-28)。

本轮模型就绪检查已在实际安装下通过：Magpie目录包含官方 `deepseek/deepseek-flash`；LM Studio精确加载 `text-embedding-qwen3-embedding-0.6b`、context8192并应用回环／关闭敏感和verbose日志／登录服务策略；Easy-Turn8091核对固定CPU身份与单worker能力；Juno19880核对专属LaunchAgent、PID、监听、argv、运行YAML与固定磁盘权重。原共享9880实例保留，未改全局JIT或驱逐策略；模型资产仍依赖已固定的项目／GPT-SoVITS路径，不能清理为临时run文件。见[实际就绪结果](../../run/mac-activation-20261028/model-readiness-first.json)。

这些检查没有发送模型推理，不重复此前已通过的新minor、embedding、Easy-Turn和Alia TTS adapter样本。目录与GET健康不证明DeepSeek上游当时可达、模型长期驻留、驻留tensor与磁盘字节相同或业务质量；同一角色预算、路由5秒、记忆3秒、Easy-Turn客户端5秒、TTS总60秒／idle10秒保持。真实Server首启、本人管理员、shadow、33项迁移和本地Qwen双pin已核对；原Server key保留，详见[生产记录](server-embedded.md#mac-正式启用2026-10-28)。

本轮真实 `/health` 又发现独立的探测缺陷：原GPT-SoVITS检查GET `/tts`却不提供语言参数，引发上游500（`None.lower`）。这不是此前TTS合成样本失败。最小修复只补当前voice语言参数，仍用空text且不发送reference／prompt，在参数校验返回400，不启动合成；原5秒期限、权重身份门槛和500拒绝保持。双平台定向验证、发布构建及Mac实际重启通过，最终TTS／embedding为healthy、没有unhealthy模块；本人账号、SQL双pin和shadow保持。首错与正常退出记录保留，见[发布记录](software-release.md#mac-正式启用与健康检查修复2026-10-28)。整体degraded仅为3项LLM在线probe按配置关闭，不将它解释成云端失败或为健康全绿自动开启付费请求。

当前 `channels=[]` 没有完整纯文本HTTP会话入口；`/api/v1/voice/chat` 即使只输入文字也会调用TTS，且不经过Interaction／路由／Execution或canonical memory。它不能代替无音频的本人会话验收，本轮不为这个检查扩展功能。下一步先准备Pi的无音频配置／身份核对，再通知用户进入真人整链窗口；换气长句、系统Start／Stop、生产初始化后真人讲话、取消下一轮、升级恢复与24小时故障负载仍未验。AEC及首版实际MCP／第三方插件保持独立门槛。

## 当前决定与 adapter 核查（2026-10-28）

用户已将 **minor 改为 Magpie 官方 DeepSeek Flash Instant**。本机目录没有独立 Instant ID，实际固定 `deepseek/deepseek-flash`、`chat_completions`、`provider_specific.reasoning_effort="none"`；不用 `auto/` 或第三方同名路由。major 原本也是该完整路由与 none，两角色仍保留独立 profile、1024／4096 输出预算，`default` 复用 major；省略 `memory_capture` 使用 minor fallback，`sentence_expression` 继续确定性规则。旧本地 Instant 不再是 Alia 启动依赖，既有资产／实例未因此删除或卸载。本地 Qwen embedding／1024、CPU Easy-Turn、独占 Juno、参考文本及 `ke-qiao` 登录会话保持。路由分类与记忆提取现在也调用 DeepSeek；本地 embedding 不代表全部会话／记忆内容只在本机。

当前已经使用纯 Rust 库：`LlmService` 复用锁定的 Rig 0.44.0 fork `c2ff3e30e6b8d84fc6759eab030656a42cae14ca`，无需增加 adapter 框架。以下是[实际构造代码](../../packages/alia-server/src/providers/llm/service.rs)已接通的聊天协议，不是对任意远端模型的兼容承诺。

| 协议 | 现有配置与实现 | 边界 |
| --- | --- | --- |
| OpenAI Chat Completions | `type="openai"`、`api_family="chat_completions"` → Rig `.chat()` | 官方或兼容端点均须核对实际模型、流、工具与 JSON 行为 |
| OpenAI Responses | `type="openai"`、`api_family="responses"` → Rig `.responses()` | 端点必须实际提供 Responses，不能仅凭 OpenAI-compatible 标签选用 |
| Anthropic Messages | `type="anthropic"` → Rig 原生 Messages | API version 可配置；结构化输出需 `json_schema`，单独 `json_mode=true` 会拒绝 |
| Gemini | `type="gemini"` → Rig 原生 Gemini | 已处理 `/v1beta` 与 JSON MIME；具体模型能力仍需组件验证 |

四路默认构建启用，[现有 HTTP fixture](../../packages/alia-server/src/providers/llm/service/http_tests.rs)覆盖流终止、工具参数／身份、预算、取消和协议请求格式，已包含在软件候选回归中。协议核查本身只读，未重跑回归或发起模型请求；随后新 minor在原路由5秒、记忆捕获3秒内完成下节有限样本，不放宽预算。原生工具继续进入Execution。若经Magpie的Chat兼容入口访问其他厂商模型，Alia一侧仍选择 `openai`／`chat_completions`。

用户已澄清“system One”为口误并撤回，不列为待确认项或新增适配。Embedding 与聊天协议分开：当前真实 [embedding factory](../../packages/alia-server/src/bootstrap/factory.rs)仅接 OpenAI-compatible `/embeddings`，会拒绝 Anthropic／Gemini embedding provider；已选本地 Qwen 路径不变。

## 新 minor 与新库准备验证（2026-10-28）

[本轮汇总](../../run/fresh-start-20261028/summary.json)分别固定配置、有限模型调用、隔离新库与启动材料证据，不合并为生产整链通过。

[准备阶段独立配置](../../run/fresh-start-20261028/config/server.production.pending.toml)、[组件配置](../../run/fresh-start-20261028/config/server.components.toml)均经原 Rust loader／factory 检查；运行软件仍为 `6123691`，没有改生产代码或依赖。pending文件保留 DB port 1、空 JWT、无 channel／discovery，不能直接启动生产；实际安装另用私有有效配置，不覆盖这份准备证据。[Magpie 目录证据](../../run/fresh-start-20261028/config/magpie-deepseek-catalog-v2.json)支持官方路由和 none 参数；目录标签不能固定可变供应商 alias 的权重版本，也不把其他原生 endpoint 标签当作本轮实测。

使用原 Rust `LlmService` 诊断和原提示词，各一次新 minor 请求通过：路由分类 **788.167ms／原 5 秒**，瞬时问候的记忆捕获 **854.765ms／原 3 秒**，得到期望的 JSON；[完整结果](../../run/fresh-start-20261028/minor/summary.json)保留原始响应，没有重试。两个样本不代表业务语料准确率、并发／长上下文／断网表现或 p95；记忆样本只覆盖不应写入的瞬时问候。

核对本机 provider 元数据时发生一次凭据输出失误：DeepSeek key 曾误入工具结果和新建本地 JSON；本地 JSON 已脱敏，未提交或修改原凭据，旧工具输出无法撤回。用户已确认在 Magpie 更新 key，之后才执行上述请求；没有读取或记录新 key。仅保留[不含原值的事件记录](../../run/fresh-start-20261028/config/credential-handling-incident.json)。

此前只通过直接 HTTP 的 Juno 本轮已补齐 **Alia `create_tts`／`GptSovitsTts.init`／stream adapter**：独占19880，固定权重和文件名参考文本，一次真实合成通过；init **484.084ms**、首块 **5892.716ms**、全流 **6773.710ms**，7块／304684bytes，结束后编码预算为0。原总60秒／idle10秒不变，没有第二次求绿、播放或试听；这仍不是 Delivery／Pi／声学整链。原始流和[adapter结果](../../run/fresh-start-20261028/juno/adapter-result.json)保留，临时诊断代码不进入生产实现。任务19880已停止、原9880和源码／权重／参考文件保持；关闭时上游 `resource_tracker` 报告1个待清理semaphore，原警告保留，未测OS资源上界或通过重跑抹去。

新的 [Mac 服务材料](../../run/fresh-start-20261028/services/README.md)只要求 LM Studio 的选定 embedding 驻留，不再要求旧本地聊天实例或强制两模型 JIT 策略；准备时敏感日志／登录设置尚未应用。15 项 ready fixture、3 份 plist、最终13项 Server gate／凭据FILE／exec拦截检查和停服日志轮转检查分别留证；它们不等于实际安装或运行中日志有硬上界。随后正式应用及实际就绪见本页首节；独立新空库首次启动／同身份重启的原隔离结果仍见[新库入口](upgrade-recovery.md#新生产数据来源决定2026-10-28)，不与生产结果混用。

## 历史模型选择与输入清单（2026-10-25）

以下 2026-10-25 各节保留当日决定、配置和验证证据；minor 选择以 2026-10-28 为准。旧配置不可直接作为当前生产配置。用户已撤回使用两个旧 PG 数据源／备份的方案，下面从旧库隔离副本核对身份的建议不再是执行入口。

用户已选择 Magpie 的本地 Qwen3.8 27B 作为 minor、DeepSeek Flash 作为 major，并授权其余按推荐处理。minor 的标准高思考版与 Instant 由业务需求选择：采用 **Instant、关闭思考**，适配路由／分类原 5 秒和记忆捕获原 3 秒期限；复杂任务沿用 major 升级路径。512K 是该服务的上下文配置，不提高 Alia 的上下文、输出、并发或期限预算。本节替代先前的模型待选清单；软件候选 `6123691516fae1c2b0e813652eaa1104c33da9af` 及归档保持独立。

选型阶段只读核对了 Magpie 0.1.1436／LM Studio 目录和资产。随后用户授权下载、独立配置与有限 Mac 组件预检，结果见下节；随后完成独立配置与合成数据库 profile 切换／恢复演练，见[升级恢复](upgrade-recovery.md#选定-embedding-的切换与隔离恢复2026-10-25)。没有切换生产配置或 SQL，没有接触 Pi／麦克风／播放。目录、组件、隔离数据与生产整链证据分别记录。

| 项目 | 已确认 | 历史候选／已盘点资产 | 下一轮待提供或核对 |
| --- | --- | --- | --- |
| 聊天与角色 | `minor` → `lmstudio/local/qwen3.8-27b-512k-instant`；`major` → `deepseek/deepseek-flash`；`default` 绑定同一个 major profile；省略 `memory_capture` 使用 minor fallback，省略 `sentence_expression` 使用确定性规则 | 两个路由均在 Magpie 目录；Instant 为 MLX 4bit，已存在的实例报告 context 524288／parallel 1／默认思考 off。DeepSeek 目录标签为 V4.1 Flash 1017 | 共用 `http://127.0.0.1:3425/v1`、`chat_completions`；两 profile 的 `provider_specific.reasoning_effort="none"`，保留原输出预算。本轮已通过有限 JSON／native tool stream／本地取消检查与原期限；下一轮扩大到实际业务和 Execution 整链。供应商 alias 与目录标签不能代替不可变权重版本 |
| Embedding | **本地 Qwen3-Embedding-0.6B、GGUF Q8_0、1024 维**，复用 LM Studio `http://localhost:1234/v1/embeddings`，不增加 Alia adapter 或模型框架 | LM Studio 内置 Nomic v1.5 Q4_K_M／768 权重存在、完整 SHA 已核对、目录显示未加载；Qwen embedding 已按固定 revision 下载、校验并导入，实际 ID 为 `text-embedding-qwen3-embedding-0.6b`。旧 OpenAI／1536 和 Nomic／768 配置不作为本轮有效选择 | 1024 维、2 次 HTTP 与 1 次 Rust adapter 请求已通过，中文小样本 top-1 为 3/3；真实记忆语料与并发负载仍未验。生产前核对 SQL active profile，保留 canonical memory，显式切换并重建投影。云端 `text-embedding-v4/1024` 仅为实测不满足需求时的备选，不自动切换或混用向量 |
| Easy-Turn | **复用项目内固定 CPU 栈**：`services/easy-turn/.venv`、`run/models/easy-turn-source`、`run/models/easy-turn/checkpoint.pt`、`run/models/qwen2.5-0.5b-instruct`；生产候选 loopback 8091、本轮隔离端口 18091、1 worker、并发 1 | source `fe175eaa…` 干净；66 项包版本元数据匹配；checkpoint 及 Qwen 所需 7 文件完整 SHA 匹配。外部 `Developer/Easy-Turn` 副本有修改／环境差异，不选用 | 独立 CPU 启动及三份官方 PCM 经 Rust adapter 已通过原 5 秒期限；真人语音和并发负载仍待验。MPS 不是当前固定栈已通过路径 |
| GPT-SoVITS／Juno | **Alia 独占 loopback 实例**，CPU／FP32／v2ProPlus；复用 `Juno-e30.ckpt`＋`v2Pro/Juno_e16_s80.pth` 及 BERT／HuBERT／SV 基础资产 | 两权重 SHA 分别为 `b8830efd…a01c94`、`050dba36…76d`；现有 Python 3.10.17、torch 2.12.0、torchaudio 2.11.0 的导入／算子／两次实际合成通过；最终参考为 `我一直很好奇我们的基地怎么会和星际旅程失联.wav`，4.4787 秒／32kHz／mono／PCM16，文本按用户确认直接取文件名 | 独立 `127.0.0.1:19880/tts` 已完成两次仅落盘合成，权重路径与前后 SHA 固定；下一轮做音色／可懂度和真实播放验收。保留依赖与训练键警告，不与其他客户端共享可切权重实例 |
| 独立 Whisper | **首版 VoiceSession 不启用**；直接使用 Easy-Turn transcript | 既有 whisper.cpp／large-v3-turbo 资产保留 | 以后确有独立 HTTP 音频转写需求时单列启用与验收，不给当前 VoiceSession 增加重复 STT |
| 调用与素材范围 | 先有限组件检查，再生产整链；默认 Companion shadow 保持；既有离线 PCM 和 R1 硬件基线复用 | Magpie 已配置所选上游，LM Studio 目录认证可用；凭据仅在内存／子进程环境中用于本机接口请求，未输出或归档原值 | 本轮有限启动和固定非敏感样本调用已获授权并完成，自建实例已停止。后续生产窗口另按当前授权进入；major 的必要上下文仍会送往 DeepSeek，本地 embedding 不代表全部记忆／会话内容都只留本机 |

当前选择与只读证据在 [selected-plan.json](../../run/model-plan-20261025/selected-plan.json)、[Magpie 目录](../../run/model-plan-20261025/selected-chat-catalog.json)、[LM Studio 目录](../../run/model-plan-20261025/lmstudio-models-readonly.json)、[Instant 资产](../../run/model-plan-20261025/instant-assets.json)、[Nomic 资产](../../run/model-plan-20261025/nomic-bundled-asset.json)、[Easy-Turn 资产](../../run/model-plan-20261025/easy-turn-readonly-inventory.json)和[Juno 资产](../../run/model-plan-20261025/gpt-sovits-juno-assets.json)。先前推荐、[历史交接](../refactor/handoffs/2026-10-17-model-preflight/acceptance-HANDOFF.md)及冲突配置保留为历史，不覆盖本次决定。

Instant 的本地 hub 清单实际指向 `orcarouter/Qwen3.8-27B-Uncensored-MLX-512K-Text`，不是标准 `qwen/qwen3.8-27b` 仅切换思考参数。已记录 12 个文件的 SHA，包括约 16.05 GB 的三个权重分片、config／tokenizer／chat template 与 hub 映射；没有可据以认定的上游 revision。当前固定的是磁盘资产，不能证明已驻留实例仍使用相同字节。本轮已验各一条路由／记忆 JSON 样本，下一轮仍须扩大业务语料并验真实工具链；Execution／授权边界保持原样。

本地 embedding 选择依据是中文／多语言定位、较小的共存开销和现有接口适配，不是 Alia 实测优于云端。[Qwen 官方模型卡](https://huggingface.co/Qwen/Qwen3-Embedding-0.6B-GGUF)列出 0.6B／1024 维和 Q8_0；已固定 revision `370f27d7550e0def9b39c1f16d3fbaa13aa67728`、文件 `Qwen3-Embedding-0.6B-Q8_0.gguf`、SHA-256 `06507c7b42688469c4e7298b0a1e16deff06caf291cf0a5b278c308249c3e439`。Alia 目前不区分 query/document 指令，先按现有 plain input 验中文检索，不为选型增加 reranker 或协议。[Nomic 官方模型卡](https://huggingface.co/nomic-ai/nomic-embed-text-v1.5)的英语定位和任务前缀要求与当前契约不同，保留其可复用资产，不据此断言中文不可用。

[Magpie 接入文档](https://usemagpie.ai/docs/integrate)支持现有 OpenAI-compatible 聊天入口；本机 `magpie-alia` 只是调用方标识，不是供应商密钥。使用完整 provider/model 路由，避免 `auto/` 或裸模型名带来上游歧义。Embedding 直接走 [LM Studio embeddings](https://lmstudio.ai/docs/developer/openai-compat/embeddings)，不能由 Magpie 聊天目录推定其转发 embeddings。LM Studio 的真实 credential 留在受控来源。本轮独立配置将该适配器命名为 `openai` 并显式指向本地 LM Studio，以复用现有 `OPENAI_API_KEY` 补空机制；运行器仅在子进程环境填入现有 token，不写入配置或日志。其他命名 provider 不会自动读取任意同名环境变量。

已生成[独立组件配置](../../run/model-preflight-20261025/config/server.components.toml)，由当前 Rust `ServerConfig` 和 adapter factory 实际加载通过，启用真实模型但没有 channels／discovery／后台计划任务；旧可选角色绑定未带入。配置目录没有 `.d` 或 `settings.toml`，子进程清除继承的 ALIA／数据库／模型密钥覆盖，仅注入本地 embedding token。数据库故意指向 loopback:1，**本文件用于直接组件检查，不能启动生产 Server**；未读取或变更生产 SQL policy。先前[选型片段](../../run/model-plan-20261025/model-sections.toml)保留为历史。生产组装仍须核对 base／`.d`／环境／settings／SQL 的有效值，省略可选角色不会删除深合并中的旧值。

当前资产、参考文本、有限组件预检和独立生产候选配置已具备；SQL 切换／Memory 投影重建与前后数据还原已在合成隔离库验证。待部署配置仍关闭入口，真实生产数据库、账号／设备／网络身份和服务账户托管尚须核对，不能直接启动。下一步填实这些绑定、取得一致备份并安排现场窗口，然后在 Mac Server＋Pi Embedded 上集中验收：**生产启动 → 讲话 → 模型处理 → 回复播放 → 打断／恢复 → 取消与下一轮对话**。该轮包含正常换气长句、系统 Start／Stop、生产单次初始化后的真人语音；复用已通过的插拔、方向拾音、试听和静默恢复基线，不把 fixture 音频算成真人语音通过。

实际部署后另做升级／恢复及至少 24 小时混合负载，覆盖断网、模型超时、进程退出、设备重连、内存／线程／句柄／队列与日志磁盘上界。验收前固定记录上界与观察方式，不以放宽原期限或并发求绿。AEC 等商家确认后独立验证；具体首版 MCP 服务和第三方插件按实际目标验证，通用框架回归不代替它们。V01／V02 历史根因继续开放，扩大无人值守主动运行前再处理对应条件；ROS、执行器现场、固件重写和其他平台不因模型接入自动恢复。

## Mac 有限组件预检（2026-10-25）

源码基线 `0238620`（软件实现仍为 `6123691`）；使用临时 Rust 诊断调用当前 `LlmService`／Rig、`OpenAiEmbedding`、`EasyTurnDetector`，没有启动 Alia Server、Execution 工具或数据库。临时 example 已移除，诊断源码／二进制摘要和准确提示词保存在 [rust 证据](../../run/model-preflight-20261025/rust/diagnostic-build.json)。组件原始响应、请求预算、独立配置、身份与清理见 [summary.json](../../run/model-preflight-20261025/summary.json)。

| 检查 | 本轮结果与实际耗时 | 保留的期限与边界 |
| --- | --- | --- |
| minor 路由分类／记忆捕获，Rust `LlmService` | 各 1 次通过；4.175 秒／1.040 秒，合法 JSON，问候路由正确、瞬时对话未产生记忆候选 | 原 5 秒／3 秒；路由只剩约 0.825 秒余量，不能外推长上下文、忙时或稳定 p95 |
| major 原生工具流／本地取消，Rust＋Rig | 各 1 次通过；0.627 秒完整得到正确工具参数／0.384 秒收到首个文本后停止本地读取 | 原 60 秒；没有执行工具，不证明远端推理已停止，也不代替真实 Execution 整链 |
| embedding | HTTP 两次通过（0.231 秒／0.017 秒），另一次生产 Rust adapter 通过；全部 1024 维、有限数值、模型身份与序号匹配；固定中文小样本 top-1 为 3/3 | 原 30 秒；单样本/小语料，不是生产检索质量、SQL profile 切换或重建验收 |
| Easy-Turn CPU＋Rust adapter | 独立启动 10.11 秒；Complete／Backchannel／WAIT 三份官方 PCM 各通过，1.961／0.380／0.440 秒 | Alia 原 5000ms、并发 1；服务原生 worker 上限仍为 60 秒，不把它冒充客户端期限；未测真人或全部语义类别 |
| Juno CPU／FP32，直接 HTTP 合成 | 独立启动 8.10 秒；两份输出分别 6.472／1.223 秒，首个响应体 6.346／0.584 秒；有效 32k mono PCM16、非静音、无满幅裁剪 | 原总 60 秒／流空闲 10 秒；仅保存 WAV，没有播放、试听或音色／可懂度判断；未通过 Alia TTS adapter 发起 |

Qwen embedding 文件为 639,150,592 bytes，SHA 与所选 revision 一致；LM Studio 通过符号链接导入，进程映射到固定 GGUF，实际 model ID 已写入配置。原件位于 `run/models/qwen3-embedding-0.6b-gguf-370f27d/`，清理运行证据时须保留这个模型目录。Juno runtime 的 torch／torchaudio 导入、重采样和 MelSpectrogram 通过，没有安装或升级依赖。

保留的失败与警告：诊断程序初版有一处 Rust enum 引用类型错误，在任何模型调用前修复；第一份独立配置缺少必需 Insights 字段，被 serde 拒绝，原配置和失败仍在，补齐现有字段且关闭后台任务后 v2 加载通过。上述均不是生产模型请求失败。Juno 原有 RequestsDependencyWarning 和 103 个 `enc_q.*` 缺失键保留，unexpected keys 为空；源码中这些键属于训练 posterior encoder，当前 infer/decode 不使用，没有改加载行为或隐藏警告。Easy-Turn 的可选 NPU 导入提示也保留，不为 CPU 预检安装 NPU 依赖。

参考素材遵循用户最新说明，文件名就是文本。先前选中的 `audio (3).wav` 已被替代；用户纠正前已完成一次本地 Whisper 文件转写，原始记录保留且不用于最终配置，之后没有重跑。最初读取其完整 token JSON 遇到无效 UTF-8 的解析错误也保留，不当作 Whisper 推理失败或参考文本依据。首版 VoiceSession 仍没有独立 Whisper。

所有调用使用非敏感合成文本或既有离线音频，推理阶段顺序执行；同机模型驻留与资源快照不等于并发负载或资源上界验收。本轮自建 Easy-Turn 18091、Juno 19880 已停止，仅卸载本轮新增 embedding 实例；原 Magpie／Instant 和 9880 上既有 TTS 保留。没有麦克风、播放、UART、Pi 部署或生产数据库写入。V01／V02、Companion shadow、AEC 和长稳门槛不因这些组件结果改变。

## 生产配置与隔离 SQL 出口（2026-10-25）

[待部署单文件配置](../../run/production-config-20261025/config/server.production.pending.toml)和[隔离配置](../../run/production-config-20261025/config/server.isolated.toml)经实际 Rust loader、SettingsRegistry 和 factory／profile preview 通过。采用已确认模型／原期限，关闭未验的后台计划、Insights 与自动 motion，Companion 仍以 SQL shadow 核对为准；无具体首版插件接入。配置不是已部署状态，数据库 port 1、空 JWT 和 disabled channel 保留到真实绑定落实。

本轮复现旧 settings 会把本地 Qwen／1024 覆盖成 OpenAI／1536，因此新目录不继承冲突层。选定 profile 为 `alia.embedding.d07c6780f61ebf58ba106d7e1cee0a5cfd4405ba6e2c4c62f04690811978aa19`，endpoint 精确为 `http://localhost:1234/v1`。正常 Settings 保存会落盘环境注入的 token；生产 `settings.toml` 必须作为私有凭据文件管理，不能因主 TOML 无 key 就视作无 secret。

[隔离结果](../../run/production-config-20261025/summary.json)覆盖新配置不覆盖旧 SQL pin、stale CAS 409、合法 profile 切换、3 条合成记忆重建、同维 endpoint 身份隔离与显式回切、缺失投影重启修复，以及切换前后各 67 表的实际备份／还原核对。旧 1536 维为 fixture，新 1024 维调用固定本地模型；canonical 内容不变，private_runtime 无投影。Cache 没有旧 active 数据的自动 backfill，不能合称所有向量重建完成。

操作 payload、只读 SQL、原期限、部分成功判定和前向回切／成套恢复边界集中在[升级恢复入口](upgrade-recovery.md#选定-embedding-的切换与隔离恢复2026-10-25)。旧配置指向的 localhost:5432 当前拒绝连接；本轮未读生产 SQL，也没有证明真实账号／绑定、Companion 状态或生产数据量。恢复副本仅只读核验，主隔离库验证重启补建；没有部署或启用音频。

进入真人窗口前还须落实：真实库和原身份／备份；Mac 服务账户及模型启动顺序（Magpie／LM Studio embedding → CPU Easy-Turn／独占 Juno → Server）；Pi 可达 endpoint 与设备 pin／owner；随后成套部署并验实际 Alia TTS adapter、讲话／模型／播放／打断恢复／取消下一轮、换气长句和 Start／Stop。此前 Juno 只验直接 HTTP。实际升级恢复、24 小时混合负载、AEC 和具体插件仍保留独立门槛。

## Mac 常驻材料与本机盘点补充（2026-10-25）

只读核查确认现有模型资产与服务归当前 `ke-qiao` 用户。当前 Magpie／LM Studio 可达、Instant 已加载；embedding 未加载，独占 Juno 19880 和 Easy-Turn 8091 尚未运行。现有共享 Juno 9880 保留。已准备用户 LaunchAgent、独立 Juno 运行 YAML、无推理／无加载的就绪检查和 Server 启动前检查；本轮没有安装或启动这些材料。目录／健康检查不替代真实调用、权重驻留字节或生产整链验收。

详见[服务材料](../../run/mac-production-readiness-20261025/services/README.md)和[本机数据／常驻方式](server-embedded.md#本机数据与常驻准备盘点2026-10-25)。首轮建议当前用户登录会话：模型先就绪，Server 显式启动且不自动重试；无人登录开机方案尚未实施。LM Studio 敏感请求日志当前开启，关闭日志和显式预加载的候选设置只准备、未应用。两套既有 PG 都停止，Rust 卷映射5433、历史Python卷5432；生产账号与SQL身份应从停服数据的隔离副本核对，不新建空库绕过。

## Embedded Models

Embedded keeps local audio intelligence in the existing Rust host:

- sherpa-onnx for local wake word keyword spotting, or R1 firmware wake events
- Silero ONNX through Rust `ort` for VAD activity evidence on the continuous capture stream

Model paths are configured in `config/embedded.toml`. The helper script
`scripts/download-sherpa-models.sh` populates the KWS and Silero VAD model
directories. Embedded does not run local STT or make semantic turn decisions.
Its bounded VAD endpoint evidence is interpreted by the Server's existing voice pipeline.

Embedded audio/model behavior is described in
[alia-embedded audio and voice](../../packages/alia-embedded/docs/audio-and-voice.md).

## Server Turn Detection

Easy-Turn runs only on the Server host in the current macOS Server + Pi 5
Embedded deployment. Embedded keeps wake word, VAD, continuous capture and
playback; it does not host this model. Enable it in
`config/server.d/providers/easy_turn.toml`:

```toml
[providers.easy_turn]
enabled = true
endpoint = "http://127.0.0.1:8091"
timeout_ms = 5000
max_concurrency = 1
```

There is no Embedded `speech.enabled` switch. For a later authorized device
deployment, select the capture/playback, wake-word and VAD modules, configure
`speech.channel_id`, and satisfy the normal binding/session requirements. Keep
the all-noop staging profile for preparation without devices; choosing real
audio modules can open devices during startup. Continuous VoiceSession is the
only device voice path. Preparation, pinned
revisions and the service HTTP
contract are documented in the [Easy-Turn service guide](../../services/easy-turn/README.md).
`/health` confirms readiness and `/v1/capabilities` reports the actual model,
format and concurrency limits. Readiness alone does not measure first-request
latency; record initial and subsequent calls separately when assessing load.

Server uses bounded PCM windows to obtain a transcript and Complete,
Incomplete, Backchannel or WAIT evidence. This is continuous capture with
windowed inference, not a model-native incremental decoder. The accepted model
transcript goes directly into the recognized voice input
path and is not sent through a second STT request.

Use matching voice v2 and playback v3 peers. A fresh Backchannel may release a
temporary acoustic hold; explicit WAIT retains the same output until explicit
Continue. The 3-second temporary semantic deadline, configurable 60-second
hold default (hard maximum 120 seconds), and original output lifetime are
separate from `timeout_ms`. Increasing a model HTTP timeout does not extend
resume authority. These are engineering defaults; see the
[voice protocol](../refactor/voice-protocol.md) for exact ownership and expiry
rules. Model labels and `inference_ms` do not prove physical speaker pause,
AEC or end-to-end acoustic latency.

## Server STT

Server STT currently uses a whisper.cpp-compatible HTTP service. The server
provider is configured under:

```text
config/server.d/providers/whisper_cpp.toml
```

The authenticated HTTP voice/transcription APIs can use this provider for
explicit audio requests. Device VoiceSession uses the Easy-Turn transcript
described above and does not issue a second STT request. The Whisper endpoint
is a base URL to which the adapter appends `/inference` and `/health`; its
configured `model` is informational, while the actual weights are selected by
the service startup configuration.

## Server LLM

Chat providers use the pinned Rig fork through `LlmService`; native tool calls
enter the shared Execution ledger. The former text-based tool protocol and
standalone provider parsers have been removed.

Chat roles select a model profile in `profiles.llm.<id>`, which selects a named
adapter in `providers.llm.<id>`. Major and minor are the main chat roles:

- minor LLM is usually a smaller/cheaper model and can be used for routing or lighter work
- major LLM is usually OpenAI-compatible and can also point at LM Studio if it
  exposes an OpenAI-compatible API

Record the actual provider ID, model ID/version, complete base URL, API family
and credential source for each selected role. OpenAI adapters support
`chat_completions` or `responses`; the chosen service must support that exact
family and the required streaming, tool and JSON behavior. Anthropic and Gemini
use their own adapters. An OpenAI-compatible label alone does not establish
these capabilities.

`default` reuses minor or major when their profile IDs match; a distinct profile
creates an independent chat model. All five roles participate in startup,
health and settings updates, with shared instances processed only once. An omitted
`memory_capture` role uses the minor fallback. An omitted `sentence_expression`
role uses deterministic expression rules. Dedicated optional roles must be
included in the deployment inventory: a local major model does not make other
roles local. Do not change capture policy or execution approval to simplify a
model test.

Configure these under:

```text
config/server.d/providers/openai.toml
config/server.d/providers/gemini.toml
config/server.d/providers/anthropic.toml
config/server.d/policies/routing.toml
```

Model profiles are in `config/server.d/profiles/llm.toml`. The repository example
includes remote providers for some roles; it is not an approved deployment.

## Server Embedding

The embedding role is independent of chat roles. The current real adapter uses
an OpenAI-compatible `/embeddings` endpoint. Confirm its provider, model ID,
actual vector dimensions, service/weight version and credential source. Only
`text-embedding-3*` requests send a dimensions parameter; other models must
already return the configured dimension. Response identity, ordering and vector
shape are checked, but the request descriptor's `request-config.v1` revision is
not proof of remote weight identity.

Memory and cache bind semantic use to the active profile in SQL policy. A
configuration mismatch does not authorize rewriting that policy; canonical
keyword retrieval can remain available while mismatched semantic use is
unavailable. Keep model identity confirmation separate from policy changes.

## Server TTS

Server TTS currently targets the GPT-SoVITS WebAPI. Provider endpoint and timeout
settings live under:

```text
config/server.d/providers/gpt_sovits.toml
```

Voice profile and reference-audio parameters live under:

```text
config/server.d/profiles/voice.default.toml
```

Files referenced by GPT-SoVITS config are paths on the remote TTS server, not
local files inside `alia-server`.

Confirm the service version, complete `/tts` URL, selected voice, reference
audio and matching reference text/language, target language, and actual GPT and
SoVITS weights. `reference_voice` maps to the remote `ref_audio_path`, and
`style_prompt` maps to `prompt_text`. Record a trusted asset/version identity,
not just a display model name.

When the default voice has nonempty weight paths, Server initialization calls
the service's `set_gpt_weights` / `set_sovits_weights` endpoints. Every configured
switch must succeed. Failure is returned, and synthesis remains blocked while
the remote state is unconfirmed, including a GPT success followed by a SoVITS
failure or cancellation. Reapplying the weight settings retries the complete
configured pair. Desired settings are retained on failure; they do not certify
the remote state. Weight changes wait for the original audio streams to finish
or be dropped, and new synthesis waits for the switch owner.

With no weight paths, Alia uses a preloaded instance and reports Degraded with
an unverified model identity. A successful switch acknowledges the requested
paths; neither this acknowledgement nor HTTP health attests the remote model
bytes or proves Juno acceptance. Use a dedicated instance when Alia owns weight
switching, since another client can otherwise change its state independently.
Initial software validation can save synthesized audio privately without
playing it.

Easy-Turn, whisper.cpp and GPT-SoVITS adapters currently have no configurable
API-key or arbitrary authorization-header support. Confirm any gateway's
compatibility before choosing it. Easy-Turn's CLI listens only on loopback; its
adapter rejects URL credentials/query/fragment and disables proxy/redirect
following. Do not infer a supported remote authenticated deployment from a URL.

## Configuration and Acceptance Boundaries

Review the full effective configuration before any later model run:

1. The selected base file and recursively sorted same-name `.d` fragments are
   merged first. See [Server loading order](../../packages/alia-server/docs/configuration.md#loading-order).
2. `DATABASE_URL` and `ALIA_JWT_SECRET` override their fields. Provider API-key
   variables only fill empty keys in the matching default provider IDs
   `openai`, `anthropic` and `gemini`; they do not automatically fill custom IDs.
3. Persisted `settings.toml` overrides registered settings, with module-specific
   application timing. A provider can initialize from base configuration before
   later runtime settings apply, including TTS weight-switch side effects.
4. Committed SQL Context policy takes precedence over its old TOML projection;
   embedding profile admission remains enforced. Settings are not a universal
   replacement for startup configuration or policy authority.

Do not archive raw API keys, full settings responses or private settings files.
Record the configuration identity and credential source name/path without its
value. Unconfirmed deployment values stay marked pending in the release record.

Keep LLM `health_probe_enabled=false`. With a configured key, a disabled probe
reports Degraded; an empty key reports Unhealthy. This is not an inference test,
and the noop staging assertion that all modules are Healthy must not be copied
into a real-model acceptance. GPT-SoVITS health also accepts HTTP 400 from
`GET /tts`; it does not synthesize speech. Easy-Turn readiness/capabilities,
Whisper health and remote model names likewise need separate real-output and
loaded-model evidence. Preserve original request deadlines, concurrency and
cleanup ownership instead of changing them to obtain a passing result.

The two existing ignored targets `easy_turn_live` and `voice_stream_live` call
real Easy-Turn. The latter constructs its response/output in a fixture; neither
is a real LLM-to-TTS chain test. Reuse them only for their stated scope. The
[next-stage acceptance steps](../refactor/handoffs/2026-10-17-model-preflight/acceptance-HANDOFF.md)
separate provider checks, Easy-Turn integration and the still-pending combined
model chain. No live target is enabled by this documentation update.

## Memory

Memory is an Alia-owned core system. Capture policy, privacy filtering,
relationship modeling, and scheduled memory meditation live in the server
runtime instead of a replaceable provider adapter. Configure memory policy
under:

```text
config/server.d/policies/memory.toml
```

Automatic memory capture is agent-policy driven by default. `capture_mode =
"llm_selected"` asks the assistant-side LLM to extract only durable user facts,
preferences, stable relationship context, goals, or habits before storing
Alia `MemoryNode`s. External services do not decide what should be remembered.

When Memory is enabled, startup accepts Healthy or Degraded and keeps it
enabled. Persistent Unhealthy after the configured attempts fails startup;
there is no silent policy change that disables Memory to make startup succeed.

Memory privacy controls are available through `/api/v1/memories/privacy`.
Setting `automatic_capture_enabled = false` switches capture to
`explicit_tool_only`, while the memory system remains available for explicit
user actions such as the memory UI/API or `memory_store` / `memory_forget`.
Automatic and chat-tool memory writes block sensitive categories by default
(credentials, identifiers, health details, religious views, and political
views). Sensitive memories are only accepted through direct user-managed memory
entry.

Prompt injection uses only configured retrieval layers, defaults to
`core`, `preference`, and `relationship`, strips metadata, filters sensitive
content, and applies a bounded character budget before adding memories to the
runtime prompt.

## Deployment Reminder

The server expects provider endpoints to already be reachable. Model download,
container startup, GPU/Metal tuning, and provider-specific performance settings
should live in setup scripts or provider deployment docs rather than in the core
architecture docs.

Server provider configuration is described in
[alia-server configuration](../../packages/alia-server/docs/configuration.md).
Memory capture modes and provider-neutral capabilities are described in
[memory operations](./memory.md).
