# Server 与 Embedded 无前端操作入口

更新：2026-11-03。**Mac 保持原 `8481b0f` Server／PID14258；Pi 已安装独立顺序修复版 `58bb11e`，当前全 noop／PID4239，active／disabled／Restart=no，PCM closed、硬件 FD 为 0。** 四份 R1 claim 全部保留，最新记录仍为 `claimed_outcome_unknown`；原错误明确 manual_wakeup 未发送，未重试。此前第二次真人窗口在已初始化采集流被关闭后普通重开，偏离已验证的同进程／同原生流方案；用户说了指定句但无回复，不能据 Ready 或收到 PCM 判定免唤醒声学链成立。主线 `1a7a118` 已修正显式初始化的 UART／音频启动顺序，独立 Embedded `58bb11e` 已完成影响范围软件验证和 noop 安装核验；本次未初始化、未发送 Capture Start，不代表真实断电恢复或真人链验收通过。旧 `cc7ffef` 初始化诊断包只完成 stage，未激活。Server 恢复候选 `e2831e4` 仍因 V05 不部署，V06 也未定位。下方旧运行状态和“下一步”按记录时点保留，最新入口以紧接一节为准。

当前硬件/部署输入、只读核对与R02/R03→R05门槛集中在 [部署资料清单](hardware-deployment-readiness.md)。

当前软件产物／平台证据和真实模型端点／版本／部署待确认项见[软件发布记录](software-release.md)。Linux noop通过不代表当前Mac部署或真实模型链已验收。

Harness／Embodiment 版本切换使用[成套升级与失败恢复步骤](upgrade-recovery.md)：双端协议、数据库迁移、插件 grant 和 Appraisal pin 必须一起核对。

## 真人窗口、连续采集偏离与初始化失败（2026-11-03）

当天晚间准备时，Mac 的原 PostgreSQL 容器、固定 embedding 和 Server 未运行；先核对原容器／image／volume，再恢复 OrbStack、仅启动原容器一次、加载原 Qwen embedding（context 8192）一次，并启动原 `8481b0f` Server 一次。没有新建或还原数据库、驱逐其他模型、修改配置或发送预热推理。新鲜门禁确认 PID14258、二进制／配置身份保持，依赖 ready、无 unhealthy，三项 LLM 仍只是 live probe 关闭而 degraded。首次失败与恢复分别保留，见[依赖恢复](../../run/voice-acceptance-20261103-human/mac-recovery-after-engine/summary.json)和[实际就绪基线](../../run/voice-acceptance-20261103-human/mac-baseline-ready/summary.json)。

用户确认 R1 真正重新上电后，cold_boot_id `3b6f378b-bb0a-45f3-91c5-803256b8ad0d` 的一次初始化成功，语音进程 PID2170 与原生采集保持；第三份 claim 已持久化。第一次窗口按 30 秒进入清理并取得精确 Stop ACK，但用户随后明确没来得及说指定句，不能作为模型失败。实际编排随后停止 PID2170 并恢复 noop PID2620，关闭了已初始化的原生采集流；第二次以 PID2897 普通启动、未重新初始化，用户确认说了指定句但没有回复。见[首次初始化](../../run/voice-acceptance-20261103-human/pi-initialize.json)、[首次反馈](../../run/voice-acceptance-20261103-human/human-first-feedback.json)、[首次恢复 noop](../../run/voice-acceptance-20261103-human/restore.json)、[第二次窗口](../../run/voice-acceptance-20261103-human/window-second/summary.json)和[第二次反馈](../../run/voice-acceptance-20261103-human/human-second-feedback.json)。

**这一普通关闭／重开偏离免唤醒验证所依赖的同一长期进程／同一原生采集流，不能把传输 Ready 当作声学基线。** 第二窗口有两次 Easy-Turn 请求和一次 Juno HTTP 200，未出现上传 Full／Closed；这些元数据不证明指定人声有效、识别准确或回复已播放，也没有证明关闭／重开是唯一根因，见[有界日志元数据](../../run/voice-acceptance-20261103-human/log-review-second/model-metadata-summary.json)。两次 observer exit 124 都是窗口截止主动截断，不是 Easy-Turn deadline。修正后的[连续流操作方案](../../run/voice-acceptance-20261103-human/r1-continuous-plan/plan.json)要求：一次有效冷启动初始化后，若在同一验收轮继续窗口，只做 CaptureControl Inspect／Start／Stop，保持原 PID、device boot 和 native capture；到整轮结束或流异常退休才停进程并回 noop。Capture Stop 的会话结束与 native capture 关闭不能混同。

用户随后再次确认真实 R1 冷启动，唯一新 cold_boot_id 为 `cf884e24-7c85-43e5-a488-e5cc1293171c`。这一次启动实际 exit 1，原 stderr 为 `R1 version response timed out; manual_wakeup was not sent`：在原 2 秒绝对版本期限内没有接受到匹配回复，manual_wakeup 没有发送；不是已证实的手动命令 ACK 超时。claim 中 firmware 是初始化前写入的预期常量，不是本次观察到的固件回复；command ID 也不证明发送。见[唯一启动与 claim](../../run/voice-acceptance-20261103-human/r1-continuous-attempt/pi-initialize.json)及[阶段和历史协议只读核对](../../run/voice-acceptance-20261103-human/r1-continuous-attempt/version-handshake-readonly-review.json)。只读核对随后确认旧生产实现先打开原生音频、取得首 PCM 并持久化 claim，才执行 UART open／clear；这与已验证 cold03 工具先持有 UART 再启动采集的顺序不一致。已确认的是代码顺序偏离，本次没有逐阶段 RX／TX 计数，仍不能认定晚清输入就是现场超时的唯一根因。后续修复见下文，原 2 秒期限不延长。

临时初始化 override 已移除，没有重复初始化、补发命令或新造 UUID 重试。此时恢复全 noop PID3810，disabled／Restart=no，两路 PCM closed、硬件 FD 为 0，四份 claim（包括本次 unknown）保留，新私有 `backups/failed-r1-continuous-20261103` 保存收口材料；见[失败后状态](../../run/voice-acceptance-20261103-human/r1-continuous-attempt/restored-noop.json)。先行独立初始化诊断源码 `cc7ffef61d89392fb94c12a2324df16bc80ad765` 直接以 Pi `d392401` 为父提交，仅增加失败标量诊断和测试；Linux ARM64 initializer 单次 15 passed／0 failed／0 ignored，以及原 workspace release、ELF／ldd／loader、三配置检查的历史结果见[诊断产物清单](../../run/voice-acceptance-20261103-human/initializer-diagnostic-candidate/diagnostic-manifest.json)。该包已另存并核对 stage，**没有激活**，不再作为仅补日志后结束本轮的交付目标；原产物和证据继续保留。

**初始化顺序已在主线 `1a7a118` 修正。** 显式冷启动请求现在先持久化 claim、打开并独占 UART、完成唯一一次输入清理，准备线程结束后才启动任何模块（包括播放）及 native capture；收到并校验实际 native 16 kHz mono PCM 后，由同一个 UART 所有者执行原版本查询和单次 manual 命令。事务中不重新打开串口或再次清输入，取消或启动失败须释放／join owner 后回滚。原 2 秒版本／ACK 期限、250 ms 写期限、同 ID 单次 version requery、manual 单次尝试和 claim 格式均不变。此前增加的失败元数据保留。机制和源码证据见[顺序核对](../../run/voice-acceptance-20261103-human/initializer-order-review/mechanism.json)及[最终三文件身份](../../run/voice-acceptance-20261103-human/initializer-order-review/final-source.json)。PTY 回归用于证明已排队握手不能被晚清输入丢弃，以及取消／丢弃 prepared 时不发命令；它使用既有 PTY fixture，不代替真实 USB modem ioctl、ALSA 时序或现场根因验证。

claim 现在先于所有模块 start：即使后续模块启动或 PCM 校验失败、尚未发送 query／manual，该 ID 的记录仍保留，不能自动释放 claim、原 ID 重试或在同一上电周期另造 ID。普通启动仍不初始化，不新增设备枚举等待或自动重连初始化。独立交付源码为 `58bb11e08d50971afa4224232fa835611824abdc`，以旧 `cc7ffef` 为父提交，只移植主线 `1a7a118` 的三个 Embedded 文件；不包含 held 的 Server／Memory 恢复改动。Linux ARM64 有效验证分别为 R1 **40／0／0**、registry **1／0／0**、runtime **11／0／0**、preflight **6／0／0**（通过／失败／忽略）；原 workspace release 用时 194.495 秒，ELF／loader／ldd 与三配置预检通过。Mac 最终 registry **1／0／0**、runtime **10／0／0**；未重跑完整 workspace 回归。默认并发、原期限保持，测试没有真实 UART、音频、模型或生产数据库访问。详见[独立候选身份与验证](../../run/voice-acceptance-20261103-human/initializer-ordering-candidate/candidate-manifest.json)及[Mac 最终影响验证](../../run/voice-acceptance-20261103-human/initializer-ordering-mac-final/test-run.json)。

晚清输入的独立机制突变用例按预期得到 **0／1／0、exit 101**，证明新回归可以识别已排队握手被清掉；它不是历史现场根因的证明。第一次切回 final 路径时，共享 Cargo target 仍复用了突变测试二进制，出现的 **39／1／0** 已保留并判定不是最终源码的有效回归。依据 depfile／二进制身份核对，只清理本包 dev 构建缓存，显式重新编译并核对 final 二进制后，才取得上列各目标的一次有效结果；没有重复红例、改断言或放宽期限。见[缓存污染证据](../../run/voice-acceptance-20261103-human/initializer-ordering-candidate/cache-investigation/conclusion.json)和[有效 final 二进制](../../run/voice-acceptance-20261103-human/initializer-ordering-candidate/final-validation/final-test-binary.json)。Mac 最初 runtime 的 loopback bind 被沙箱拒绝，原不完整运行保留，允许绑定后仅补该影响目标，不能合并成一轮成功统计。

