# 构建记录

**2026-10-14：R02 Mac 离线模拟10/10，六轮仍2/6。** 实现 `1607045`；严格generate/replay、通用流摘要、closed loss自洽校验与四项生产helper组合模拟完成。默认Embedded327/0/0，无默认252/0/0为重复验证；实际CLI九场景符合预期，666项提交blob匹配。保留真实RED与旧CLI复现；单consumer、精确取消及清理保持。R03继续暂停，未启用设备/服务/模型或增加依赖。见[当前检查点](resume-checkpoint-r02-mac-2026-10-14.md)、[四份HANDOFF与证据](handoffs/2026-10-14-r02-mac/README.md)。以下为历史记录。

**2026-10-14：R02／R03 现场前置离线包10/10，六轮仍2/6。** 实现 `e409dc9`；显式只读配置/动作文件检查、全noop staging模板与旧操作资料清理完成。默认Embedded316/0/0，无默认CLI5/0/0；两示例实际离线检查通过，665个提交blob与最终输入一致。用户确认R1仍未接入且允许Mac模拟，R03后续暂停，部署资料待定但Pi可SSH；未启动服务/设备/模型或改共享环境。见[当前检查点](resume-checkpoint-field-prep-2026-10-14.md)、[四份HANDOFF与证据](handoffs/2026-10-14-field-prep/README.md)。以下为历史记录。

**2026-10-14：R03-A 可信动作内容身份10/10，六轮仍2/6。** 实现`e7265f0`；661项输入逐blob匹配。双端默认空本地清单、Body v2内容签名、同次有界读取/解析、原意图恢复及用户确认的cue写入保护完成。macOS默认Core/transport41、Embedded306、Server860，共1207/0/5；保留初始RED、候选DB中止、清理断言和最终证据。task PG已移除，astrbot被后续重启故保留共享OrbStack。当前Linux应用与硬件未验收。见[恢复检查点](resume-checkpoint-r03-a-2026-10-14.md)、[四份HANDOFF/证据](handoffs/2026-10-14-r03-a/README.md)。以下为历史记录。

