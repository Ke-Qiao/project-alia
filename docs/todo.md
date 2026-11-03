# Project Alia TODO

更新：2026-11-04。**默认关闭语音打断已获用户现场确认；本轮两条实际输出都完整提交，播放线程保持。** Mac d7126a9／PID5817、Pi 588c434／PID8106未变，扬声器保持最大设置，麦克风未改。第一条回答出现当前输入里没有的“无尽冬日”；限定历史／依赖查询未找到该词的来源，完整最终提示词未落盘，不能直接归因旧历史或缓存。两条均直接走major、cache_miss、未执行工具。后段未完成音频累积超过原30秒上限，Server结束会话；同boot generation5 inactive，控制器未发送旧代次Stop，原错误保留。本轮不再重复讲话，下一步先接收用户的新回答流程要求，并处理内容来源和片段超限后断整场的问题；最大音量听感与第二条完整耳听尚待用户补充。见[本轮结果](operations/model-services.md#默认不打断现场复验与待改回答流程2026-11-04)、[完整日志](../run/voice-preferences-20261104/next-window/live-01/acceptance-complete.private.log)及[既有延迟报告](../run/voice-acceptance-20261104-followup/next-window/live-01/latency-review.md)。

最新实现：[架构概览](architecture/overview.md)、[Embodiment](architecture/embodiment.md)、[工具构建](concepts/skills-and-tools.md)、[MCP](operations/mcp-tools.md)、[WASM 插件](operations/wasm-plugins.md)。验证结果统一引用[软件发布记录](operations/software-release.md)，不将不同源码、平台或重复测试相加为项目完成率。

## 交付前待办

- [x] **S01 · 构建并验证 Embodiment 重构后的宿主发布产物。** 10 月 20 日统一 release 入口通过；独立新产物完成全 noop 首次绑定、正常关闭和同身份恢复，四进程 exit 0、端口关闭、数据库连接归零。源码清单、产物摘要、清理判定器错误及独立核验均保留。Unknown 不重派由 workspace 的操作恢复回归覆盖；双进程演练没有制造物理操作。见[当前发布记录](operations/software-release.md#当前发布出口2026-10-20)。
- [x] **S02 · 补齐当前源码的 Linux ARM64 构建与运行证据。** Rust 1.96 下默认完整 workspace、两端 no-default all-targets、default release 与 native 链接检查通过；新二进制全 noop 真实绑定及两轮恢复通过，四进程 exit 0。任务容器、数据库及内部网络已清理，产物和依赖身份已保存。见[当前发布记录](operations/software-release.md#当前发布出口2026-10-20)；这不是 Pi 部署或音频验收。
- [x] **S03 · 准备成套升级与恢复步骤并完成隔离演练。** [操作手册](operations/upgrade-recovery.md)覆盖逐文件迁移、身份／配置／资产备份、插件重新授权和失败恢复；旧配置拒绝、历史记录保留、旧批准失效由本轮完整回归覆盖。停服后 pg_dump／pg_restore 的 67 张表及 9 个私有配置／状态文件核对一致。真实部署升级与旧版回滚仍属于 R05，不把该隔离演练视为现场通过。

- [x] **S04 · 当前软件源码与双平台发布包整理交付。** 10 月 24 日按当前工作区重新构建 macOS ARM64／Linux ARM64 默认 workspace release，冻结双端二进制与 Rust PDK guest、记录源码和产物哈希；原生链接检查通过，Linux staging／R1 自然／R1 系统三份配置离线预检通过。迁移说明补齐 Silero 配置与 R1 初始化 claim 保留；交付包和分层验证见[本轮软件交付](operations/software-release.md#alia-软件交付与-r1-阶段收口2026-10-24)。本轮不重复全量回归或硬件验收，新包未部署。

- [x] **S05 · 固定本地软件候选与最终源码软件验证。** 修复 WASM 撤权持久重试、handoff 抢绑／重启恢复、验证 fixture 路径和前端构建；补齐常驻部署与模型输入材料。源码 A 单次 workspace 为 Mac **1388/0/10**、Linux **1413/0/12**，两端 no-default 和 release 通过；后续 Delivery 等价标签块修正的最终 B，两平台各 **56/0/4** 影响验证、Server no-default 与 release 均通过，Clippy 通过并保留警告。源码、锁文件、本地提交、双平台产物和独立候选归档由[候选记录](operations/software-release.md#本地软件候选与最终源码验证2026-10-25)及交付清单固定；A/B 分母不相加，旧交付与原始失败保留。该候选轮没有推送、合并、正式 Release 或生产部署；10月28日后续Mac启用及定向修复单独记录，不改写原候选证据。

S01—S03 是 10 月 20 日的交付与部署出口，该轮没有新增生产代码变更。10 月 20 日 macOS 单次完整 workspace 为 1329 passed / 0 failed / 5 既有 ignored，Linux ARM64 为 1337 / 0 / 5；分别统计。10 月 19 日的 fixture 首轮失败与按目标汇总继续保留为历史，详情见[验证记录](operations/software-release.md)。10 月 21 日的 R1 改动与定向回归另行记录，不冒用旧 workspace 结果。

## 当前推进

已整理交付正确性修复、Rig／工具构建／MCP／Extism／fetch、Embodiment 和 R1 主机接入。软件范围见下方“已完成的软件范围”，配置切换遵循[升级与恢复](operations/upgrade-recovery.md)，产物和验证分母见[软件发布记录](operations/software-release.md)。R1 不再安排独立重复现场测试；V01／V02 按已有恢复条件保留，不为收口反复重跑求绿。

- [x] **R02 现固件 Rust 主机接入与长句处理**：自然唤醒 UART／ALSA／签名 VoiceSession、固定状态 Silero 和正常长句端点已落地；真实长句取得 24.98 秒活动／25.68 秒窗口，修复后的暂停／恢复／取消／重新唤醒由独立短入口通过，−14 dB 音量获用户确认。两次独立通过不写成同一完整用例。见[长句与控制记录](operations/software-release.md#r1-自然长句现场复验2026-10-23)。
- [x] **R02 系统控制采集软件与真实 Server 链路**：显式冷启动 ID／持久 claim 防重、单次初始化、签名 CaptureControl、boot／代次隔离及原生退休已实现；完整 Embedded 回归 macOS 389/0/1、Linux ARM64 414/0/3，最终 CLI 增量各 6/0。最新 Linux duplex 40/0/4，自动 Pi 现场三轮 Start／Stop＋同身份普通重启 1/0、29 秒；无语音活动，仅证明控制与传输。见[软件接入](operations/software-release.md#r1-系统控制采集软件接入2026-10-24)和[现场记录](operations/software-release.md#r1-系统控制传输与普通重启现场验收2026-10-24)。
- [x] **R02 单次初始化硬件基线**：cold-03 在当前固件／beam 0／位置下保存 395.56 秒同流 PCM，包含 300.02 秒静默，前后语音获用户确认完整清楚。辅助 reference 缺少一块 20 毫秒副本的限制保留；不外推其他方向、全天运行或 AEC。见[独立实测](architecture/r1-system-controlled-capture-research-2026-10-23.md#71-2026-10-24独立冷启动-cold-03-实测)。
- [x] **R02 当前阶段收口**：用户确认结束独立 R1 补测，直接复用上述证据；验证进程／采播设备、临时容器／网络／隧道已释放，音量保持 −14 dB。真实模型完整语音链统一归 M01，AEC 归 R02-AEC；不再单列要求重复插拔、beam 0 讲话、试听或零模型长句组合。
- [x] **R05 前置 · 真实 Mac Server + Pi Embedded 无设备验证**：10 月 21 日隔离全 noop release 两轮首次绑定／正常关闭／同身份恢复通过，四进程 exit 0，正常鉴权和持久化一致，资源已清理。该前置不等于常驻部署、真实模型或 24 小时验收，见[原始记录](operations/software-release.md#mac--pi-实机无设备验证2026-10-21)。

## 保留跟踪的异常

- [ ] **V01 · 历史 Companion 配对超时根因。** 旧 25 秒失败没有足够阶段证据；10 月 22 日 Linux 完整 duplex 中又出现既有 H1→H2 场景等待 Resume 的 3 秒超时，新 R1 场景在该轮通过。同一 binary 定向诊断通过，追加失败专用状态诊断后一次完整目标 32/0；均不能倒推根因已解决或认定与旧 25 秒失败同因。仅在新证据出现时有界定位，保留原期限、并发与失败，不无限复跑求绿。扩大无人值守 live 前需根因证据或明确接受已有边界；默认 shadow 保持。见[本轮记录](operations/software-release.md#r1-长句与真实-server-采播软件闭环2026-10-22)、[历史检查点](refactor/resume-checkpoint-v01-server-startup-2026-10-15.md)及[历史台账](refactor/deferred.md)。
- [ ] **V02 · Harness 验证中未定位的时间准入／数据库截止异常。** 10 月 19 日最终统一整套已通过，但两次独立诊断的失败原因未确定；与 V01 分开保留，不把休眠或系统代理扫描统一解释为所有失败的根因。出现新证据时复用原日志有界定位，见[Harness 验证记录](operations/software-release.md#harness-迁移验证2026-10-19)。

- [ ] **V03 · 数据库暂不可达后的恢复生命周期。** 已在真实PG隔离复现并修复Execution／Delivery安全巡检失败永久停止、Memory扫描健康不恢复的问题，保留真正派发／receipt／投影写入的失败边界；Mac逐目标回归和双平台构建已留证。原合盖／DarkWake与数据库不可达的底层原因仍未证明，新候选因V05暂不部署。11月2日旧Server停止仍exit1，但六类执行／输出账本前后均为空、连接归零；不把重启后的健康当正常关闭或自动恢复通过。见[软件修复记录](operations/software-release.md#数据库恢复修复与候选准备2026-11-02)。
- [ ] **V04 · Easy-Turn真人输入的原5秒门槛。** 已通过同样本单次前台／后台冷请求对照定位调度成本，仅将LaunchAgent的ProcessType改为Interactive；生产重载后complete／backchannel／wait为1535／384／436ms，原5秒、并发、权重、线程和生成参数不变，无预热。10月31日未知现场输入与原冷样本5.961秒失败保留。11月2日第二现场窗口17个输入分段的native最大1333.747ms，未观察到turn-model deadline；用户已确认该窗口没有讲话，指定短句测试未执行，声源未由PCM确认。不是17条独立验收，也不能关闭识别／换气长句门槛。见[现场记录](operations/model-services.md#受控真人窗口与输出取消2026-11-02)和[调度修正](operations/model-services.md#easy-turn-调度定位与实际修正2026-11-02)。

- [ ] **V05 · Linux暂停后Resume回执／SQL行锁等待。** 本轮两次完整duplex目标均39/1/4，同一held-before-first-start用例在原3秒内缺少Resume回执；第二次确认native仍Held、wire无Resume且随后cleanup失败。同轮PG有Response行锁statement timeout，但缺少确切blocker／case对应。最后一次加入失败后SQL诊断的完整目标40/0/4，observer未触发；无生产修复，根因仍未定，不合并解释V01／V02。保留默认并发和所有首错，新Rust候选暂不作为通过版本部署。下一次只在具体新假设／证据下定位，不为求绿重复整套。见[原始结果与边界](../run/software-recovery-20261102/linux-diagnostic-sql/summary.json)。

- [ ] **V06 · VoiceSession Ready 前上传终止。** 11月3日第一次 Start 后约0.703秒、Ready前出现合并的 Full／Closed 错误，无法回溯分型。已补最小诊断元数据并完成双平台定向验证；独立 `d392401` 只携带该增量，已安装到 Pi。随后唯一一次无讲话检查取得 Ready，并在约0.795秒内完成 Start／精确 Stop ACK，没有复现故障；**不因此关闭根因**。原容量、期限、并发和首错保持，不重复现场求绿。下一次真人窗口直接复用诊断件，若再失败按具体 Full／Closed 和同窗日志有界定位。见[原失败](operations/model-services.md#voicesession-ready-前上传终止2026-11-03)与[诊断启动](operations/model-services.md#诊断版无讲话启动核对2026-11-03)。

- [ ] **V07 · R1 冷启动版本握手超时与免唤醒恢复。** UART准备已前移，修复版58bb11e在用户确认的新冷启动中一次完成版本查询和beam0命令，原期限不变；PID4532的原生采集保持至本轮升级正常停止。旧版本超时和错误关闭流的记录保留，不把一次初始化成功写成日常断电、USB重连均已恢复。后续指定句已正确识别，feedback04的无声已定位为独立播放worker退出，不能继续归因R1输入。138ee28普通重启未发UART，实际指定句识别与用户可闻回复已取得；后续ALSA underrun属于输出错误；3bd88c8连续窗口第一条提交完整、第二条仍EPIPE，588c434双端复验已取得连续对话和主动打断反馈，播放线程保持。不能因此前单次成功认定日常断电／USB重连全部恢复；连续输出按M01继续验收。见[当前结果](operations/model-services.md#播放后端退出与显式-start-恢复2026-11-03-晚)和[初始化软件修复](operations/software-release.md#r1-冷启动顺序修复2026-11-03-晚)。

## 待输入或明确恢复后推进

| ID | 任务与当前状态 | 恢复条件与完成标准 |
| --- | --- | --- |
| M01 | **不打断已现场确认；待用户回答流程要求，处理内容来源和长片段断会话** | 最新Mac d7126a9／Pi588c434本窗两条实际输出均End=true、submitted，播放8113／采集8114保持。用户确认插话未打断；实际输入偏离指定脚本，不能代填指定第二问通过。第一条出现无来源记录的“无尽冬日”，未命中缓存／未执行工具；完整最终messages未落盘。未完成音频累计超过30秒后整场被关闭，原错误和generation5 inactive保留；不延长上限。先按下一轮用户要求明确上下文、分流和回复时序，再针对反例修正并集中真人验收。最大音量听感、正常换气长句、取消／恢复／下一轮和完整Start／Stop仍有未验项。见[本轮结果与日志](operations/model-services.md#默认不打断现场复验与待改回答流程2026-11-04)。 |
| E01 | **ROS 与具体具身适配器暂缓**：Embodiment 地基已完成，默认没有实际物理适配器 | 用户指定首个设备、通信协议／ROS 版本和目标能力后，实现 `PhysicalAdapter`；核对设备身份、反馈、停止证据、迟到请求终结、断线／重启和共享执行器仲裁。先仿真再实机，沿用 Execution；不扩大 expression 的设备权限。见[接入契约](architecture/embodiment.md)。 |
| R02-F | **R1 固件改造由用户明确暂缓**：新旧资料、镜像及两份不同虚拟机已核查，未找到匹配板端 SDK；未开发、构建或烧录新固件 | 用户明确恢复后，再取得匹配的最小板端应用 SDK／ABI，或满足契约的受支持常开模式，并补齐当前 1.9.6 恢复路径。核查与候选方案见[硬件记录](operations/hardware-deployment-readiness.md#r1-通用固件工作2026-10-21)，[商家资料清单](operations/r1-firmware-vendor-inputs.md)仅保留为参考，不继续调查或索取。 |
| R02 | **当前软件接入与分阶段验证已收口；整体验收按条件恢复** | 真实模型整链统一归 M01；AEC 归 R02-AEC。其他方向／安装标定及长稳在实际部署时随 R05 明确范围，不默认重做已通过的 R1 基线。模块正常，R02-F 暂缓不阻断 Alia 主线；FYP ROS 验收不等于 Alia ROS 接入。见[硬件资料](operations/hardware-deployment-readiness.md)。 |
| R02-AEC | **AEC 单独等待商家确认** | 商家明确当前固件、模拟参考链及支持的配置后，再安排回声、双讲、打断期间参考与物理尾音验证；真实模型整链成功不自动关闭本项。 |
| R03 | **执行器现场工作由用户暂停**：Motion 软件契约与资产身份已完成 | 用户恢复后补板卡、供电／OE、通道、限位／校准、双端资产审核和现场反馈；验证停止保持、失联／重启、迟到命令。主机写入、取消 ACK 或 Unknown 退休都不算物理停止。见[现场门槛](operations/hardware-deployment-readiness.md)。 |
| R05 | **`.local`签名迁移／noop普通重启通过；完整升级恢复与长稳仍开放** | Mac当前监听0.0.0.0、public endpoint为Ke-QiaoMac.local；Pi .221解析及持久绑定／公钥／claim通过。11月2日迁移前新PG恢复点隔离还原67表一致，Pi快照只读完整性／pin／claim一致；迁移后恢复点见操作记录。仍需实际应用级成套恢复、resolver整机重启持久性、登录／异常／休眠恢复及至少24小时断网、模型超时、设备重连和资源上界。Pi仍disabled／Restart=no，Mac受控登录会话，手动恢复不等于无人值守。执行器按R03暂缓。 |
| I01 | **具体生产 MCP 服务／第三方插件验收待目标**：客户端、WASM 运行器与 Rust PDK 已交付 | 选定首个服务／包后，按实际凭据主体和版本验证目录变化、授权／撤权、未知结果及清理。现有回环 MCP 与真实 WASM 测试不证明任意生产服务、任意包或 Pi 容量已可用；无需重建通用 harness。 |
| D01／D02／D04 | **Web／iOS／Android 提升及随前端延期的跨终端体验** | 用户开启对应轮次后接入当时契约，完成界面、权限、生命周期和端到端验收。已做的接口必要迁移不等于启动全面前端重构。详见[延期台账](refactor/deferred.md)。 |
| D03／D05／R06 | **通用 Embedded、多实体及其他平台／部署形态** | 指定新设备或部署目标后独立评估并验证；当前不评估 Pi 5 承载 Easy-Turn，不因已有 Rust trait 或 feature 就宣称正式支持。多实体与未来部署属于条件项，不统一称作用户明确延期。详见[延期台账](refactor/deferred.md)。 |

## 下一步执行顺序

1. **先接收用户下一轮的回答流程要求，本轮不再安排讲话**：默认不打断已有现场反馈；两条实际输出完整提交。将无来源话题、上下文可追溯和未完成片段超过30秒直接断会话纳入具体改动，不清空聊天历史掩盖、不放宽期限。既有延迟分段报告供取舍，不能把HTTP健康耗时或模型片段长度当成说完到耳听的等待。新流程明确后先做有针对性的自动验证，再按实际变化集中真人验收；仍先取得当窗Ready、由用户反馈结束，不要求重复断电／UART初始化。
2. **新Rust候选仍留在软件门槛**：V03具体恢复修复已提交、构建产物保留，但V05两次Resume失败未解释，最后一次通过不算根因修复，不部署为已通过候选。下一次有具体假设或新失败时使用已准备的失败专用状态／PG锁诊断，不扩大期限、并发或忽略项，也不重新整套求绿。
3. 短句连续对话与主动打断已有本次结果；继续完成正常换气长句、无主动打断的连续结束、恢复、输出取消、成功Stop／新Start和下一轮；随后在维护窗口完成应用级成套升级恢复、登录／进程异常／休眠唤醒／resolver重启恢复，以及至少24小时混合故障和资源上界。分阶段结果不合并为整链完成。
4. AEC等待商家确认后单独验；首版实际MCP／第三方插件按目标验。V01／V02／V03／V04／V05／V06／V07保持各自证据，Companion仍为shadow，固件／ROS／执行器／全面前端及其他平台范围不扩展。

## 按具体需求再立项

以下是能力边界或候选方向，不是已承诺但遗漏的当前修复：

- 多实例／外部 SQL 的 Skills 自动索引通知：当前显式 reload；出现明确多实例需求后再处理。
- Temporal 独立 Worker、Home Assistant、设备侧模型、同进程组合／动态迁移：沿用[台账 C01—C05](refactor/deferred.md)的触发条件；Rig 与 rmcp 已不在待选型列表。
- WASM 的存储／网络／设备 host 能力、通用 Python/native 隔离、插件市场与自动依赖安装：当前计算型 WASM 没有这些权限。扩大支持时逐项设计授权与验收；受信 native／stdio 不视为沙箱。
- 具体通知渠道与设备工具：先明确真实接收端和效果回执，复用现有工具构建、Execution 和 Embodiment，不恢复空壳 executor。
- 早期 Affect／Expression 设计讨论中的其他建议：先对照当前实现确认实际需求，不把旧版本阶段计划重新展开为开发任务。原文见[整理前快照](refactor/todo-before-2026-10-19.md)。

## 已完成的软件范围

- [x] **原审计 1／2／4**：Embedded capture owner／健康生命周期、配置原子持久化与并发更新、首管理员选举和 refresh token 单次消费已修复并有回归。
- [x] **原审计 3／5／6／7**：模型流解析／工具参数边界、fetch 连接地址约束、独立 default chat profile／noop 装配、GPT-SoVITS 配置与确认状态已修复。后续模型与工具路径统一迁至 Rig；fetch 已修，不再暂缓。
- [x] **原审计 8／9，PN01—PN03**：管理权限与本人 scope、包内容／实例身份、原任务清理、稳定命令和未知结果、表达最小输入与私有解释器批准已落地。旧表达 JSON 子进程后续由 WASM ABI 3 替代；不用旧入口继续做扩展。
- [x] **原审计 10，PN04**：Extism 有界 WASM、未激活安装、精确 pin 授权、升级失效／撤权及真实 Rust PDK 示例已接入并完成本机回归。支持面为当前计算型工具／表达／processor，不包含通用 native 沙箱、host 存储代理或 Pi 实机认证。
- [x] **Rig／工具构建／MCP 干净迁移**：统一实现接 Execution，旧 provider／SSE、文本工具回退与 ToolExecutor 已删除。GitHub [Rig fork](https://github.com/Ke-Qiao/rig/tree/codex/alia-harness) 已建立，依赖固定修订；配置与边界见[工具契约](concepts/skills-and-tools.md)和[MCP 操作说明](operations/mcp-tools.md)。
- [x] **Embodiment 地基与完整重命名**：Body → Motion，传感器归入 Embodiment，物理操作状态／资源／取消／恢复复用 Execution 与现有设备能力；迁移保留历史事实，expression 只负责候选表达。已完成真实 PG、HTTP 和确定性适配器回归，ROS 与具体设备仍按 E01 暂缓。
- [x] **原审计 12 的软件验证入口**：统一验证脚本、真实 PG 缺失失败、全 noop 配对恢复、构建身份与 release 记录已建立。本轮候选出口按 S05 核对，S01—S03 保留为历史隔离演练，真实模型部分按 M01 暂缓。
- [x] **旧 TODO 的 Skills 热更新与时区**：Skills HTTP CRUD 已统一准备／提交／发布索引，并接动态 classifier 目录；时间工具支持 IANA 时区与无效值拒绝。证据见[Skills 检查点](refactor/resume-checkpoint-skills-2026-10-11.md)、[Classifier 检查点](refactor/resume-checkpoint-classifier-2026-10-11.md)。

## 历史记录与维护规则

原 1390 行 TODO 和混入的表达设计讨论完整保存在[整理前快照](refactor/todo-before-2026-10-19.md)。[首轮计划](refactor/plan.md)、[延期台账](refactor/deferred.md)、dated checklist／HANDOFF 保留当时范围、失败与证据，不按历史未勾选项重复派工。历史分轮数字不作为当前软件状态或整体完成率依据。

新增待办写清具体缺口、启动条件和完成标准；关闭时链接对应实现或验收记录。软件实现、fixture 验证、发布构建、真实服务和现场验收分别记录，用户最新决定优先。

每轮收尾必须集中说明：本轮实际完成、仍未完成、下一步最先执行什么、需要用户提供或拍板什么，以及何时需要 Pi／真人现场配合；不让用户从历史 checklist 自行推断推进顺序。