固定 Embedded 产物为 70,847,880 bytes，SHA-256 `bea1e8f7aac97f29202df276b45d4b07172cbeffc0112c18235079595ba7def5`，位置与源码／锁文件／验证对应关系见上述候选清单。该产物已安装并完成 noop 核验：当前 PID4239，active／disabled／Restart=no，unit SHA-256 为 `42a7b4b56b48adbb2eb8021ee283349c0502a6c69ab1021037d20c8fcf4ecfd4`；原配置、key、SQLite 和完整四份 claim 保持，PCM closed、硬件 FD 为 0。签名 Inspect 核对 boot `77c887c2-8722-4419-a6d6-de0b754ba429`、generation 0、inactive／available=false；noop 未连接采集 pipeline，available=false 是该模式的预期状态。安装前材料保存在私有 `backups/pre-initializer-ordering-20261103`，旧 `d392401` 与仅 stage、从未激活的 `cc7ffef` 保留。见[实际安装](../../run/voice-acceptance-20261103-human/initializer-ordering-deploy-plan/activate-execution.json)及[签名 noop 核验](../../run/voice-acceptance-20261103-human/initializer-ordering-deploy-plan/noop-activation-complete.json)。本次初始化请求和 Capture Start 均为 0，Mac 原 Server 未变；这次安装不等于已验证真实冷启动恢复或真人声学链。

下一次获准的真人轮使用修复后的独立产物，并先确认真实新的板端上电事实和用户可即时讲话时机。一次初始化成功后，同轮各窗口仅做 CaptureControl Inspect／Start／Stop，保持同一 PID、device boot 与 native capture；Stop 结束会话，不关闭底层采集。到整轮结束或流异常退休才停止进程、恢复 noop。完整回复链、Hold／Resume、Cancel、后续对话、24 小时负载及 AEC 仍分别待验。

## 独立 Embedded 诊断与就绪窗口（2026-11-03）

用户授权部署诊断件后，Pi 安装 `/home/ke-qiao/.local/share/alia/releases/d392401/alia-embedded`，SHA-256 `2062967f682a5125d401aa0444d4943c962e9875944dfc911445f6acde129346`。源码 `d39240111617a4c741f8dc5fdd4aa32159df6012` 直接以 `8481b0f` 为父提交，只移植两个 Embedded 日志／测试文件；不包含主线尚未放行的 Server 恢复代码。Linux 原 workspace release、一次 Embedded duplex 模块回归及静态预检的身份见[诊断清单 v2](../../run/voice-acceptance-20261103/diagnostic-candidate/diagnostic-manifest-v2.json)。这是一组明确记录的 Mac 旧版＋Pi 诊断件组合，不是整套新版 Release 或 `e2831e4` 放行。

安装仅调整两份 systemd unit 中 `ExecStartPre`／`ExecStart` 的二进制路径，原配置、身份、SQLite 绑定、两份 claim、disabled／Restart=no 保持；没有插件重新授权。首次普通 noop 启动为 PID5012，修正本地 Inspect CLI 参数门禁后，签名 Inspect 确认为新 boot、generation 0、inactive／available=false；noop 没有连接采集 pipeline，不能要求 available=true。原拒绝发生在网络调用前，见[实际部署](../../run/voice-acceptance-20261103/diagnostic-deploy-plan/deployment-summary.json)及[签名核验](../../run/voice-acceptance-20261103/diagnostic-deploy-plan/noop-activation-complete.json)。旧二进制和私有恢复点均保留。

新窗口准备首次在任何停服／启动前误把 noop 独有的 `grpc_drain_timeout_ms`／`shutdown_grace_ms` 与 voice 默认值差异当作身份差异，原失败保留；只核对该已存在的配置差异后继续，未改产品期限。见[准备拒绝](../../run/voice-acceptance-20261103/diagnostic-window-preparation/prepare.json)、[差异核对](../../run/voice-acceptance-20261103/diagnostic-window-preparation/prepare-config-difference.json)和[准备通过](../../run/voice-acceptance-20261103/diagnostic-window-preparation/prepare-corrected.json)。Mac 本轮只读门禁确认生产字节／配置不变，模型依赖 ready，Server 模块无 unhealthy；三项 LLM 仍仅因关闭 live probe 显示 degraded，见[Mac 基线](../../run/voice-acceptance-20261103/diagnostic-mac-baseline/summary.json)。

用户暂时不能讲话，因此本次只执行就绪诊断。普通语音进程 PID5788 无初始化启动；同一编排仅一次 Start（capture generation 2）→有效 SessionReady→新的 Inspect→一次 Stop（generation 3、inactive，精确 ACK），原 5 秒 RPC、零重试保持。0.681366 秒进入清理，0.795196 秒完成，没有 human prompt，也没有等待 30 秒讲话。见[窗口摘要](../../run/voice-acceptance-20261103/diagnostic-readiness-window/summary.json)、[Start](../../run/voice-acceptance-20261103/diagnostic-readiness-window/start.json)和[Stop](../../run/voice-acceptance-20261103/diagnostic-readiness-window/stop.json)。它证明本次 Start→Ready 和受控 Stop 可完成，不证明物理静默期限、真人输入或回复播放。

窗口后有界读取原日志增量，原始字节仅保存在私有证据目录：观察到一条 Ready，没有 terminal／Full／Closed；Easy-Turn accepted 日志事件 0、Juno `POST /tts` 访问事件 0。现有日志不能精确计数云 LLM／embedding 请求，不能将缺少可计数日志写成实际请求必为 0。见[日志元数据摘要](../../run/voice-acceptance-20261103/diagnostic-log-review/capture/summary.json)。**一次未复现不能关闭 V06，也不能把日志补充称为故障修复。**

语音进程随后正常停止，恢复诊断版全 noop PID6015；两路 PCM closed、硬件 FD 为 0、disabled／Restart=no，两份 R1 claim 摘要保持，没有 initialization override。私有 `/home/ke-qiao/.local/share/alia/backups/voice-window-diagnostic-20261103` 保存收口状态，尚未演练还原，见[最终 noop 状态](../../run/voice-acceptance-20261103/diagnostic-window-preparation/restore.json)。下一步等用户能够即时讲话时重新核对当前身份和新鲜 Ready，再做一次自动有界收口的短句窗口；不要求再次断电或重复初始化。长句换气、模型回复、Hold／Resume、输出 Cancel、Stop 后新 Start／下一轮、24 小时混合负载和 AEC 仍分别待验。

## 窗口前置依赖恢复与中止（2026-11-03）

准备新窗口时，Mac 首次仅余 149 MiB 磁盘空间，文档／脚本写入遇到 ENOSPC。只删除已核对为可重建的 `target/debug/incremental` 缓存（清理前约 202 GiB），未删除源码、数据库、模型或证据，见[限定清理记录](../../run/voice-acceptance-20261102/disk-cache-cleanup.json)。随后发现 Docker API 不可达、OrbStack engine 未运行；不能据时间邻近推断磁盘不足就是 engine 停止的原因。`open -a` 后仍未启动 engine，继而用 `orbctl start` 恢复，再只对精确匹配的原 PostgreSQL 容器执行一次 `docker start`，未创建或重建容器、volume、image，也未还原数据库。原失败与恢复核对见[依赖诊断](../../run/voice-acceptance-20261103/health-diagnosis/assessment.json)、[engine 恢复](../../run/voice-acceptance-20261103/health-diagnosis/orb-start.json)和[原容器启动](../../run/voice-acceptance-20261103/health-diagnosis/start-existing-postgres.json)。