**2026-10-09：L09 固定核心分层 10/10，L09—L12 1/4，六轮仍 2/6。** 源码 `ffd7af1`，V1 637 输入逐项匹配。Interaction 拥有 Conversation/Cache/Router/Persona/Skills，Platform 拥有 Insights 调度/Sensors；Memory maintenance 与 Cache review 归各自领域。macOS workspace **1056/0/5**、PTY **3/0**、无默认 availability **2/0**；Linux ARM64 Server **774/0/5**、无默认与 release/link 通过。默认 Shadow 和政策保持，任务资源已停止。见 [当前检查点](resume-checkpoint-2026-10-09.md)、[最终验证](layering-2026-10-09.md#最终验证与证据)、[L10—L12](checklists/layering-next.md)。以下为历史记录。

**2026-10-08：目录与依赖分层本轮 8/8，后续分层 0/4，六轮仍 2/6。** 实现 `08c54be`、最终源码 `978af9c`，V4 630 个输入逐项匹配。Memory 固定核心迁入模块；旧 Pipeline 拆为 Interaction 用例；Bootstrap / Platform 分离装配和生命周期。macOS workspace **1053/0/5**、PTY **3**；Linux ARM64 Server 全包 **771/0/5**、默认 release/native link 与无默认检查通过。V3 Linux 原失败及 V4 仅测试夹具修正分别保留。默认 Shadow 和政策不变；本轮任务资源已停止。见 [当前检查点](resume-checkpoint-2026-10-08.md)、[最终验证](layering-2026-10-08.md#最终验证与证据)、[后续 L09—L12](checklists/layering-next.md)。以下为历史记录。

**2026-10-06：旧入口与混音清理 10/10 完成，六轮仍 2/6。** 源码 `4434be4`，157 文件 +4154/-9590；619 个 Git 输入与最终 v3 清单逐项一致。语音/动作统一 VoiceSession v2、Playback v3、Body v1；Embedded 实际语音+cue 混音，mixer 不等于 AEC。macOS workspace **1051/0/5**、真实 PTY **3**、Embedded 无默认 **240/0/0**；Linux ARM64 定向 **669/0/3**、两端默认 release/link、无默认全部目标、两轮实际启停/重启均通过。旧 RPC 实测 UNIMPLEMENTED，默认 Shadow 与政策值保留，任务资源已停止。v1/v2 失败原件保留，真实模型两项本轮未重跑。详见 [本轮记录](cleanup-2026-10-06.md)、[恢复入口](resume-checkpoint-2026-10-06.md)、[10 项 checklist](checklists/cleanup-2026-10-06.md)、[四份 HANDOFF 与证据](handoffs/2026-10-06/README.md)。以下日期为历史。

**2026-10-04：本轮有界补缺口已完成，六轮仍为 2/6。** 源码 `010f7aa` 与冻结的 633 项输入逐项一致；交付身份、原始日志及 5 个 ignored 已核对。macOS workspace **1062/0/5**、真实 PTY **3**，本轮为 **1065 个不同 macOS 通过用例**，真实 Easy-Turn 两项本轮未重跑；Linux ARM64 两端默认 features release/link、两轮实际 PG/HTTP/gRPC 启停和重启通过，必要 8 目标 **578/0/3**，与 macOS 分开计数。补采集连续性/KWS reset、setup 单次完成/来源/关闭边界和真实 gRPC 拒绝证据；V01 未复现，根因继续开放，默认 shadow 不变。两个本轮容器已停止。AEC/声学、资产内容签名/控制板反馈、health 聚合/目录持久性和实机长期出口已保留。详见 [当前检查点](resume-checkpoint-2026-10-04.md)、[四份 HANDOFF 与证据](handoffs/2026-10-04/README.md) 及 [R01—R06 checklist](checklists/next-round.md)。以下旧日期段落仅为历史。

**2026-09-28：R01与R04均已完成，后续轮次为2/6。** R01源码 `a79cb1f`（CP7/7），R04源码 `8f5bb83`（管理4/4及统一验收完成）。R04冻结633输入逐项与提交一致，workspace1053/0/5、真实PTY3、实际Easy-Turn2，合计1058 unique macOS；macOS/Linux ARM64两端no-default all-targets与fmt通过，Linux仅编译。首次V3旧配对超时根因未定，保留V01；下一步依H01—H05推进AEC、执行器及Mac+Pi实机验收，其他Server平台按明确目标推进。详见 [恢复入口](resume-checkpoint-2026-09-28.md)、[R04 checklist](checklists/management.md)及 [本包四份HANDOFF](handoffs/2026-09-28-r04/README.md)。以下旧日期段落仅为历史。

**2026-09-26 当前状态：DU11 已完成，暂停纵向整合 11/11。** 合并 `948b3c4`，605 个输入与最终 V2 一致，未重跑相同源码的验证。当前进入 R01 Companion shadow/回放与受约束主动表达，三名 subagent 分别梳理策略状态、投递接线、回放与故障验证；详细步骤见 [Companion checklist](checklists/companion.md)。下方 9 月 23 日暂停状态作为历史保留。

**2026-09-26 R01 基础包：CP01—CP04 为 4/7。** 619 输入 V3 软件验证通过：macOS workspace 1005/0/5，另行真实 PTY 3、Easy-Turn 2 通过，1010 unique；macOS/Linux ARM64 两端 no-default all-targets 通过。默认 shadow，live 仍禁用；CP05/06/07 保持未完成。最新恢复入口为 [9 月 26 日检查点](resume-checkpoint-2026-09-26.md)；V1 五项失败、V2 导入编译失败与 V3 通过证据分别保留。

更新：2026-09-23。分支：`codex/architecture-refactor`；源代码基线：`3530395`。

**2026-09-23 额度暂停：暂停纵向整合实现与完整软件验收完成，DU01—DU10 为 10/11，用户因额度明确要求暂停，文档已保存，正式合入留待下次 DU11。** 源码修复为 `5810687`，默认栈修复为 `9c4f5b0`。voice v2 / playback v3 已接通原 Response/Context、两类暂停、持久控制、生产背压与精确旧输出退休。完整入口为[本轮检查点](resume-checkpoint-2026-09-23.md)、[DU checklist](checklists/duplex-integration.md)及[四份交接与证据](handoffs/2026-09-23/README.md)。

当前实现与本轮验收均指 `codex/duplex-integration`；主重构分支生产源码仍为 `e6ba75b`，待恢复后 DU11 合入。


最终 V2 共 605 个输入，manifest `1939a948191002882ba70e2578ad87d266d428b2ba423ccfa31aa178a34998d4`。macOS 完整 workspace **967 pass / 0 fail / 5 ignored**（203.85 秒）；5 个 ignored 已按真实 PTY 3 项、实际 Easy-Turn 2 项单独通过。Linux ARM64 约定矩阵 **325 unique pass / 0 fail**；两端 no-default all-targets 与 27 个变更 Rust 文件格式检查通过。各阶段输入无漂移。首次 V1 **964/3/5**、栈溢出、首次模型超时及其它红测均保留；只有两份旧测试夹具在 V1/V2 间变化，其余 603 项相同，不混算复跑次数。

后续 R01—R06 细分为[六项 checklist](checklists/next-round.md)，目前 0/6 完成；前端与通用 Embedded 适配仍按[台账](deferred.md)搁置。软件暂停与退休不构成 Pi、声卡物理静音、AEC、执行器到位或长期负载验收。

以下 2026-09-22 及更早的数字、主分支状态和下一步仅为当时记录，不表示当前状态。

**2026-09-22 再次按用户要求暂停。** 本轮已完成隔离组合和三个原组件包的修复/验证，Server output owner、语义接线及生产背压已进入整合树。恢复入口为 [2026-09-22 完整检查点](resume-checkpoint-2026-09-22.md)，数字状态为 [resume-state-2026-09-22.json](resume-state-2026-09-22.json)，逐项完成条件见 [暂停整合 checklist](checklists/duplex-integration.md)。

准确计数：DU01/DU02 已完成，**2/11**；其余 **9** 项仍开放。3 个原子组件包（协议/账本/媒体）已各自验证并合入隔离分支；真实PG运行时专项 **18 unique pass / 0 fail**，不重复累计单项复跑。最新Pipeline回归启动20项，只有3项明确完成，随后单个测试线程发生stack overflow/SIGABRT，未取得完整结果。新的Voice、5项配对gRPC、完整workspace/平台/真实模型验证尚待执行，不能记为已交付。

当前主分支仅保存文档；生产代码仍以 `e6ba75b` 的875项通过为已验收历史基线。未验收新代码保存在 `codex/duplex-integration` 的明确WIP快照，准确SHA见最新机器状态。下一次先排查栈溢出，再完成DU03—DU11，不重新规划已确认范围。

以下2026-09-21数字和未合入状态仅作当时的历史记录，现况以最新检查点为准。

**2026-09-21 暂停记录。** 三个 subagent 当时均已结束，构建/测试 exec 已收集结束。恢复入口：[完整检查点与 17 项后续 Todo](resume-checkpoint-2026-09-20.md)，历史数字状态：[resume-state-2026-09-20.json](resume-state-2026-09-20.json)。文件名沿用 2026-09-20 工作批次，收口在北京时间 2026-09-21 完成。

本批次 6 个工作包中，2 个已合入、4 个隔离保存。已验证主生产源码为 `e6ba75b`（默认 workspace 875 通过、0 失败、5 显式跳过）；后续 `b995098` 仅更新设计文档。未合并包分别为 A 协议 `2619a28`、B 控制账本 `76f5b04`、C 媒体预算 `779f4f8`、Root Server 接线 `e94f66e`。A 组件验证通过；B 的 9 项 PG 测试仅编译未执行；C 当前编译失败；Root 未装配/未编译。全部原始状态及四份交接已归档，不把 WIP 视作完成验收。

用户已授权开始构建，当前 Easy-Turn 只运行在 Server。当前记录是 P1、P2 与运行时基础改造的结果，首轮整体交付尚未完成。前端和 Embedded 通用适配仍按 [台账](deferred.md) 搁置。

## 已提交构建包

| 提交 | 内容 | 已取得的证据 |
| --- | --- | --- |
| `525046f` | 可调用的 Server 启停、可取消 CLI/OOBE、事务迁移、共享音频/轮次领域契约与 Easy-Turn HTTP 边界 | Server 回归；真实 PostgreSQL/pgvector 空库、回滚、恢复、并发、重复及校验和测试；两个真实 PTY 生命周期测试 |
| `b40120a` | 端侧有界播放与独立停止控制、本地 generation 失效、统一 CPAL/ALSA 控制循环 | 播放控制 11 项、已有 audio 集成 9 项通过；Linux ARM64 构建通过 |
| `29e9ab6` | 固定官方源码/权重的常驻 Easy-Turn 服务与可复现依赖 | 14 项服务契约测试；真实 CPU 严格加载及 16 个官方样本 HTTP 推理通过 |
| `33a7c98` | 持续采集 v1 协议、端侧 server_turn 模式、Server 窗口与模型决策、可靠语音入口、可监督交互管线 | 实际 gRPC→Rust→CPU 模型；136 个 Server 常规单元测试及全部 Server 集成测试（带真实数据库）通过；Embedded 无默认特性 98 项通过 |
| `1ecb2ce` | boot/session/generation 播放协议、原子入队失效、凭证续期与解绑关闭、端侧信号退出 | macOS 默认特性整个 workspace 409 项通过；真实 DB 套件通过；两项真实模型测试另跑通过；Server 无默认特性 all-targets 通过；Linux ARM64 端侧变更检查通过 |
| `c257f07` | Embedded 启动失败回滚、可等待的有界关闭、实际监听地址、长 HTTP/2 连接清理及原生任务所有权 | 无默认特性 122 项通过；macOS 默认 all-targets 和 Linux ARM64 无默认 all-targets 检查通过，见 [Embedded 生命周期](embedded-runtime.md) |
| `5aecb38` | 持久 Execution 共用入口、Memory 规范来源与隐私栅栏、可信输入贯穿消息和工具 | 空数据库默认 workspace 500 项；真实 CPU 模型与 gRPC 两项另跑通过；固定提交 Linux ARM64 Server 检查及 152 项库测试、Embedded 默认完整链接通过 |
| `2f8797c` | Server 运行时所有权与故障回滚、Memory 原子批次与批量遗忘、数据库排序的可信 source epoch | 默认 workspace 561 项通过；真实 PTY 与 CPU 模型 4 项另跑通过；固定提交 Linux ARM64 Server 库和真实 PG 共 280 项通过，Embedded 默认完整链接通过 |
| `ee9cbfd` | 规范 Response、逐句音频 Delivery、播放 v2、真实后端提交回执与生命周期监督 | macOS 默认 workspace 636 项通过；4 项真实 PTY/模型另跑通过；固定提交 Linux ARM64 Server + Embedded 库及 Server 集成共 542 项通过，Embedded 默认完整链接通过 |
| `e63d40c` | TextDelivery 与签名 D3 身体动作，唯一发送、精确取消、部分提交与迟到事实 | macOS 默认 workspace 720 项通过，5 项 PTY/真实模型另跑通过；固定提交 Linux ARM64 642 个不同测试通过、5 项跳过，保留首次 Runtime 并行超时和同源串行复核记录 |
| `a33ec67` | 不可变 ContextLease、实际读集、持久政策、Cache 审核与向量 CAS，派生写入及输出接纳校验 | macOS 默认 workspace 785 个不同测试通过、5 项跳过；5 项 PTY/真实模型另跑通过；Linux ARM64 固定提交与夹具修正候选的结果见平台记录 |
| `759985f` | Memory 实际向量身份、规范 revision 投影、可关闭后台重建与原子 profile 热切换 | macOS 默认 workspace 802 个不同测试通过、0 失败、5 项跳过；两端无默认特性 all-targets、28 个变更 Rust 文件格式检查通过；5 项真实 PTY/模型另跑通过 |
| `5467e8b`，整合 `9de8c74` | 实际后端播放参考、独立采集/播放时钟、有界交接与可见缺口/关闭统计 | macOS 默认特性实际链接及库测试 124 项；Linux ARM64 无默认特性 all-targets 与 130 项库测试通过，13 个变更 Rust 文件格式检查通过 |
| `b4987a8`，整合 `8c3f545` | 规范 Appraisal/Relationship、独立关系版本、显式重建、完整 Memory 来源校验与隐私擦除传播 | 冻结主分支 macOS 默认 workspace 859 个不同测试通过，5 项真实 PTY/模型另跑通过；完整记录见下节 |

## 可恢复暂停 Stage 1 整合（2026-09-20）

本地基础提交 `213d5ed`，主分支整合 `e6ba75b`，为同一输出作用域增加可逆 SoftwareGate，保留原生播放流、重采样状态、块游标与尚未提交媒体。疑似插话与明确等待分别记录；新采集语音会阻止旧恢复决定，明确等待只接受明确继续。终结位不可重开，重复控制返回原命令结果与当前快照，已退休作用域也不把历史 Applied 伪造为未执行。单 payload、共享源 PCM 和渲染缓冲均在复制前检查字节预算。详见[Stage 1 实施记录](duplex-pause-stage1.md)。

与 `8c3f545` 整合的 macOS 默认完整 workspace 验证为 **875 个不同测试通过、0 失败、5 显式跳过**，370.02 秒；其中 Core 25 项、Embedded lib 135 项、Server lib 216 项。测试使用真实任务 PostgreSQL 与实际 Sherpa 原生归档，未设置测试串行模式。587 个构建输入清单 SHA-256 为 `c97283aee880270634f7cedb58ced6ffbdc18d67e425e5aa6533817c7f29a6c1`，运行前后无漂移。两端无默认特性 all-targets 检查也通过，36.18 秒。证据位于 `run/validation-20260920/duplex-stage1-integration/`。5 项真实 PTY/模型在前一 Appraisal 包已另跑，本次完整回归仍明确跳过，没有将此前结果冒充本次重跑。

Stage 1 当时只完成本地门和证据基础；该历史基线的网络 WAIT 仍永久取消。下一包接通 playback v3 / voice v2、原 Response 控制记录、持续采集屏障与生产背压，实施契约见[两类暂停设计](duplex-pause-design.md)。软件停止提交不等于声卡已排空、物理静音或 Presented；Pi 真机、AEC 和听感验收继续待做。

## Appraisal / Relationship 与 Memory 来源整合（2026-09-20）

用户消息在原接纳位置固定 privacy/relationship 两个版本，实际召回、规范 Response 提交及 Execution 终态作为 Appraisal 的输入。判断保留解释器、政策、原始时间、规范根和有效期；冻结前的 Affect 投影进入同一 Context，之后的事实只影响后续规划。旧 RAM 关系 profile、无来源 writer、全局句子缓存与私人 Expression trace 已移除。关系查看、清除及有界显式重建使用真实数据库权限、CAS 和原操作回执，见[实施契约](appraisal-provenance-design.md)和[操作手册](../operations/relationship.md)。

Memory 在独立检索、Context、图遍历及向量 provider 调用前后检查完整父根；原消息摘要不可重新补证。维护提升保留原始 episode，稳定转换 ID 阻止重复与软删除后复活，去重不会删掉最终保留结果所需的祖先。隐私删除在同一事务擦除规范正文、向量、图和全部派生后代，直接推进 SQL 隐私版本也走同一闭包。新增 `000010`、`000011`、`000012`，未重写之前已提交的迁移。

完整 workspace 候选共 584 个冻结构建输入，清单 SHA-256 为 `7c1c2e8048e6539ccc555480d3753ba6e0694642043e77acf55f9c6af3c2f9a6`。macOS 默认 `cargo test --workspace --locked --offline --no-fail-fast` **859 个不同测试通过、0 失败、5 显式跳过**，172.63 秒，运行后源码无漂移；生命周期测试内部的子进程重复项不再累计。默认 Embedded 测试实际链接已校验的 Sherpa 原生归档。

5 项跳过已单独完成：真实 PTY 3 项；真实 CPU Easy-Turn 与持续 gRPC 2 项。最终直接 Backchannel 为 1335 ms，gRPC Complete/Backchannel/Wait 为 542/335/398 ms；首次候选直接 CPU 调用曾为 **12611 ms**，保留原记录，不能用后续暖调用掩盖首个慢调用。这些是模型功能及本次调用时间，未证明端到端声学延迟达标，也未启用新的可恢复 WAIT。

证据统一保存在主仓库 `run/validation-20260920/appraisal-integration/`：`source-manifest-verified.json`、`workspace-verified.log`、`workspace-verified-summary.json`、`pty-verified-result.json` 与 `model-live-verified.log`。前两轮失败完整保留：第一轮 857 通过、1 失败、5 跳过，暴露 SQL 错误先于 HTTP timeout 时清除响应丢失原操作身份，补确定性 red/green 后修复；第二轮 853 通过、6 失败、5 跳过，均为测试完整 schema 并发创建/销毁耗尽 PostgreSQL 默认共享锁容量。Context 夹具现在从建立到清理最多并行 4 个；用例内部的真实并发和原 45 秒期限未变。最终全量重新运行通过，不合并失败轮次的计数冒充成功。

Linux ARM 复核进一步证实，Pipeline 夹具的首次真实 executable 身份计算占用了 15 秒行为期限：415,129,944 字节的 debug ELF 连续读取约 14.2 秒。最终仅将真实 `ExecutionService::new` 初始化移到夹具计时前，没有伪造身份、改生产逻辑或延长期限。此最终提交候选仍为 584 文件，清单 `source-manifest-release.json` 的 SHA-256 为 `208f11edb3ba7c75e7b3e7acd1042ff002e0c7d150f092d7b495d746c3394cc3`，相对前一清单只有该测试文件不同。受影响的 macOS 默认 Server lib 216/216 已重新通过，3 项 PTY 显式跳过；66 个变更 Rust 文件格式检查通过。这是完整 workspace 成功之后的测试夹具增量复核，不能改称前一日志运行过修改后的测试文件。

Linux ARM 最终候选默认 Server **637 个不同测试通过、0 失败、5 跳过**，全部 45 个集成目标执行完毕，默认及无默认特性所有目标检查通过；原始失败和修复依据见[平台记录](platform-validation.md)。可恢复暂停的本地 Stage 1 已单独提交为 `213d5ed`，Server/Proto 纵向接入仍未启用；Companion、实际 AEC/漂移映射、Pi 与执行器验收和混合长时负载继续属于首轮主线。

## 播放参考与音频时钟（2026-09-20）

CPAL 参考来自最终原生格式回调缓冲区，包含实际增益、转换、量化和填零；ALSA 只记录 `writei` 实际接纳的完整帧前缀。采集与播放保留独立时钟和流身份，缺口、回退、部分写、重开及最终丢失都有明确表示。播放参考交接采用预分配有界队列和不等待的锁尝试，既有采集转换/broadcast 路径的锁与分配仍需后续处理。设计和接口边界见[音频参考契约](audio-reference-clock-design.md)。

本包的 macOS 默认特性测试实际链接 Sherpa 原生库，124 项通过；无默认特性检查与同一库测试也通过。冻结后参考流/关闭专项 7 项再验证通过。Linux ARM64 无默认特性全目标检查及 130 项库测试通过，包含 6 项 ALSA 原生调用及明确注入的错误边界测试。只读构建输入的 597 文件清单摘要为 `503f32ba40b55419ac5f7bc320b6900d85bd19071d6e426426018d99a2ee8d53`，逐文件哈希与实际命令结果保存在 `run/recovery-20260920/audio/FINAL.json` 及其验证目录。以上是本包定向证据，不能替代整合后的完整 workspace 或树莓派声学验收。

恢复工作也已完成：中断后临时目录中的两个未提交工作树源码不完整，现从本任务实际成功的文件修改记录恢复；仍存活的 Appraisal 测试与恢复结果逐字节一致。开发工作树与后续证据改存主仓库忽略的 `run/worktrees/`、`run/recovery-20260920/`，并重新执行验证。历史记录中的 `/private/tmp` 路径仅代表当时的证据位置，不能假定文件仍存在；平台记录说明了可恢复摘要与不可恢复完整原件的区别。Appraisal/Relationship 后续整合的当前验收在本文件上节记录，不沿用音频恢复时的未完成状态。

## Memory 向量身份与后台重建（2026-09-14）

向量投影现在绑定实际 provider、endpoint、模型、维度、输入摘要和规范 Memory revision。新增 `000009_memory_embedding_identity` 移除无可信请求身份的固定 1536 维表；规范内容仍可读取和管理。Memory 的向量检索独立于 Cache 开关。Inline 与后台 worker 共用有界并发，后台按规范记录扫描缺失投影，遵守当前政策和来源限制，关闭时取消并等待实际任务收尾。具体结构、预算与边界见 [Memory 向量设计](memory-embedding-design.md)。

管理员热切换 embedding 配置时，完整不可变描述符与两项政策 pin 在同一事务内提交，之后发布 provider 与设置。真实 HTTP 回归覆盖 Cache 关闭、模型/endpoint/4→6 维切换、政策 UPDATE 故障的完整回滚，以及不重启立即 store/update/query。独立复核还推动 Context 在查询 embedding 之前验证原始输入和完整旧读集，三个测试在修复前均观察到一次不应发生的 provider 调用，修复后均为零。

冻结源码的默认 workspace **802 个不同测试通过、0 失败、5 显式跳过**，按 target 排除内嵌子进程重复计数。完整日志 `/private/tmp/alia-memory-workspace-verified.log`；统计 `/private/tmp/alia-memory-verification-summary.json`；422 个源码/配置文件的 SHA-256 manifest `/private/tmp/alia-memory-final-source-manifest.json`，运行后未改变。Context 26、Cache/Insights 16、Memory 13、Memory embedding 13、Memory 隐私 24、Server runtime 38 和 Execution service 30 项均包含在整体结果中，不重复累计。两端无默认特性 all-targets 记录 `/private/tmp/alia-memory-no-default-verified.log`。

首次完整运行只有迁移夹具仍期待旧向量表存在而失败，现改为验证新表存在且旧表移除，再跑全部 workspace 获得上述结果；首轮失败日志 `/private/tmp/alia-memory-workspace-final.log` 保留。5 项跳过随后全部另跑通过：3 项真实 PTY 生命周期，输出 `/private/tmp/alia-memory-pty-verified.log`；2 项真实 CPU Easy-Turn 与持续 gRPC，输出 `/private/tmp/alia-memory-model-verified.log`。直接 Backchannel 为 834 ms；gRPC Complete/Backchannel/Wait 为 524/340/397 ms，只是本次模型调用耗时。HTTP 热切换定向记录 `/private/tmp/alia-http-memory-profile-20260914-escalated.log`；请求前来源校验的 red/green 记录 `/private/tmp/alia-context-query-admission-red.log`、`/private/tmp/alia-context-query-admission-green.log`。

本包同时修正两处 Linux ARM 验证夹具：Execution approval proxy 禁用 Nagle 并一次写出同一协议消息，Runtime 初始化期限测试先通过真实构建身份入口预热大型 debug binary 摘要。固定 `a33ec67` 仍保留 703 通过、4 失败、5 跳过的原始结果；仅含两项夹具修正的候选将受影响 67 个不同测试并行重跑全部通过，不能改写为固定提交全套已通过。候选没有包含本包新的 Memory 代码，见 [平台记录](platform-validation.md)。

尚未合并的 Appraisal 包正在补足规范 Memory 的递归父根校验：已接受的派生 Memory 在父记录改版、变为私有或过期后，必须从 Context、独立检索和向量投影中排除。本包自身 revision/epoch/profile 检查不能替代该闭包。Appraisal/Relationship、Companion、真实硬件和长时混合负载仍属于首轮主线。

## ContextLease、派生缓存与持久政策（2026-09-13）

本包将实际进入模型的 Memory、图、历史、缓存与工具事实绑定到不可变 ContextLease。原始用户消息在纠错或模型执行之前持久化，生成过程可有界扩展读集，第一次准备 Response 时冻结；旧 epoch、policy、来源身份和规范 revision 不能通过重试刷新。缓存写入要求已提交的规范 Response，审核、向量回填与替换采用真实 SQL 版本比较。自动记忆抽取、维护、工具写入和输出接纳各自使用明确用途。

持久政策由 PostgreSQL 管理，HTTP PATCH 先做管理员授权与 CAS，再发布进程设置。停止记忆检索仍保留真实存储和清除能力；恢复或模型/profile 切换产生明确版本转换。独立复核已修复设置应用中断后重试无效、模型隐式维度未保存、合法省略维度导致启动失败，以及关闭 Memory 时误选 Noop 后端的问题。Context 的共同事务门槛不覆盖尚未接入来源的 Appraisal/Relationship；完整 Companion 闭环仍属后续主线。

最终默认 workspace **785 个不同测试通过、0 失败、5 显式跳过**，排除同一测试启动的子进程重复计数。Server 库为 218 通过、3 跳过，Context 23、Cache/Insights 16、Execution service 30、Memory 隐私 24、Server runtime 37 项通过。完整日志 `/private/tmp/alia-context-workspace-verified.log`，按 target 去重的汇总 `/private/tmp/alia-context-verification-summary.json`。两端 `--no-default-features --all-targets`、67 个变更 Rust 文件格式检查和 `git diff --check` 通过。

5 项跳过测试均在冻结生产源码后另跑通过：真实 PTY 3 项、官方音频 → CPU Easy-Turn 和持续 gRPC 2 项。直接 Backchannel 为 821 ms；gRPC Complete/Backchannel/Wait 为 527/335/404 ms，仅代表这次模型调用。PTY 首次命令将 stdout 重定向到文件导致终端前置条件失败；修正为真实 PTY 的直接输出后 3 项通过，记录 `/private/tmp/alia-context-pty-actual.log`。真实模型记录 `/private/tmp/alia-context-model-final.log`。

整包首次运行揭示三个旧测试夹具问题：停用账户仍期待清除成功、Memory PG 用例未建立有效账户且没有隔离 schema、Insights 的断言未检查新增外层说明中的完整错误链。已保留原有故障语义并修正夹具，随后重新运行完整 workspace 得到上述结果。生产代码没有为通过这些测试放宽授权。首轮原日志 `/private/tmp/alia-context-workspace-final.log` 与源码 manifest 保留用于核对。

新增迁移 `000007_context_lease` 与 `000008_cache_context`，没有修改已提交迁移。需要退回前一程序版本时，应使用其对应的独立数据库；旧程序不能解释本包新增的来源和缓存约束。当前结果不覆盖 Appraisal/Relationship、旧 Memory embedding 写回、真实硬件、长期负载或当前包的非 macOS 平台。实现边界见 [Context 契约](context-revocation-design.md)。

## TextDelivery 与 D3 身体动作（e63d40c）

文本输出已接入共用 DeliveryService，UTF-8 分块先持久化再交给 CLI native writer，保留实际字节前缀和准确取消，模型断流不提交完整助手历史。文本、音频和身体分别记录证据，未将 Submitted 解释为已读、已听到或机构到位。

身体链路包含 Core 严格类型、双向签名 gRPC、可信配置公钥、规范 Response 对应的唯一 dispatch permit、设备共享 boot、control lease/sequence 和唯一 native writer。绑定/voice 撤销、回执淘汰、动画完整预检、部分串口/I²C 写入、旧身份取消和关闭纳入同一生命周期。设备明确拒绝的新身份会留下签名 Rejected；取消携带完整原意图，先退休尚未入队的序号，阻止迟到 preparation 执行。Server 对未确认的原取消限频重试，永不重试 Submit。

独立审查发现并修复两条真实竞态：普通拒绝只返回无签名错误导致 Server 永久等待不存在的命令，以及取消先于 preparation 到达后迟到 Submit 仍可能执行。另收紧 Unknown 的迟到证据校验，已确认 terminal/quiescent 后不能再增长 native 提交量。

定向证据：Motion 45 项、真实签名设备 gRPC 6 项、身体 PostgreSQL 6 项、运行时 HTTP 身体/文本接口 1 项；Text repository 5 项、service 4 项、Pipeline 38 项、CLI 7 项通过。Core 20 项、Server 签名身体/pin/原播放 26 项通过。关闭异常的测试包含已完成 JoinError 不被重复 poll，以及继续 join 其余 owner。

默认 workspace 的完整回归 **720 个不同测试通过、0 失败、5 显式跳过**；内嵌子进程重复执行的一项不再累计。5 项随后另跑通过：真实 PTY 3 项、官方音频→CPU Easy-Turn 和持续 gRPC 2 项。当前模型直接 Backchannel 848 ms；gRPC Complete/Backchannel/Wait 为 508/311/372 ms，仅代表这次模型调用，不是端到端声学延迟。两端 no-default-features all-targets 检查通过，60 个变更 Rust 文件格式检查通过。

整包回归修复了一处新测试 fixture：删除本 schema 的表会沿 search_path 回退到其他迁移测试安装的 public 同名表；故障注入改为本 schema 内列定义故障，HTTP 503 与诊断隐藏已通过。末次恢复修复另跑身体仓储 **6 项**、Server runtime **33 项**与签名客户端/pin/播放 **26 项**，全部通过，验证 `latest_checkpoint` 恢复已确认的 quiescence，同时原 Unknown/status/progress 不改写、原动作不重播。这些是对应既有用例的末次回归，不重复累计。

本包新增迁移 `m20260908_000005_text_delivery.sql` 与 `m20260913_000006_body_delivery.sql`，没有修改已提交迁移。前端没有进入构建。真实执行器和音频呈现仍需硬件信息；当前旧 `neck.anim` / `neck2.anim` 超过 v1 短动作时长/帧界限，会明确拒绝，未截断或修改原素材。短动作资产与真实机构验收仍属身体产品化工作。

固定 `e63d40c` 的 Linux ARM64 复核现已完成：Core 20、Server 514、Embedded 108，共 **642 个不同测试通过、5 项跳过**，Embedded 默认完整构建与链接通过。Server Runtime 首次并行运行出现一项同步期限超时，原源码串行复核 33 项全过；这一时序敏感性保留在 [平台记录](platform-validation.md)，不将串行通过描述为并行稳定性证明。这些结果只覆盖已提交的 TextDelivery/D3 包，不覆盖当前 Context 变化。

## 已提交的 Response / Delivery 与播放 v2（ee9cbfd）

本包将规范 Response、逐句音频 Delivery、投递监督和设备回执接入同一个 Server runtime。Response 与唯一助手历史原子提交；输出前持久化意图、原始身份与发送许可。重复调用查询已有事实，过期恢复不重播。CLI 文本也在外发前建立 Response，独立 TextDelivery 发送监督继续进入下一包。接口与状态见 [Delivery API](delivery-api.md)。

播放 RPC 升级为 `alia.playback.v2`，Start 握手后才发送媒体。端侧输入 End 后仍保留观察；CPAL / ALSA 按实际后端提交计数，源帧通过连续 sinc 重采样映射。精准取消等待已进入的输出写入边界收尾并返回原序号最终快照；本地超时不伪造设备回执，迟到证据独立审计。Submitted 不冒充 Presented，PostSpeech 仍需真实呈现证据。

交叉故障审查补齐了三处设备竞态：取消先于在途 writei/callback 返回、End ACK 早于输入 lease 释放，以及等待 Start 时撤权仍先推进 generation。Server 另修复心跳暂停持锁媒体 future 的死锁，保留原始 native/future 直到真实 join；panic 时尝试记录 Unknown，依赖继续隔离持有。Affect 数据库写入也从未托管 spawn 改为实例有界队列，关闭先排空后释放数据库。

当前定向证据：Delivery repository 14 项、service 12 项；Pipeline lib 35 项、memory capture 6 项、pipeline integration 17 项；Embedded 无默认特性 142 项和 Server 播放 8 项；root 真实 PG/HTTP/native Delivery runtime 4 项、Affect writer/清除 runtime 7 项；Expression 库 41 项。各组存在被 workspace 收录的同名用例，不能将定向和整包结果重复累计。

最终默认 workspace 在独立空数据库 `alia_refactor_delivery_final` 上 **636 个不同测试通过、0 失败、4 显式跳过**；日志中已有测试启动的 1 次子进程内部运行不重复累计。Server runtime 入口 32 项、Delivery repository 14 项、Delivery service 12 项全部通过。Server 无默认特性 all-targets 检查、60 个变更 Rust 文件格式检查及 `git diff --check` 通过。

4 项跳过测试均在冻结源码后显式运行通过：真实 PTY 2 项，以及真实 CPU Easy-Turn 和 gRPC 持续语音 2 项。直接 Backchannel 为 307 ms；gRPC Complete / Backchannel / Wait 为 476 / 305 / 381 ms，仅为模型调用时间，不代表端到端声学延迟。Linux ARM64 将使用整包固定提交归档验证。硬件呈现、AEC、身体 D3、完整 ContextLease 撤销及 Companion 仍是未完成主线，见 [Delivery 边界](delivery-design.md)、[身体命令设计](body-command-design.md)与[上下文失效设计](context-revocation-design.md)。

```sh
ALIA_TEST_DATABASE_URL='postgresql://alia_test:alia_test@127.0.0.1:32768/alia_refactor_delivery_final' \
  cargo test --workspace --locked --no-fail-fast
cargo check -p alia-server --no-default-features --all-targets --locked
# 在真实 PTY 内执行：
cargo test -p alia-server --lib --locked -- --ignored --nocapture --test-threads=1
ALIA_EASY_TURN_PCM="$PWD/run/easy-turn-backchannel.pcm" \
ALIA_EASY_TURN_PCM_DIR="$PWD/run/easy-turn-stream-pcm" \
  cargo test -p alia-server --test easy_turn_live --test voice_stream_live --locked -- --ignored --nocapture
```

## Server runtime、原子 Memory 和来源版本整合包（2f8797c）

本包建立可嵌入的 ServerRuntime：启动失败完整回滚、实例独立取消、连接和后台 actor 排空、统一关闭期限，以及超时后继续保留真实 worker 和依赖的 ShutdownHandle。工具目录写入或显式 Provider 配置出错会拒绝 ready。现有 WS 是管理员诊断流，依据数据库当前账户和角色授权、定期复核，并随宿主关闭实际连接。详见[运行时契约](server-runtime-design.md)。

Memory 的 LLM 整理改为一次完整原子批次，模型只可退役记录；用户批量遗忘在一个事务内处理全部目标。最终回归还复现并修复了时间戳的因果漏洞：当 Server 时钟比数据库快 60 秒，旧请求可能在 clear 后重新写入记忆。现在所有可信入口在数据库接纳时固定 source_privacy_epoch，排队、Execution、用户消息和后台抽取保留该版本；版本变化后旧请求被拒绝，新请求不受快慢钟影响。具体边界见[来源接纳](source-admission.md)和[Memory 记录](memory-findings.md)。

最终默认特性 workspace 为 **561 passed、0 failed、4 ignored**。日志另含故障测试启动的 1 次子进程内部运行，统计去重；Server 常规 lib 161 项、Execution repository/service 19/27 项、Memory 隐私 24 项、后台 13 项、Server runtime 21 项均通过。新来源迁移已在本任务专用的全新数据库上实际运行。Server 无默认特性 all-targets 检查也通过。

4 项 ignored 测试均随后显式运行通过：两个真实 PTY 退出与终端恢复测试，以及真实 CPU Easy-Turn 与 gRPC 持续语音两项测试。模型直接 Backchannel 322 ms；gRPC Complete/Backchannel/Wait 为 511/337/405 ms，均为模型调用时间，不是端到端声学延迟。最终回归同时修正两项过时 Provider 测试，明确未知配置必须失败、noop 必须显式选择。

```sh
ALIA_TEST_DATABASE_URL='postgresql://alia_test:alia_test@127.0.0.1:32768/alia_refactor_runtime_v1' \
  cargo test --workspace --locked --no-fail-fast
cargo check -p alia-server --no-default-features --all-targets --locked
# 在真实 PTY 内执行：
cargo test -p alia-server --lib --locked -- --ignored --nocapture --test-threads=1
ALIA_EASY_TURN_PCM="$PWD/run/easy-turn-backchannel.pcm" \
ALIA_EASY_TURN_PCM_DIR="$PWD/run/easy-turn-stream-pcm" \
  cargo test -p alia-server --test easy_turn_live --test voice_stream_live --locked -- --ignored --nocapture
```

本包随后已对固定 `2f8797c` 完成 Linux ARM64 验证：Server 默认 all-targets、161 项库测试、119 项真实 PG 集成和 Embedded 默认完整构建/链接均通过。排除的 2 项 PTY 在 macOS 已验证。见[固定提交平台记录](platform-validation.md)。持久 Delivery、播放后端提交证据、完整遗忘传播及硬件联调继续属于未完成主线。

## 先前 P2 整合包（5aecb38）

Execution 已成为工具入口共用的持久执行边界，拥有意图、确认、Outbox、认领、结果、幂等与 unknown 恢复。提交/等待/关闭有整体期限；批准已提交但 ACK 丢失时仍保留会话取消所有权。操作入口与证据见 [Execution API](execution-api.md) 和 [数据库验证](execution-validation.md)。

Memory 在真实事务内处理用户隔离、规范 revision、隐私 epoch、来源关系、递归删除和投影失效。可信 Server 输入时间在排队前固定；用户消息在模型/工具之前提交，Execution 保留真实来源 ID，AI MemoryStore 在执行时重新验证原来源而不能制造新时间。语义去重保留中文、否定和数值标点。详见 [Memory 记录](memory-findings.md) 和 [通道身份](channel-identity.md)。

最终默认特性 workspace 结果为 **500 passed、0 failed、4 ignored**，包含两端实际默认特性编译/链接。Execution repository 19 项、service 25 项、Memory 原有 13 项、Memory 隐私 14 项、gRPC 身份 6 项和迁移 7 项全部通过。两项 ignored 的真实模型测试随后单独运行通过；另外两项 PTY 测试本轮未重复执行，先前基础包已实际通过。

该提交的 Server 无默认特性 all-targets 检查也通过（18.05 秒）。随后已对固定 `5aecb38` 获得 Linux ARM64 容器原生检查、152 项 Server 库测试及 Embedded 默认特性完整构建链接证据，详见[平台验证](platform-validation.md)。这些结果不覆盖下一包未提交的运行时变化，也不代表 Windows 或 Pi 音频外设已通过。

```sh
ALIA_TEST_DATABASE_URL='postgresql://alia_test:alia_test@127.0.0.1:32768/alia_refactor_p2_verify2' \
  cargo test --workspace --locked

ALIA_EASY_TURN_PCM="$PWD/run/easy-turn-backchannel.pcm" \
ALIA_EASY_TURN_PCM_DIR="$PWD/run/easy-turn-stream-pcm" \
  cargo test -p alia-server --test easy_turn_live --test voice_stream_live --locked -- --ignored --nocapture
```

数据库为本任务独有的临时实例。首次全空数据库运行发现 pgvector 被安装到临时 schema，已将数据库扩展准备固定到 public，并从另一个无扩展、无表的空库完成上述 500 项回归；未修改旧迁移内容。该包真实 CPU 模型直接 Backchannel 约 827 ms，gRPC Complete/Backchannel/Wait 约 497/326/396 ms，仍只是模型调用时间，不含 LLM/TTS 和声学呈现。

`33a7c98` 中嵌入渠道测试的 feature 属性位置随后修正；修正后默认特性测试及 `--no-default-features --all-targets` 检查均通过。记录验证时应以修正后的分支为准。

## 运行和验证入口

- 模型安装、固定版本、启动和官方样本复现：[Easy-Turn README](../../services/easy-turn/README.md)。模型只在显式 prepare 时下载，权重保存在忽略的 `run/models/`。
- 模型真实结果与 MPS 失败依据：[组件验证](../../services/easy-turn/VALIDATION.md)。本机 CPU 样本通过，当前依赖栈的 MPS `torch.isin` 不可用，启动会明确失败；CPU 路线不依赖 MPS。
- Server 配置使用 `config/server.example.d/providers/easy_turn.toml` 中的 `[providers.easy_turn] enabled = true`，endpoint 默认 `http://127.0.0.1:8091`。
- Embedded 使用 `[speech] mode = "server_turn"`、16 kHz 单声道。唤醒后在播放期间持续上传；VAD 只提供活动证据。没有静默回退成端侧模型。
- 模拟与领域测试验证协议、控制、取消及队列；它们不等同于声卡、AEC 或真机双讲验收。
- 连续采集、认证续期、播放隔离和关闭的实际契约：[语音协议 v1](voice-protocol.md)。

本次真实验证宿主为 Apple M5 Max / 64 GiB / macOS 26.6.2 / arm64。尚未得到用户确认其就是最终部署 Mac。Python 3.10.20、Torch 2.2.2 的 CPU 首次样本推理约 1,449 ms，其余 15 个样本约 286–537 ms；这是特征提取和模型执行耗时，不含 LLM/TTS、网络设备播放或模型加载。

服务端完整测试使用独立的临时 PostgreSQL/pgvector，未使用现有开发数据库。针对固定 `ef07d43` 的 Linux ARM64 Server 默认特性构建及 136 项库测试、Embedded 默认 Sherpa 特性完整构建链接均已通过，详见 [隔离平台验证](platform-validation.md)。新播放变更另外通过 Linux ARM64 Embedded 无默认特性检查；不能把这些容器结果推导成 Pi 音频外设通过。

`1ecb2ce` 的完整 workspace 测试启用了两端默认特性：409 passed、0 failed、4 ignored。四项跳过中，两项真实 Easy-Turn 测试随后显式运行并通过；另外两项真实 PTY 测试未重复运行（前一基础包已实际通过）。最新真实 gRPC 测试还核对了 Complete 2、Wait 3、Close 的原始会话身份；对应 CPU 模型执行约 508 / 328 / 426 ms，不代表端到端声学延迟。

## 尚未完成的主线

这些项目仍属于首轮范围，不能归入用户要求的延期项：

1. AEC、采集/播放时钟映射与漂移测量、可恢复暂停、真实设备呈现回执、双讲与自然插话质量。实际后端参考流与独立时钟已在 `5467e8b` 提供；当前 WAIT 仍停止当前回答，尚不能从实际播放游标恢复。
2. D3 短动作资产及真实呈现/位置证据，依据 [Delivery 设计](delivery-design.md) 实施。TextDelivery/D3 固定提交的 Linux ARM64 复核已完成；唯一发送与软件提交不能代表硬件已完成。
3. Companion 状态、可出错假设与受约束的主动表达，见[实施设计](companion-design.md)。Appraisal/Relationship 的规范来源、失效重建、完整派生 Memory 根和旧 Expression 缓存移除已在 `8c3f545` 完成；Companion 闭环仍属首轮未完成工作，不是用户明确延期。
4. 其他 Server 平台、Pi 真机联调和至少 24 小时混合负载。Server 和 Embedded 已建立可调用运行时；未来新增 Delivery 等模块仍需纳入同一所有权及关闭契约。

播放网络身份栅栏和持续会话凭证续期已在 `1ecb2ce` 实现。续期测试推进到虚拟 365 秒，同时用真实签名检查错误签名/身份/scope 和 301 秒旧 iat 被拒；绑定变更由设备立即及每 250 ms 复核，覆盖上传结束后队列尾部。真实设备长时会话仍需要单独验收。

模型观测成功、文本回复生成、音频被接收、提交后端和实际呈现分别记录。播放 v2 已提供 accepted/submitted 的软件证据，presented 仍未知。旧全局中断错误提示旁路已停止外发，无法验证的表达 JSON 不再升级 completed；身体 signed v1 已接入，带作用域提示音仍待接入，不能称全链路已证明声学完成。旧广播事件入口仍有可见丢弃策略，尚未全部替换为可靠命令入口。

## 待补充的实机信息

需要 Pi 系统版本、测试连接方式、麦克风/声卡/扬声器与 AEC 能力、执行器与反馈/限位信息，以及最终 LLM/TTS/Embedding 服务配置。已在对话中提出，独立代码与协议构建继续进行；依赖这些信息的实机结论保持未验收。