数据库恢复可达后，旧 Server PID50935 仍显示 tools unhealthy 和 Memory rebuild scan degraded；三项关闭 live probe 的 LLM degraded 与此分开。单次停止旧进程实际 exit 1，保留原失败；六类执行／输出账本的 count 与行摘要在停服前后相同，两条 unknown Delivery 未删除或重派。新私有恢复点的 dump／配置材料与未演练还原的边界见[恢复说明](upgrade-recovery.md#窗口前置故障与新恢复点2026-11-03)。之后仅启动同一 `8481b0f` Server 一次，PID77141 约 4.057 秒恢复就绪，六类账本摘要仍相同；tools／Memory 恢复健康，三项 LLM live probe 仍按配置关闭。这是人工受控恢复，不证明 V03 自动恢复或异常关闭已修复，见[停止与新恢复点](../../run/voice-acceptance-20261103/health-diagnosis/controlled-server-checkpoint.json)及[同版恢复](../../run/voice-acceptance-20261103/health-diagnosis/server-resumed.json)。

Pi 随后仅以原语音配置普通启动 PID3962，没有初始化请求；新 wrapper 在发送任何 Capture Start 前发现旧 `target/debug/examples/capture_window_control` 不存在，整个旧 `target` 目录在检查时缺失，原因未定。限定缓存清理只针对 `debug/incremental`，不能据此解释父目录缺失。当时先停止 PID3962，exit 0、两端 PCM closed，两份初始化 claim 摘要保持。随后已在独立目录用原归档源码与锁定依赖重建 helper，固定到 `run/voice-acceptance-20261103/control-bin/`，原 13 项自检与离线配置检查通过；未覆盖旧失败 manifest。见[前置拒绝](../../run/voice-acceptance-20261103/window-manifest-gate-failure.json)、[当时停止核验](../../run/voice-acceptance-20261103/pi-stop-await-helper.json)及[helper 重建记录](../../run/voice-acceptance-20261103/control-build/summary.json)。

新 wrapper 在同一进程中从唯一 Start 调用前计时，30 秒到时进入 Stop／异常清理；这不是保证设备在第 30 秒物理静默。23 项假时钟／runner 离线检查通过，保留原 5 秒控制 RPC、零重试及身份不匹配不盲目 Stop 的边界，见[离线检查](../../run/voice-acceptance-20261102/next-window-preparation/selfcheck.json)。重建后的第一次真实 Start 在 Ready 前约 0.6 秒出现旧版混合错误 `voice upload queue full or disconnected`，没有提示用户讲话；fresh Inspect 已为同 boot 的下一代 inactive，wrapper 保守退出并未盲目 Stop，随后恢复 noop。它没有区分 Full 与 Closed，独立按 V06 保留，见[首次窗口](../../run/voice-acceptance-20261103/window/summary.json)和[只读定位](../../run/voice-acceptance-20261103/upload-queue-diagnosis/readonly-review.json)。后续仅补诊断并完成上节就绪窗口，没有完成新的指定短句测试。11月2日用户没有讲话的澄清不改变旧实际账本，也不允许重复初始化，见[用户澄清](../../run/voice-acceptance-20261102/human-clarification.json)。

## 真人短句窗口与安全收口（2026-11-02）

用户确认 R1 自上次窗口后真正断电，本次以唯一 cold_boot_id `0797886f-48b2-401d-958b-08f0620d0111` 初始化一次；固件检查与 beam 0 命令接受成功，同一长期语音进程 PID2857 保持。临时初始化 override 移除后没有重启或补发初始化。新 claim SHA-256 为 `37a93185fbffc1013f7bfcf80c8aec7fd30bf405e6a85ee0ec1f546f3e7b4cff`，10 月 31 日旧 claim 也保持。见[唯一初始化记录](../../run/voice-acceptance-20261102/pi-initialize.json)；命令接受不等于声学输入或回复播放已验证。

第一次 Start 后，只读日志 observer 因 `PermissionError` 失败，未向用户发出指定短句提示，随后先确认 Stop；不能把该阶段环境／偶发输入当作指定测试句。修正 observer 为受限 `sudo` 只读日志后，第二次 Start 的 capture generation 为 4，取得当前 SessionReady 并再次 Inspect 确认 active 后才提示讲话。后续签名 Stop 成功推进到 generation 5、active=false，原 5 秒 RPC 期限和零重试保持。capture generation 与 VoiceSession generation 不是同一身份字段。见[首阶段解释](../../run/voice-acceptance-20261102/first-window-assessment.json)、[有效 Ready](../../run/voice-acceptance-20261102/session-ready-v2.json)、[提示时状态](../../run/voice-acceptance-20261102/at-prompt-v2.json)和[第二次 Stop](../../run/voice-acceptance-20261102/stopped-v2.json)。

只读 SQL 确认本窗口产生两条 Delivery，均为 unknown；offered／accepted 为 0，submitted／presented 为 null。第一条取消时 SQL observation 总期限尚未到；Pause AcousticCandidate 至 `tts_output` 中断约 2.990 秒，与现有 gate 的 3 秒临时语义 hold 到期一致，不能把通用 deadline 日志误称为总期限耗尽。下一条 Backchannel 在退休后才返回；第二次取消则与本轮明确的 Stop 一致。三个中间 turn 各自创建新 producer／gate，时序支持独立新 hold 到期；缺少逐 hold 触发帧身份，`stage=llm` 也不证明三次模型超时，见[有界核查](../../run/voice-acceptance-20261102/sql-analysis/later-turn-holds-readonly-review.json)。仍不能由此判断声源或识别准确，也不能重放 unknown 输出。用户已确认旧窗口没有讲话，指定短句测试未执行；舍友聊天是用户推测、未由PCM确认，不再等待旧窗口听感，见[用户澄清](../../run/voice-acceptance-20261102/human-clarification.json)。完整判句→回复播放、打断／恢复、输出 Cancel 与下一轮均没有本窗口通过结论。见[账本与取消分析](../../run/voice-acceptance-20261102/sql-analysis/summary.json)和[模型窗口记录](model-services.md#受控真人窗口与输出取消2026-11-02)。

语音 PID2857 正常退出 exit 0 后，恢复原 active noop 配置和既有身份／绑定，收口时 PID3435、disabled／Restart=no；全部 PCM closed，硬件 FD 为 0，初始化 override 不存在。新旧两份 claim 摘要均核对保持，私有 `/home/ke-qiao/.local/share/alia/backups/voice-window-20261102` 保存本次收口状态，见[语音退出](../../run/voice-acceptance-20261102/pi-stop-voice.json)与[noop 收口](../../run/voice-acceptance-20261102/pi-restored-noop.json)。该窗口没有安装新 Rust 候选。用户随后要求重来；11月3日的 helper 重建、首次上传失败、独立诊断件和无讲话就绪窗口已在上节分别记录，没有新的指定短句通过结论，也不要求重复断电或重跑旧硬件基线。恢复早先快照前必须保留当前完整 claim 集合及本次新增账本证据，见[本窗口恢复边界](upgrade-recovery.md#真人窗口后的恢复边界2026-11-02)。

## 主机名连接与受控恢复（2026-11-02）

用户选择电脑名 `.local`，因此 Mac 的 public endpoint 现为 **`http://Ke-QiaoMac.local:3000`**，VoiceSession 继续使用同主机3001。Mac 当前 DHCP 地址为 `.191`；listener 改为 `0.0.0.0`，实际监听所有 IPv4 接口，仍只用于受控局域网，不新增公网转发，现有认证与设备公钥 pin 保持。Pi 仍为 `192.168.10.221:50051`，其地址变化仍需独立核对 channel，不能误以为 Mac `.local` 同时解决 Pi DHCP。

Pi 使用原 systemd-resolved，在全局与 wlan0 的两份 drop-in 开启 mDNS `resolve`，不安装新 daemon、不广播服务。实际 `Ke-QiaoMac.local` 解析为 `.191`，两端口连通。之后用原 Server key 执行一次签名 `PrepareHandoff`＋`CompleteHandoff`，同一 `alia-main`／设备身份更新 endpoint；没有 Unbind、直接写 SQLite 或重新配对。该协议更新 bound_since 并撤销旧输出代次；原公钥、owner、配置及 R1 初始化 claim 保持。见[签名移交](../../run/software-recovery-20261102/network/handoff/execution.json)与[持久地址读回](../../run/software-recovery-20261102/network/pi-after-handoff.json)。

本人管理员的签名 settings 读取通过，缺失／伪造认证各为gRPC16；一次普通 noop 重启后 endpoint／bound_since／公钥与 claim 均保持，PCM closed、硬件 FD 为0。见[重启及认证结果](../../run/software-recovery-20261102/network/pair-migration.json)。Pi仍disabled／Restart=no；本轮没有麦克风、播放、Capture Start／Stop或UART初始化。resolver 的整机重启持久性、异常自动恢复和长稳尚未验。

恢复 Mac 时先被既有启动门禁拒绝：LM Studio当时停止，没有已加载实例。保留首次30秒观察失败与gate日志，按原回环1234和context8192恢复固定Qwen embedding后才受控启动；没有改变共享JIT、期限或发送推理。恢复时Server PID50935，loopback／LAN／`.local`健康通过，账号、33迁移、Qwen1024双pin和shadow revision1不变；degraded仅为三项关闭在线probe的LLM。见[依赖恢复](../../run/software-recovery-20261102/network/lmstudio-recovery.json)和[实际服务恢复](../../run/software-recovery-20261102/network/existing-server-mdns-resumed.json)。这仍是受控恢复，不宣称登录后自动恢复通过。

本轮新 Rust 修复未安装，运行字节仍为Mac Server `cba48222…e8cd`／Pi Embedded `6608a23d…680a`。新源码、软件回归失败与构建产物单独见[本轮发布记录](software-release.md#数据库恢复修复与候选准备2026-11-02)，不能把第三次诊断通过当作前两次失败的根因已解决。当前新旧恢复点及隔离还原边界见[升级与恢复](upgrade-recovery.md)。

下一步可在用户明确在场后，以当前已部署版本做短句受控复验，重点确认当前有效SessionReady后即时讲话、判句与回复播放。先确认10月31日以来R1是否真正断电，不要求为了测试拔插；只有新的真实断电事实才允许新cold_boot_id与一次初始化。短句结果不能替代换气长句、Hold／Resume、Cancel、Stop／新Start、下一轮、24小时负载或AEC；V03／V05与新候选部署门槛继续独立保留。

## 真人窗口失败与安全收口（2026-10-31）

用户确认 R1 在上次验证后真正断电重启，随后以唯一 cold_boot_id `d9b21420-fa34-43ee-bc55-2406fc696442` 执行一次初始化；固件回复匹配，beam 0 命令被接受，初始化本身不证明声学内容有效。ALSA 16 kHz mono 采集与 48 kHz stereo 播放打开，USB `PCM` mixer 只读确认仍为 −14 dB，未重复硬件基线或调整音量。同一 PID／采集流保持，移除临时初始化 override 后没有重启或补发 UART。见[初始化](../../run/voice-acceptance-20261031/pi-initialize.json)。

两次签名 Start 分别获得新代次和 SessionReady，随后均以 `DeadlineExceeded: turn model deadline` 结束并自动退休。第一会话没有指定测试句；第二会话 Ready 为 08:26:50 UTC、结束为 08:27:02，讲话提示记录为 08:27:37，现场调度未及时确认会话仍有效。用户反馈“没有回复”保留，但不能据此认定「你好 Alia，请用一句话介绍你自己」进入了模型。没有已确认回复／播放，长句、Hold／Resume、输出 Cancel、成功 Stop 与下一轮均未执行。原始日志和纠正后的时序说明见[本轮汇总](../../run/voice-acceptance-20261031/summary.json)。

关闭语音进程 exit 0 后，已恢复原全 noop 配置、同一身份和绑定，当前 Embedded 仍运行（disabled／Restart=no）；采集／播放 PCM closed、硬件 FD 为 0。active 配置 SHA-256 为 `54c00655071804c69ddfaccadef37c6b3a3fca4ab66b3a5796dacc6c41d51797`，新 claim 及其独立私有副本均保留，见[收口核验](../../run/voice-acceptance-20261031/pi-restored-noop.json)。普通后续重启不能再次初始化；不能恢复 10 月 30 日的空 claim 集合覆盖新增记录。Mac 服务继续运行，Easy-Turn 新诊断的激活和固定样本失败单独见[模型记录](model-services.md#真人窗口失败与-easy-turn-诊断补充2026-10-31)。

窗口开始前还发现独立的 Mac 恢复故障：数据库本身重新可达、管理员登录成功，但 Execution worker 保留 deadline fault，Memory 健康仍报 rebuild scan 超时。错误时段与合盖休眠／DarkWake 相邻，仅支持时间相关，尚未证明底层根因。Server 正常 SIGTERM 请求后实际 exit 1，包含 worker／Delivery 未完成关闭和未知提交结果；受控重启后健康恢复，不等于根因或自动恢复已解决。保留[故障调查](../../run/voice-acceptance-20261031/database-health-investigation.json)、[停止结果](../../run/voice-acceptance-20261031/server-stop-outcome.json)和[重启结果](../../run/voice-acceptance-20261031/server-recovery-start.json)。这两项新增故障分别按 TODO V03／V04 跟踪，不并入旧 V01／V02。

下一步先完成无设备的软件定位：原 5 秒预算内的 CPU 实际窗口推理，以及数据库暂不可达／休眠后 worker、健康与关闭语义的隔离验证。重新进入真人窗口时先协调讲话时机，取得当前有效 SessionReady／generation 后立即提示并同步观察终止；会话已退休时取消该提示，不能拿旧 Ready 继续要求讲话。模型、密钥、参考文本和账号无需重选，当前不用本人守在设备旁，也不要求再次断电。

## Pi 正式身份无采播上线（2026-10-30）

用户提供可用 Pi 后，经固定 SSH key、严格 known_hosts 校验核对 `KeQiaopi`（Ubuntu 24.04.4／aarch64）。旧项目真实设备 ID 为 `alia-embedded-2ae7195b6a9f`，公钥 `KucZW2qfcqs4Zau0GvPzLIVJ+27hw7i99VTsjxn1Tcg=`；SQLite 原先只有音量 1/1，没有 binding。没有使用历史验证身份，也没有重置旧 key、SQLite／WAL／SHM 或旧验证目录。新目录 `/home/ke-qiao/.local/share/alia` 中的 key 仅在 Pi 内复制，SQLite 通过 backup API 迁移，安装前后旧文件摘要和逻辑内容保持。

两端沿用代码候选 `8481b0f`，本轮没有源码、锁文件、协议或数据库迁移变更。Pi 安装固定 Linux Embedded、Silero 和独立状态目录；ELF／loader／ldd、active 与 voice.pending 两份配置的 `--check-config`，以及实际 Pi 的 `systemd-analyze verify` 全部通过。安装证据见[Pi 汇总](../../run/pi-production-20261030/pi-install/summary.json)。

| 当前对象 | 实际入口／状态 |
| --- | --- |
| Mac Server | `192.168.10.201:3000` HTTP、`:3001` VoiceSession gRPC；仅绑定该 LAN 地址，`gui/501/com.alia.server` 运行中 |
| Pi Embedded | `/home/ke-qiao/.local/share/alia/releases/8481b0f/alia-embedded`，`192.168.10.221:50051`；`alia-embedded.service` 运行中，disabled、Restart=no |
| Pi active 配置 | `config/embedded.toml`，七个模块槽位全部 noop，Motion／vision／毫米波／mDNS／BLE 关闭，`skip_binding=false` |
| 待真人配置 | `config/embedded.voice.pending.toml`，同一身份／SQLite、固定 Silero 和已有 R1 system 参数；已预检，未选择、未启动 |
| 正式绑定 | `alia-main` → `alia-embedded-2ae7195b6a9f`；channel `pi-living-room`，owner 为本人 UUID `a4b078c0-daf5-47f4-9303-e30af26eadb6` |

首次配对先让 Pi 仅监听 loopback，经严格校验的 SSH 隧道读取设备 ID／公钥并与 Pi 磁盘独立身份比较，再以本人管理员发起一次 manual Bind。`require_confirmation=false` 只绕过当前无 mDNS 的候选检查，`skip_binding` 始终 false；没有实现物理按钮确认，也没有设备签名挑战应答，不能据此宣称不可信网络上的自动首次配对安全。绑定记录的 Server endpoint 是 `http://192.168.10.201:3000`，VoiceSession 按既有规则使用 3001，不能把 3001 写成 HTTP base。

随后两端正常停止、Pi 仅改 listener、Mac 改为正式 LAN channel 后重启。设备 key、Server 公钥、binding 内容和 bound_since、音量均保持；两阶段的合法签名设备 settings 读取均为200，直接缺失／无效认证各返回 gRPC16。关闭本轮 SSH 转发后再次成功读取设备 settings，并核对真实 TCP peer 为 Pi LAN；Pi 到 Mac 3000／3001 可达，HTTP health可读。仅有 TCP Connected 或公开 BindingStatus 不作为认证成功证据。见[首次绑定](../../run/pi-production-20261030/pairing-checks.json)、[LAN 检查](../../run/pi-production-20261030/lan-checks.json)、[重启比较](../../run/pi-production-20261030/restart-comparison.json)和[隧道关闭后网络](../../run/pi-production-20261030/network-final.json)。

两次 Pi cgroup／设备状态快照中，硬件 FD 为0，枚举到的 capture／playback PCM 全部 closed，初始化 claim 集合为空；没有发送 Start／Stop／UART 初始化或生成播放内容。claim 空不能证明 R1 刚冷启动，也不能授权补发初始化。旧硬件基线继续复用，当前二进制的完整真人生产链仍未验。

本轮开始时 OrbStack 停止、PG不可达、embedding 未加载、Server未运行；按既有归属启动原引擎／原容器并加载选定 embedding 后就绪通过。Easy-Turn／独占 Juno 原进程未重启。这是手动受控恢复，不证明登录／整机重启或异常自动恢复。[生产 SQL 只读复核](../../run/pi-production-20261030/sql-current.json)确认原管理员、33迁移、Qwen1024 Memory／Cache 双pin与 shadow revision1保持；四张 canonical／cache／projection 表仍为空。整体health degraded仅来自关闭在线LLM probe，TTS／embedding healthy，没有额外推理请求。

当前 Pi 的正常操作入口（通过已核验 SSH 主机执行）为 `sudo -n systemctl status/start/stop alia-embedded.service`；启动前必须复核 active 配置仍为预期文件，不能把 voice.pending 替换进去后照搬本轮“无设备”结论。日志在正式目录 `logs/embedded.stdout.log`／`embedded.stderr.log`。Mac 的启动门禁已固定最终配置摘要；后续配置变更须停服、审阅并同步 `services/deployment-paths.json`，否则门禁会拒绝启动。私有管理员登录元信息已更新为 LAN 地址，密码不变且不进入报告。

已创建配对后的双端私有[恢复快照](../../run/pi-production-20261030/paired-checkpoint.json)，这份快照尚未做实际恢复演练。局域网地址使用本轮实测值，尚未验证DHCP保留。地址变化须先停服，按经过审阅的绑定／移交流程一起核对 Mac listener／Pi channel endpoint／Pi bound Server endpoint，不能直接改SQLite、只改一处或恢复旧绑定猜测。当前 HTTP／gRPC 沿用受控 LAN 传输，未配置 TLS，不暴露到公网。

**下一步需要本人在场并明确进入采播窗口。** 使用已准备的 R1 system 配置与现有签名 CaptureControl 入口，按可确认的板端冷启动事实处理初始化；完成生产启动→讲话→模型→回复播放→打断／恢复→取消与下一轮，包含正常换气长句、Start／Stop和初始化后的真人语音。模型、密钥、参考文本和账号无需重选。双端升级／恢复、登录及异常恢复、24小时混合负载、AEC与具体生产插件仍独立验收。

## Mac 正式启用（2026-10-28）

本轮把已验证的Mac候选安装到 `/Users/ke-qiao/Library/Application Support/Alia`，首次使用 `releases/6123691`，健康检查修复后当前运行 `releases/8481b0f/alia-server`；私有配置、凭据和状态位于版本目录之外，日志位于 `/Users/ke-qiao/Library/Logs/Alia`。保留原 `alia-main` Server key字节，生成新库所需凭据与JWT；不导入演练账号、旧库或备份。脱敏安装与依赖证据见[本轮汇总](../../run/mac-activation-20261028/summary.json)，私有配置及凭据原值不进入仓库或报告。

三个 `com.alia.easy-turn`、`com.alia.juno`、`com.alia.server` LaunchAgent 已安装到用户 `~/Library/LaunchAgents` 并登记在 `gui/501`。Easy-Turn 与 Juno 使用登录启动，Server 保留 `RunAtLoad=false`，全部 `KeepAlive=false`；这是受控的登录会话运行方式，不是无人登录开机服务或故障自动恢复承诺。共享 Magpie／LM Studio／9880 TTS 沿用原归属，不随 Alia 停止。

LM Studio 已应用回环监听、关闭敏感／verbose 日志和登录服务设置，选定 Qwen embedding 的精确实例及 context8192 已加载；没有改共享 JIT 或驱逐策略。Easy-Turn8091、独占 Juno19880 的实际就绪检查通过，包含 Juno 的 LaunchAgent／PID／监听／运行 YAML／磁盘权重归属检查。新 `alia-production-pgdata` 卷使用固定 PostgreSQL17.10／pgvector 镜像，仅发布 `127.0.0.1:5432`，`vector`／`pgcrypto` 与最终 TCP 就绪已核对；不是从已删旧卷恢复。GET 就绪不等于真实推理、resident tensor 字节或持续驻留保证。

冻结候选 `6123691` 首次启动后约5988ms提供HTTP，已注册首位 active管理员 `ke-qiao@alia.local`，UUID为 `a4b078c0-daf5-47f4-9303-e30af26eadb6`；Companion为shadow revision1。实际核对33项迁移与源码精确一致，Memory／Cache同时固定到选定Qwen1024 profile、policy version1，四张canonical／cache／projection表均为空，原Server key一致。迁移会另行登记3个静态seed profile；首个验证脚本误以为只应有1个profile而失败，原断言及修正保留，不能把额外seed视为旧库恢复或切换错误。

实际 `/health` 暴露GPT-SoVITS旧就绪探测的问题：GET `/tts`缺少语言参数，服务返回500（`None.lower`）。Server随后正常停止，exit0、约161ms完成关闭，3000／3001关闭且生产DB连接归零。停服后建立私有 `backups/initial-20261028` 恢复点，在network-none／tmpfs独立PostgreSQL还原成功，67张public表的count与排序JSON摘要逐项一致，临时容器已删除，生产库未被替换或写入；见[恢复核验](../../run/mac-activation-20261028/restore-checkpoint.json)。

修复提交 `8481b0faf1b92bb39905d84af13bad2cd731e70e` 的Mac发布二进制已独立安装并重启，约5219.626ms提供HTTP；本人UUID／admin／active、shadow revision1、33项迁移、Qwen双pin、四张空表与原Server key全部保持，仍不存在settings覆盖文件。TTS与embedding为healthy，没有unhealthy模块；整体degraded仅来自3项有意关闭在线probe的LLM状态，不为求全绿开启额外付费探测。见[安装身份](../../run/mac-activation-20261028/installed-fix.json)、[实际重启](../../run/mac-activation-20261028/fixed-restart.json)与[修复验证](software-release.md#mac-正式启用与健康检查修复2026-10-28)。本轮结束时Mac正式服务保持运行，未发起模型推理、采集、播放或Pi部署。

首次顺序为：核对全新库和私有身份 → 模型就绪 → Server迁移与loopback监听 → 首位管理员注册 → 实际SQL profile／Companion shadow核对。不能要求新管理员在Server首启前已经存在，也不能把macOS登录名当成应用用户UUID。当前 `channels=[]`、discovery关闭，HTTP3000／gRPC3001都绑定回环；下一次修改Server channel须引用上述真实UUID，不使用演练owner。

当前会话中的状态与受控启动入口如下；用 `ke-qiao` 执行，不使用 `sudo` 或 `system` 域：

```sh
launchctl print gui/501/com.alia.easy-turn
launchctl print gui/501/com.alia.juno
launchctl print gui/501/com.alia.server
'/Users/ke-qiao/Developer/project-alia/services/easy-turn/.venv/bin/python' '/Users/ke-qiao/Library/Application Support/Alia/services/ready-check.py' --check
launchctl kickstart gui/501/com.alia.server
```

仅在 Server 已停止、私有 base／settings 摘要已重新审阅且 ready通过后执行最后一行。正常停止先 `launchctl kill SIGTERM gui/501/com.alia.server`，核对该 PID退出、3000／3001关闭和日志收尾；如需停整个 Alia 模型组，再依次对 `com.alia.juno`、`com.alia.easy-turn` 发 SIGTERM并核对退出。只有确认停止后才 `launchctl bootout gui/501/LABEL` 卸载精确 job；重新登记使用 `launchctl bootstrap gui/501 ~/Library/LaunchAgents/对应文件.plist`，其中模型 job 会按 RunAtLoad立即启动。不要用按进程名杀进程或强制 kickstart掩盖未清理状态。生产 PostgreSQL在 Server停止后才可用其私有 compose执行 `stop postgres`；不因此停止共享 OrbStack 或其他容器。日志轮转沿用[停服维护步骤](../../run/fresh-start-20261028/services/README.md#停止日志与恢复)，运行中日志磁盘硬上界仍待验。

当前没有供 `channels=[]` 使用的完整纯文本 HTTP 对话入口：`/api/v1/voice/chat` 仅传 text也会合成音频，且直接调用 default LLM，不经过 Interaction／路由／Execution／canonical memory；`/voice/chat/{channel_id}` 只有已注册非语音 channel才会跳过TTS。不能把它当作无音频完整会话检查，也不为本轮临时增加入口。

下一步先通知用户准备 Pi，仅核对主机、设备／Server身份、实际管理员 owner UUID、可达 endpoint、候选产物和私有持久状态，在离线副本执行 `--check-config` 与目标 `systemd-analyze verify`；这一步不启动 Embedded，因为正常启动可能开启采播。完成双方配置审核后，再安排真人窗口启用设备，集中验启动→讲话→模型→播放→打断／恢复→取消与下一轮，包含换气长句、Start／Stop和生产初始化后的真人语音。实际升级／恢复、登录后恢复、进程退出恢复、24小时混合负载、AEC与具体插件仍分别保留验收门槛。

## 当前部署决定（2026-10-28）

用户决定**不以旧库或备份作为新生产数据来源，删除两套旧 PostgreSQL**：`alia-postgres`＋`project-alia_pgdata`、`alia-python-postgres`＋`alia_python_postgres-data`。此前“保留旧卷并查询停服副本”的建议已撤回；下方 10 月 25 日的元信息与只读证据仍作为历史保留。新库后续按全新部署准备，不继承旧账号、embedding pin 或 Companion 政策，也不把演练 fixture 账号当成生产 owner。首次初始化后须实际核对 schema、选定的 1024 维 profile 与 shadow 政策；空库没有历史记忆需要重建。

**两套旧容器和专属数据卷已删除并核验不存在。** 用户明确授权临时启动 OrbStack 后，核对完整容器 ID、停止状态和数据卷独占关系，使用非强制删除；其他 53 个容器、48 个数据卷及网络清单保持不变，初始化脚本源目录保留。未新建备份或生产库，配置、密钥、模型和历史证据未纳入删除。引擎启动期间 `astrbot` 按既有 `always` 策略自动运行，随后已随 OrbStack 恢复停止；未改变该策略。操作、删除后检查和 `Stopped` 状态见[完成记录](../../run/deployment-decisions-20261028/deletion-approved/result.json)。此前启动审批阻断及首次过窄挂载断言保留为历史；后者发生在任何删除前，按实际只读 init bind／独占数据卷核对后继续。

首轮运行域采用 **`ke-qiao` 登录后的 macOS 用户会话**。minor 已改为官方 `deepseek/deepseek-flash`＋none，独立输出预算保留；[准备配置](../../run/fresh-start-20261028/config/server.production.pending.toml)与[准备服务材料](../../run/fresh-start-20261028/services/README.md)保留当时未安装状态，实际安装见上节，不能直接用 pending配置替代私有有效配置。旧 Instant 驻留不再是就绪条件；只要求 local embedding 的精确实例与 context8192，不自动改共享 JIT。保留 `alia-main` 密钥、major、embedding、Easy-Turn 与 Juno。macOS 登录不自动创建 Alia 管理员。

新生产 PostgreSQL 使用[独立 manifest](../../run/fresh-start-20261028/postgres-production/compose.yaml)：固定已验 ARM64 image 内容 ID、仅 loopback5432、新 external 卷 `alia-production-pgdata`、同一私有密码文件、首次 restart=no 和 Docker 日志轮转。准备阶段只解析和静态核对，随后按上节正式创建；不使用含开发密码及宽监听的旧 compose 直接开生产。原准备步骤见[数据库准备](../../run/fresh-start-20261028/postgres-production/README.md)。

启动材料补齐了新库凭据连接：Server gate 从与 Compose 相同的私有 `postgres-password` 文件读取，复用 owner／权限／非symlink检查，按镜像 FILE 的尾LF语义处理并URL转义，仅向子进程注入固定loopback数据库DSN；清除外部继承值，不打印原值。准备阶段最终13项 gate／特殊字符／权限／exec拦截检查通过，3份plist、15项ready fixture通过。另以6份合成日志验证系统newsyslog停服轮转，固定保留5个归档；首次count配置错误及修正保留。这些 fixture不构成在线日志硬上界或实际安装结果，正式安装另见上节。

[隔离新库验证](../../run/fresh-start-20261028/database/summary.json)已使用冻结 Mac 发布二进制完成首次启动、HTTP 注册管理员／普通用户、默认 shadow、选定 embedding 双 pin 与同身份重启。隔离账号／key 全部为临时测试身份，清理后不可迁作生产；生产本人首位管理员随后在上节受控 loopback 窗口独立登记，密码只在本机保存，不发入对话。真实 Pi 的 owner／设备 pin／网络仍待现场绑定。该隔离阶段未创建生产服务或启用音频，不把它改写成本轮正式启用结果。

## 本机数据与常驻准备盘点（2026-10-25）

本次只读盘点，没有启动数据库、加载模型、安装服务、接触 Pi 或音频；没有新建或覆盖生产数据。[盘点与材料汇总](../../run/mac-production-readiness-20261025/summary.json)保留容器／数据元信息、身份公钥指纹、服务状态、检查结果与原文件摘要。它补充前轮的合成 SQL 演练，不能当作已读取真实数据库。

### PostgreSQL 实际位置

在 PATH、Homebrew 已安装目录、Postgres.app 和已查 launchd 目录中未发现 native PostgreSQL；实际已有两套 **OrbStack Docker、Linux ARM64、PostgreSQL 17.10**。这不是全磁盘不存在其他安装的证明。

| 项目 | 当前 Rust 项目关联数据 | 历史 Python 数据 |
| --- | --- | --- |
| 既有容器 | `alia-postgres`，ID 前缀 `905470a16802` | `alia-python-postgres`，ID 前缀 `7365984e79e5` |
| 数据卷 | `project-alia_pgdata`，约 179.7 MB | `alia_python_postgres-data`，约 49.72 MB |
| 已创建容器的宿主端口 | **5433 → 5432** | **5432 → 5432** |
| 最近停止 | 2026-10-14，正常 shutdown／exit 0 | 2026-07-02，正常 shutdown／exit 0 |
| 容器 seed user／database | `alia`／`alia` | `alia`／`alia` |

两容器均停止，5432／5433 当前拒绝连接；只读取得 `PG_VERSION=17`、`global/pg_control` 等元信息，复核控制文件摘要未变。seed 值不证明当前 SQL 账号／密码；现有配置凭据与两套 seed 都匹配，不能据此决定数据归属。image 固定为 `sha256:306e00ca129081f4a69081c09ff0bdf0ef5169779019200a5c22ce9e0c6148f3`，详见 [PG 盘点](../../run/mac-production-readiness-20261025/postgres/summary.json)。

当前 Rust 卷在引擎内的路径为 `/var/lib/docker/volumes/project-alia_pgdata/_data`，Mac 可从 `/Users/ke-qiao/OrbStack/docker/volumes/project-alia_pgdata` 查看；底层 VM 映像位于 `/Users/ke-qiao/Library/Group Containers/HUAQ24HBR6.dev.orbstack/data/data.img.raw`。这是存储定位，不代表可用 macOS 原生 postgres 直接打开，也不能把复制运行中的 VM 映像当作一致备份。

**存在三处端口漂移：**旧 Server base／settings 指向 localhost:5432，当前 `docker-compose.yml` 也声明 5432，但已创建 Rust 容器实际是 5433，历史 Python 容器占用的映射才是 5432。不要直接 `compose up` 重建或新建空库来解决连不上。两容器当前 HostIp 为空、数据库监听 `*`；直接启动会恢复其原发布范围，不能假定只监听回环。

建议下一轮先以 **`project-alia_pgdata` 的停服隔离副本**为首选核对对象，原卷、原容器和 Python 历史卷均保留；明确授权后再准备一致副本，仅在回环／隔离网络启动副本并执行[只读目录查询](../../run/mac-production-readiness-20261025/postgres/01-readonly-catalog.sql)。确认表结构后才执行[账号／政策查询](../../run/mac-production-readiness-20261025/postgres/02-readonly-rust-identities.sql)，不启动 Alia 来代替查询，不触发自动迁移。实际账号、schema journal、embedding pin 和 Companion mode 未取得之前，生产目标仍未绑定。数据库启动自身会写运行状态／WAL，因此本轮“只读盘点”未执行这一步。

### 已有身份与不能沿用的 fixture

`alia-main` 的 `config/server.key` 有效，64 bytes、0600，公钥 SHA-256 为 `f1e259198c2ea443ed83a0ae2d35b7c8d76b4ee6a0f8690fa152a4acfbff95a4`。以保留原 Server 身份为默认，不生成替代密钥。只在内存派生公钥，未调用会 chmod 的应用 key loader，未保存私钥原值。

本机旧 `config/device.key` 对应 `alia-embedded-b40503784388`，但旧 Embedded TOML 含退休字段、`provisioned=false`；旧 SQLite 只有音量设置，没有 `binding_state`，也没有 R1 claim。10 月 21 日 Pi noop 和 10 月 24 日 R1 任务使用独立验证身份，不能升级为生产账号或绑定。现有 `pi-living-room` 仍缺 owner UUID、设备 pin 和可达 endpoint；后续从权威 SQL 与实际 Pi 持久状态核对。33 个配置／key／状态文件前后未变，见[身份报告](../../run/mac-production-readiness-20261025/identity/inventory.json)。

### Mac 常驻方式的选择

当前资产与进程都归 `ke-qiao`（UID 501），因此首轮建议沿用该用户的**登录会话服务**。已观察到 Magpie 127.0.0.1:3425、LM Studio 127.0.0.1:1234 和共享 GPT-SoVITS `*:9880`；独占 Juno 19880、Easy-Turn 8091 尚未运行。Magpie 的现有 tray LaunchAgent 处于 not running，但网关另有活动 PID，不能把 tray job 状态当作网关状态。

LM Studio 的 HTTP autoStartOnLaunch 已开，而桌面 enableLocalService 关闭；当前目录存在模型不等于已加载，也不证明无人登录开机可用。当前 Instant 已加载、embedding 未加载；JIT 与自动卸载前一个 JIT 模型的设置仍在。部署候选要求显式预加载两个选定实例，并检查 native `loaded_instances`／上下文，不仅 GET `/v1/models`。现有 `logSensitiveData=true`、verbose=true 应在获准应用设置时关闭；本轮只记录并准备变更，不修改原设置。

待安装的 Easy-Turn／独占 Juno／Server LaunchAgent、独立 Juno YAML、只读就绪检查及首次启动顺序见[服务材料](../../run/mac-production-readiness-20261025/services/README.md)。复用已有 Magpie／LM Studio 管理入口，不重复创建会竞争相同端口的服务。保留共享 9880，Juno 新配置只使用 19880；上游会写 YAML，运行副本必须在私有目录，不能指向旧共享配置或证据原件。

材料验证：3 份 plist lint、就绪判断 13 个 fixture 用例、Server 的 7 个拒绝／exec 拦截用例通过，均未启动服务。首次只读 HTTP 观察保留在 [readiness-current.json](../../run/mac-production-readiness-20261025/services/readiness-current.json)；[最终判定](../../run/mac-production-readiness-20261025/services/readiness-final.json)结合该观察与最终只读策略检查，未重复整套 HTTP，Juno 的实际 job／PID／YAML／权重分支因服务未运行而未执行。当前整套 ready 为 false，不能把静态通过写成服务已就绪。

首轮模型 jobs 按登录启动准备，Server 由操作者在就绪检查通过后显式启动。launchd 没有这些应用的依赖排序，Server 使用 `RunAtLoad=false`、全部新 job 使用 `KeepAlive=false`；保留首次失败，不自动重启求绿。下方系统 LaunchDaemon 模板仅描述 Server 的系统域运行方式，**不能证明整套 GUI 模型栈在无人登录时可用**。若选择开机无人登录运行，须另行落实 headless 模型服务、资产／凭据权限、OrbStack 启动和冷启动验收，不能直接混用两种运行域。

以上为10月25日盘点时的待选项；数据库来源、登录运行域和共享LM Studio策略以本页10月28日正式启用记录为准。参考文本和保留 `alia-main` 密钥无需重选，历史双模型驻留要求不再适用。

## Mac 与 Pi 的安装和常驻入口

以下是**通用部署模板**；本机 Mac 当前使用上节已安装的用户 LaunchAgent，Pi 模板仍待目标核对。先在部署清单填入双端主机、服务账户、候选 commit／产物 SHA-256、绝对版本目录、私有配置／状态／模型／日志／备份目录、监听地址和端口。复用已有绑定时沿用原身份和状态；首次部署才创建新身份。`DEPLOY_USER`、`CANDIDATE` 和 `/absolute/...` 都是待替换值，不是现场已确认路径。

1. 从[候选交付记录](software-release.md)选定同一源码身份的 Mac Server 和 Linux ARM64 Embedded，核对归档与解包后二进制摘要，放入各自独立版本目录。不要复制整个开发仓库或覆盖现有运行文件。Linux 按交付包的 `ldd` 清单核对依赖；两端都复核平台和可执行权限。
2. 私有目录由实际服务账户拥有，目录权限 `0700`、含凭据配置 `0600`；版本目录只用于代码，配置、SQLite、插件、R1 claim 和模型放在版本目录外。核对服务账户对目标 PCM／UART 的实际访问权限，不把整个设备目录设为全员可写。Server 的 PG／pgvector 和所选模型服务须先单独就绪；下列服务管理器不负责启动或等待它们。
3. 按下节准备配置，在 Pi 用候选二进制 `--check-config` 检查副本；核对 `speech.channel_id` 与 Server channel、设备公钥 pin、真实网络可达地址。Mac 的 `server.public_endpoint` 必须是 Pi 可达地址，不能把 `0.0.0.0` 当作对端地址。生产身份首次配对只在受控网络和人工核对目标后执行，见本页账号入口。
4. 首次正常启停、配对和模型链验收通过后，再启用常驻启动。保存实际服务文件及其摘要、所有非敏感环境变量和凭据来源名称；不要把 secret 写进下面的公共模板。生产日志与备份留在私有目录，不收集完整 settings 响应。

下方 Mac 系统级 LaunchDaemon 是**另选无人登录方案时的模板，不是当前安装方式**。它以选定的普通账户运行 Server，本身不依赖 GUI 登录，但不能让当前 GUI 模型栈或 OrbStack自动变成 headless服务；不能与当前同名 LaunchAgent同时安装。选择并验证该方案后，才保存私有待审副本 `com.alia.server.plist`，替换用户和路径并用 `plutil -lint` 检查：

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>com.alia.server</string>
  <key>UserName</key><string>DEPLOY_USER</string>
  <key>ProgramArguments</key><array><string>/absolute/releases/CANDIDATE/alia-server</string></array>
  <key>WorkingDirectory</key><string>/absolute/alia</string>
  <key>EnvironmentVariables</key><dict>
    <key>ALIA_CONFIG_PATH</key><string>/absolute/private/server.toml</string>
    <key>RUST_LOG</key><string>info</string>
  </dict>
  <key>StandardOutPath</key><string>/absolute/private/log/server.stdout.log</string>
  <key>StandardErrorPath</key><string>/absolute/private/log/server.stderr.log</string>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><false/>
</dict></plist>
```

仅选定并验证该备选运行域后，才将审核后的文件以 `root:wheel`、`0644` 放到 `/Library/LaunchDaemons/com.alia.server.plist`，先确保日志父目录存在。`sudo launchctl bootstrap system /Library/LaunchDaemons/com.alia.server.plist` 会立即启动；用 `sudo launchctl print system/com.alia.server` 核对 PID、退出状态，读取上面的两个日志。普通停止先 `sudo launchctl kill SIGTERM system/com.alia.server`，等待 PID 退出并记录清理结果；确认退出后 `sudo launchctl bootout system/com.alia.server` 卸载。禁用下次开机加载还需移走这一个 plist，保留私有副本；不要删除配置和状态。再次加载前核对数据库、模型与当前版本。LaunchDaemon 与用户 LaunchAgent 的登录边界见 [Apple 指南](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/CreatingLaunchdJobs.html)。

Pi 使用系统级 `alia-embedded.service`；这里的普通启动**不发送 R1 初始化命令**：

```ini
[Unit]
Description=Alia Embedded
Wants=network-online.target
After=network-online.target

[Service]
Type=exec
User=DEPLOY_USER
WorkingDirectory=/absolute/alia
Environment=ALIA_EMBEDDED_CONFIG_PATH=/absolute/private/embedded.toml
Environment=RUST_LOG=info
ExecStart=/absolute/releases/CANDIDATE/alia-embedded
Restart=no
KillMode=mixed
TimeoutStopSec=60
SendSIGKILL=no
UMask=0077
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
```

下一轮用目标 Pi 的 `systemd-analyze verify /absolute/alia-embedded.service` 检查替换后的文件，再安装到 `/etc/systemd/system/alia-embedded.service`、`sudo systemctl daemon-reload`。`sudo systemctl start alia-embedded` 才会启动设备；首次验收后 `sudo systemctl enable alia-embedded` 才设置开机启动。诊断使用 `systemctl status alia-embedded`、`systemctl show alia-embedded -p MainPID -p Result -p ExecMainCode -p ExecMainStatus` 和 `journalctl -u alia-embedded --since '本轮开始时间'`。正常停止用 `sudo systemctl stop alia-embedded`；禁用开机启动用 `sudo systemctl disable alia-embedded`。60 秒是服务管理器的停止观察上限，不修改应用或测试期限；超时仍记录失败，不以自动强杀或重启隐藏资源未清理。`KillMode=mixed` 先让主进程负责子任务关闭，`SendSIGKILL=no` 保留未退出进程供定位，后续启动前必须确认它们已经退出。语义见 [systemd.service](https://www.freedesktop.org/software/systemd/man/latest/systemd.service.html) 与 [systemd.kill](https://www.freedesktop.org/software/systemd/man/latest/systemd.kill.html)。

两个模板均不自动重启失败进程；初次生产验收先保留原退出与 Unknown 状态，后续若启用重启策略，应连同故障恢复一起验收。系统服务也不会防止 Mac 休眠，24 小时验收需同时记录实际供电／休眠策略和日志保留上界。不要为了长稳验证临时增大应用期限。

R1 系统模式在**确认真实板端冷启动后**，需要用同一个长期运行进程执行一次 `--initialize-r1`，不能先运行初始化然后另启一份普通 runtime。采用上述 systemd 服务时，可在已停服且设备 owner 已释放的现场窗口创建唯一的临时 `/run/systemd/system/alia-embedded.service.d/initialization.conf`：清空 `ExecStart=`，再指定同一候选二进制的 `ExecStart=/absolute/releases/CANDIDATE/alia-embedded --initialize-r1 /absolute/private/embedded.toml 已确认冷启动的新UUID`。`daemon-reload` 后只启动一次；保存初始化结果和 claim，再移除这一个临时 override 并 `daemon-reload`，**不重启正在保持采集流的进程**。失败或结果未知也先移除 override、保留 claim 和错误，不能重发。普通重启使用无参数模板；机器重启／设备断电不自动获得重新初始化许可，必须先诊断并由操作者确认冷启动。完整条件见 [R1 启动契约](../../packages/alia-embedded/docs/audio-and-voice.md#r1-system-controlled-capture)。

采用 `1a7a118` 的两阶段实现时，显式初始化会在任何模块 start 前先持久化 claim 并持有 UART；实际 PCM 校验通过后才发送协议命令。后续模块启动失败也保留该 claim，不自动同 ID 恢复。一次成功初始化后的同轮窗口只切换 CaptureControl Start／Stop，不在窗口之间停进程、换 PID 或关闭 native capture；否则原持续流声学证据不能直接沿用。该规则不把新实现的软件验证当作已完成的真实断电恢复验收。

停服顺序为 Server → 确认其退出 → Embedded。记录每端 exit status、子进程、原监听端口、数据库连接及 PCM／UART owner；监听关闭或服务 inactive 单独不足以证明清理完整。只有双端和同库写入者全部停止后才执行[一致备份与恢复](upgrade-recovery.md)。实际配对恢复、开机／普通重启／设备冷启动、升级恢复和 24 小时混合负载均须在下一轮生产部署中验证，本页模板静态检查不替代这些结果。

## 配置与启动

从仓库根目录按 [本地开发](local-development.md) 准备主文件和 `.d` 分片；[Server 配置](../../packages/alia-server/docs/configuration.md)与 [Embedded 配置](../../packages/alia-embedded/docs/configuration.md)分别管理业务/模型和设备实时运行时。`ALIA_CONFIG_PATH` 可以选择 Server 主文件，配套分片目录为同名 `.d`；Embedded 使用独立的 `ALIA_EMBEDDED_CONFIG_PATH`。

正常启动会产生实际副作用。Embedded 可能创建或调整设备密钥权限、保存配置、创建/迁移 SQLite、绑定监听地址，然后在 `Builder.build()` 返回前启动选定的播放、motion、camera、capture 和 discovery。未配对不代表麦克风尚未开始采集；`motion.hardware_enabled=false` 只限制执行器，不能关闭其他设备。通用 [embedded.example.toml](../../config/embedded.example.toml) 是配置说明示例，其中原生采播、模型、vision 和发现仍按模块选择启用，不能直接当本轮 Pi 现场预设。

已有二进制提供独立的只读检查入口，必须指定实际存在的配置文件：

```sh
./target/debug/alia-embedded --check-config /absolute/path/embedded.toml
```

此入口只读配置，以及配置指向的校准文件和已批准动作文件，执行静态验证；不创建/调整 key，不进入 Builder、不打开数据库、模型、PCM、camera、串口或 I²C，不执行绑定、网络发现或启动服务。缺少配置明确报错，不将 compiled defaults 当作已检查的文件。报告指出配置选择与前置条件，不能证明真实设备、模型、机构安全或目录写权限已经可用，也不核对 Server 的批准清单。构建二进制本身会写入构建目录，应与这次只读运行区分；不要用普通 `cargo run` 或视觉 setup 脚本代替预检。

如需后续验证无设备的正常生命周期，使用专门的 [embedded.staging.example.toml](../../config/embedded.staging.example.toml)，先复制到新的私有配置目录，再检查复制后的文件。模板显式关闭全部 module 槽位、motion、vision/debug、毫米波和 mDNS/BLE，监听 `127.0.0.1:0`，保留正常绑定边界；state、key、plugin、cue 使用相对配置文件的独立 `staging/` 路径。不要与已部署设备共用这些目录。正常 staging 启动仍会写 key/state/config 并创建 loopback listener，**不属于只读操作**；它也不检验真实录放音或执行器。

```sh
ALIA_CONFIG_PATH=/absolute/path/server.toml cargo run -p alia-server
ALIA_EMBEDDED_CONFIG_PATH=/absolute/path/embedded.toml cargo run -p alia-embedded
```

上面是正常运行命令，现场前置未完成时不执行硬件配置的启动。staging 需要正常运行时，应把第二行路径明确指向复制后的 staging 文件。使用该前台进程的 Ctrl+C 或针对其 PID 的 SIGTERM 请求正常关闭，并核对进程退出与清理结果；超时不证明 native 工作或物理输出已安全停止，不使用按名称批量 kill 或顺带停止共享服务。

2026-10-17 已在既有 Linux ARM64 release 上完成隔离全 noop 的两轮实际启停与恢复，见[验收与恢复检查点](../refactor/resume-checkpoint-release-lifecycle-2026-10-17.md)。联调使用私有loopback端口和任务数据库；Server选择noop模型、禁用Easy-Turn/HA且保留默认政策，现有channel内部mDNS browse仅在隔离网络内。首次Embedded启动会按新密钥推导device ID并写回配置，即使仍未provisioned；重启应保留该配置、key和state，以首次正常关闭后的身份为基线。Server持久settings也需保留；noop模块的`ignored_runtime`只表示保存了期望值，不能当作模型已应用。此次通过不覆盖真实服务账户部署、绑定恢复、异常强杀/断电或硬件/模型。

仓内 `pnpm dev:backend` 会同时启动开发数据库、Server 和 Embedded，`pnpm dev` 还包含前端；这些便利入口不负责本轮隔离目录、设备权限或测试资源归属。`scripts/setup-embedded-vision-pi.sh` 是安装/模型/摄像头配置工具，可能安装依赖、下载文件和查询设备节点，`--run` 会启动完整 Embedded；它不是无设备只读预检。Server 普通配置加载也可能执行已有发布恢复；不能把正常 Server 启动称为纯只读解析。

未找到 Server 配置时进入 OOBE；配置已存在但不能加载时会报错退出，不自动改写成样例。stdin/stdout 都是交互终端时，输入 `c` 再按 Enter 可生成主文件加 `.d` 的完整布局，该操作拒绝覆盖已有主文件或分片树。

setup 固定监听 `127.0.0.1:3000`，此时尚未使用主配置的 Server 端口，不在局域网广播未认证的配置写入口。远程机器可使用 SSH 隧道；无交互终端时使用 HTTP setup。两个配置操作均为 POST：`generate-config` 返回 `{template: TOML字符串}`，`save-config` 接受 `{config: TOML字符串, path?: 目标主文件路径}`。`path` 指向 **Server 所在机器**的文件系统；省略时先采用该进程的 `ALIA_CONFIG_PATH`，然后按 CLI 默认路径选择。

setup 只接受 `127.0.0.1`、`localhost` 或 `[::1]` 的 Host；浏览器带 Origin 时必须与 `http://Host` 精确一致，否则返回 403。保存必须使用 JSON Content-Type，错误媒体类型返回 415。无 Origin 的本地 curl/SSH 隧道按现有方式工作；不要把入口放到任意域名的反向代理后。该策略约束浏览器来源，仍信任本机操作者，文件可写范围由服务账户权限决定。

HTTP 模板是包含所需分片内容的完整单文件，不会额外创建 `.d`；已有目标 `.d` 仍按普通加载规则覆盖主文件。保存前校验实际有效的 role/profile 引用，成功后退出 setup 并按配置启动 Server。以下命令需要 `jq`，可在第二个终端执行：

```sh
umask 077
curl --fail --silent --show-error -X POST \
  http://127.0.0.1:3000/api/v1/setup/generate-config \
  | jq -er '.template' > setup-server.toml
# 编辑 setup-server.toml，填写数据库、模型地址及密钥。
jq -n --rawfile config setup-server.toml \
  --arg path /absolute/path/server.toml \
  '{config: $config, path: $path}' > setup-request.json
curl --fail --silent --show-error \
  -H 'Content-Type: application/json' --data-binary @setup-request.json \
  http://127.0.0.1:3000/api/v1/setup/save-config \
  | jq -e 'if .success then {success, path} else error(.error // "setup save failed") end'
```

`save-config` 的正常 JSON 响应包含 `success`、`path`，失败时另有 `error`。配置验证或保存失败也可能返回 HTTP 200，因此必须检查 `success`；仅用 `curl --fail` 不够。发布开始前的失败允许修正后重试。发布已开始但同步失败时，错误会明确持久化仍为 unknown；setup 保留原发布，只接受同一路径、内容和安装方式的重试，不能换请求覆盖。只有持久化确认后才进入 Ready；成功响应排空后 setup 关闭，其端点不作为常驻管理 API。准备阶段的 `/health` 返回 `status: "setup"`，不是模块健康结果；启动完成后使用配置中的实际 Server 地址和端口。

同一 setup 进程只有一次成功完成：并发 HTTP/CLI 的输家返回失败且不写第二份配置；取消后，即使已接收请求随后才补齐 body，也不会发布。base、分片树及完整 canonical 祖先目录链按发布协议同步；若发布已开始后失败，不删除 base 或 CLI 分片，而是保留私有的相邻恢复记录。记录只含路径和内容摘要，不含配置值或密钥。CLI 按 `c` 重试原安装；HTTP 重发原请求；取消保留磁盘文件。重启时选择相同配置路径（自定义路径用 `ALIA_CONFIG_PATH`），加载器会先恢复再加载，正常无记录时仍纯只读。

恢复发现 base、候选或分片内容改变/丢失时会拒绝盲覆盖，应保留文件与报错交由操作者核对，不要只删恢复记录强行继续。恢复记录建立前崩溃可能留下需人工核对的临时文件/CLI 分片；也不能同时让其它进程修改正在校验的文件树。实际断电、存储硬件与服务账户部署权限仍需独立验证，见 [本轮发布与恢复说明](../refactor/config-durability-2026-10-11.md)。

配置语义有效不等于数据库、模型或硬件已在线。先准备独立 PostgreSQL/pgvector 和选择的 [模型服务](model-services.md)，再核对 `/health`；不要以 HTTP 200 或非空 API key 判断所有 Provider 可用。

## 账号与现有管理入口

通过 `POST /api/v1/auth/register` 建立账号，`POST /api/v1/auth/login` 获得 token，后续请求使用 `Authorization: Bearer …`。首个账号的管理员分配规则以 [认证实现](../../packages/alia-server/src/api/auth.rs)为准；不要在命令历史中直接放入真实密码或 token。下列示例假定 `ALIA_URL` 和 `ALIA_TOKEN` 已指向本人的隔离开发实例。

```sh
curl --fail --silent --show-error \
  -H "Authorization: Bearer $ALIA_TOKEN" \
  "$ALIA_URL/api/v1/users/me"
curl --fail --silent --show-error "$ALIA_URL/health"
```

| 用例 | 已有入口 | 权限与结果边界 |
| --- | --- | --- |
| 渠道/设备发现与绑定 | `GET /api/v1/channels`，`GET /api/v1/device?channel_id=…`，`POST /api/v1/device/bind` | 使用明确的 Embedded channel，人工核对发现结果及设备公钥 pin；发现不代表绑定完成。 |
| 设备状态与设置 | `GET /api/v1/device/status?channel_id=…`，`GET/PATCH /api/v1/device/settings?channel_id=…` | 设备 owner 或当前管理员；缺少遥测保留 null，网络失败不伪造成已观测的离线数据。 |
| 解绑与重连 | `POST /api/v1/device/unbind`，`POST /api/v1/channels/{id}/restart` | 重启渠道需管理员；解绑退休原 session，重新绑定使用新凭证/session，不复用旧权限。 |
| 主动策略、shadow/回放 | 四个 Companion REST 操作 | 见 [Companion 操作](companion.md)；政策完整 CAS，503 必须核对原意图，默认 shadow。 |
| 记忆/隐私/检索配置 | `/api/v1/memories/*`，`GET/PATCH /api/v1/settings` | 见 [Memory 操作](memory.md)；账号归属、来源撤销、持久 revision 不因重试刷新。 |
| 人格与关系状态 | `/api/v1/personas/me`，关系读取/清理/重建接口 | 见 [Relationship 操作](relationship.md)；清除需原 operation/revision，重建不能复活已删除来源。 |
| 工具执行/确认/取消 | `/api/v1/tools/execute`，`/api/v1/effects/{id}`，`/{id}/confirm`、`/{id}/cancel` | 从 `/scalar` 的 execution 契约取得完整请求；保留原 effect identity/version，Unknown 先核对，不再次调用非幂等工具。 |
| Response 与文本/音频/身体结果 | `/api/v1/responses/{id}`、`/{id}/deliveries`、`/{id}/events` | 见 [Delivery API](../refactor/delivery-api.md)；Submitted、Presented、位置反馈和完成分别记录，查询不是重播。 |
| 配置/模块健康 | `/health`、`GET /api/v1/settings` | 检查每模块状态；默认 probe=false；OpenAI/Gemini 在线 GET 需显式启用、单次 5 秒。整体一轮最多 6 秒、8 项并发；busy/disabled 均不能当作在线通过。 |

`GET /health` 保持公开 HTTP 200 和 `status`、`version`、`modules` 响应字段，仍需检查各模块状态。相同配置 LLM profile 的多个角色共享一次检查；Router 只报告本地模式和依赖绑定，模型状态见对应 `llm.*`。同一 Registry 有一轮检查进行时，重叠请求立即返回 `degraded` / `health check already in progress`，不排队、不复用旧结果。6 秒整体期限到达后保留已完成状态，未完成项返回 `degraded` / `health check timed out after 6s`；停机时未完成项返回 `server is stopping`，单项 panic 返回 `unhealthy` / `health check panicked`。后续请求在空闲时重新检查。

这些期限约束合作式异步检查；Server 不启动后台探测，也不能保证已发出的远端请求在远端立即停止。设置更新不等待健康网络探测，已经发出的探测使用其开始时的配置快照。没有新增深度诊断端点或 probe 热切换开关。

四个 Companion 操作和 execution/delivery 契约可在 Server `/scalar` 查阅。其他旧模块的 OpenAPI 覆盖不应由这个页面存在就自动推断；对应 Markdown 和实际 REST 用例仍是操作依据。

`GET /api/v1/settings` 在全部设置快照等待结束后、返回 JSON 与政策 ETag 前核验当前 active admin。等待期间已降权、停用或删除为 403；最终核验失败或超过 2 秒为脱敏 503，均不返回设置或 ETag。成功响应仍保留原值/schema 与 prefix 行为，包括管理员可读取的 secret 值。原 SQL 暂停到期与内存政策投影保持，响应准入后不持续持有用户锁。

纯非政策 `PATCH /api/v1/settings` 会在更新锁、政策读取和 embedding 预览结束后，重新核验当前 active admin。等待期间已降权、停用或删除返回 403；核验数据库失败或等待超过 2 秒返回脱敏 503，且不应用或保存本请求设置。已经准入的操作继续沿用部分成功规则，不因随后降权自动回滚。

运行时 `PATCH /api/v1/settings` 有 SQL 政策提交、期望设置更新、模块生效和文件保存几个独立完成点。HTTP 200 中 `success=false` 不表示此前动作已回滚，应核对 `context_policy_version`、`applied`、`module_reports` 与 `errors`。同值显式重试仍会调用模块并保存。`settings.toml` 通过完整快照原子替换；发布前失败保留旧文件，发布后同步失败明确报告持久性未确认。失败后直接重启只加载磁盘上完整的旧版或新版，SQL 仍是政策权威；此路径没有首次 setup 的恢复记录。详见 [运行时设置说明](../../packages/alia-server/docs/configuration.md#runtime-settings)。

`PATCH /api/v1/memories/privacy` 现要求管理员，提交同一全局 SQL 政策并支持相同 `If-Match`；普通登录用户仍可读取隐私状态。隐私 GET 与 audit 读取实际政策，不再返回旧内存投影。该权限收敛已由用户确认。

绑定请求应沿设备实际发现结果填写 `channel_id`，保持 `require_confirmation=true`。该字段要求 Server 有待确认的发现候选，**不代表设备侧已实现物理按钮确认**；当前 `discovery.require_physical_confirm` 没有实际设备确认处理。首次绑定必须先在受控网络核对目标设备、公钥 pin 与 Server 身份，不能把配置布尔值当作防抢绑保证。请求 JSON 为 `{"channel_id":"实际渠道ID","require_confirmation":true}`；若 Server 使用 wildcard 监听且未配置 `server.public_endpoint`，还需给出 Pi 可达的 `server_endpoint`。成功后检查响应 `status="bound"`，再用受保护的 `/api/v1/device/status?channel_id=…` 和设备 settings 查询核对原 device ID 与绑定。已有绑定正常恢复时不发送 unbind/bind；身份不匹配先保留日志定位，不删除 key／SQLite 来重新配对。这项软件接口检查仍不等于真实 Pi 的麦克风／声卡和执行器已经验收。

## 诊断与恢复

ExplicitWait 只响应新的明确 Continue；临时避让可由新鲜 Backchannel 恢复。恢复使用原 Response、原 generation 和尚未提交的音频后缀，不调用新 TTS 重建前缀。协议要求 voice v2、playback v3，主动邀请额外要求 capture gate v1，见 [语音协议](../refactor/voice-protocol.md)。

数据库或网络错误可能使写入完成状态未知。读取原政策、operation、Response、Delivery 或 Effect 核对，不换身份推测性重做。`cancel_requested` 不是设备已停声/机构已到位；最终还需原 native owner 与设备事实。

本包验证使用独立临时配置、本地受控 Provider 和隔离测试数据库。实际安装、配对及 24 小时混合负载仍归 [R02—R06 checklist](../refactor/checklists/next-round.md)。Rig 模型／工具构建、rmcp 客户端和 Extism WASM 已接入，现行操作见 [MCP](mcp-tools.md) 与 [WASM](wasm-plugins.md)；具体外部服务、模型和硬件仍须各自验收。
