# 暂停纵向整合后的首轮任务 checklist

**2026-10-17：插件契约技术草案9/10，尚待D01—D03答复。** 本轮只清理与定约，未改变当前API／权限／依赖；未审核代码安装与后续隔离执行是明确保留目标，安装不授予业务权限。后续PN01管理／scope、PN02包／原实例生命周期、PN03表达ABI、PN04实际隔离与第三方示例均未实施；先复现再修复，PA01—PA11不记通过。恢复从[当前检查点](../resume-checkpoint-plugin-contract-2026-10-17.md)、[checklist](plugin-contract-2026-10-17.md)与[契约](../../architecture/plugin-contract.md)开始，模型链精确信息继续保留前一包待定。默认shadow／政策／原期限并发保持，R03暂停、历史V01open、六轮仍2/6。以下为历史。

**2026-10-17：软件发布资料与真实模型链前置10/10，MP10答复已登记。** 已核对三个分工、配置／模型入口和来源，修正文档与两行旧注释；677受测输入全同、1项仅TOML注释差异，生产／依赖／测试不变。新[发布记录](../../operations/software-release.md)区分Linux release、Mac定向／历史全量与未验真实LLM/TTS链。BotCF候选／模型待定、本地语音资产与开发验证方式已有答复，旧base/settings方案不同，不能直接复用；从[当前检查点](../resume-checkpoint-model-preflight-2026-10-17.md)和[本轮checklist](model-preflight-2026-10-17.md)补精确模型／素材与任务配置后确定有限执行范围。未调用服务或硬件，默认shadow／probe=false、当前政策与原期限／并发保持，R03暂停、历史V01open、六轮仍2/6。以下为历史。

**2026-10-17：当前release全noop staging生命周期与恢复10/10完成。** 源码`6c62c84`；实际V1失败→确定性RED→单点取消归属修复→GREEN2/0，Linuxworkspace1287/0/5/0filtered、两端no-default/release/native链接和V2双轮正常启停/恢复通过。678提交输入、148归档证据与资源清理闭环；原日志分块派生计数显式保留，不重复测试求绿。默认shadow/政策/原期限并发不变，R03暂停、历史V01open、六轮仍2/6。从[最新检查点](../resume-checkpoint-release-lifecycle-2026-10-17.md)和[checklist](release-lifecycle-2026-10-17.md)继续；下一步先确认模型服务与部署资料，再做约定软件模型链验收，R02/R05现场门槛保持。以下为历史。

**2026-10-17：当前 Linux 软件构建/测试/release链接10/10完成。** 应用源码仍`4e2968f`，完整workspace1285/0/5/0filtered，两端no-default全目标与default release链接通过，677输入和同源Mac一致；证据归档、任务资源已回收。无生产改动，默认shadow/政策/原期限并发保持，R03暂停、历史V01open、六轮仍2/6。最新恢复从[检查点](../resume-checkpoint-linux-release-2026-10-17.md)与[checklist](linux-release-2026-10-17.md)继续；下一候选为已有全noop staging的真实进程生命周期与恢复验收。以下为历史。

**2026-10-17：CompanionInvitation 历史只读引用10/10完成。** 源码`4e2968f`；用户确认历史事实与输出授权分离，专用marker保留规范身份、完整根/真实TTL，Memory派生保存及防主动自我强化完成。真实RED0/1，定向13/0/0；一次workspace1277/0/5/0filtered，677提交输入匹配，34原件归档，任务PG0/0后容器与卷已回收。原期限、并发、默认shadow/当前政策保持，R03暂停、历史V01open、六轮仍2/6。恢复从[本轮检查点](../resume-checkpoint-invitation-history-2026-10-16.md)和[checklist](invitation-history-2026-10-16.md)继续；下一步候选为当前版本的软件发布验收。以下为历史。

**2026-10-15：V01初始数据库构造取消10/10完成，历史根因仍open。** 源码`8bcda7a`；独立测试owner保留原SQL、Fixture与CASE许可至清理结束，四项取消回归覆盖未poll/缓冲ready与真实CREATE drain。保留RED0/2和V1全量1265/1/5失败；只修测试阶段门与observer清理后，V2 workspace1266/0/5（既有ignored、0filtered）、duplex31/0/0，675提交输入匹配。结束约0.175秒同库PG0/0，任务容器/卷已回收。原期限、并发、默认shadow/政策不变，R03暂停、六轮仍2/6。恢复从[本轮检查点](../resume-checkpoint-v01-db-cancel-2026-10-15.md)和[checklist](v01-db-cancel-2026-10-15.md)继续，以下为历史。

**2026-10-15：V01部分初始化与paired late-drain 10/10完成，历史根因仍open。** 源码`dc3ec17`；三项确定性RED确认schema遗留后统一初始化失败清理，补充真实SQL等待取消与原write跨10秒shutdown的迟到排空证据。一次workspace1262/0/5（既有ignored、0filtered）、duplex27/0/0；674提交输入一致。结束后约0.584秒同库PG0/0，任务容器/卷已移除。默认shadow/政策/原期限/并发不变，R03暂停、六轮仍2/6。恢复从[本轮检查点](../resume-checkpoint-v01-partial-2026-10-15.md)和[checklist](v01-partial-2026-10-15.md)继续，以下为历史。

**2026-10-15：V01 外层45秒与失败清理10/10完成，历史根因仍open。** 源码`a73e6e8`；保留RPC原join、部分初始化owner与现有ShutdownHandle，body+cleanup共用原45秒成功截止，过期仍排空并失败，DB回收在owner完成后。workspace1256/0/5（既有ignored），duplex21/0/0，671提交输入匹配；结束后56.106秒PG快照0schema/0client，任务容器/卷已删除。三份分工和Root交接已收口。默认shadow/政策/期限/并发不变，R03暂停，六轮仍2/6。恢复从[本轮检查点](../resume-checkpoint-v01-outer-2026-10-15.md)和[checklist](v01-outer-2026-10-15.md)继续，以下为历史。

**2026-10-15：V01有界调查包10/10，历史根因仍open。** 测试诊断`f817623`；两批原baseline各9/0/0，诊断版workspace1245/0/5（既有ignored），duplex10/0/0。三批未复现，只补失败观测，不改变生产、期限或默认并发；669提交输入匹配。任务PG容器已移除，最终即时连接快照缺失如实保留。恢复从[V01检查点](../resume-checkpoint-v01-2026-10-15.md)开始，详见[checklist](v01-2026-10-15.md)。默认shadow/政策不变，R03暂停、六轮仍2/6。以下为历史。

**2026-10-14：R02 R1 硬件参考路由与 consumer 前置核对10/10。** 本轮仅文档和官方公开来源核验，音频实现仍为`28920fb`，六轮仍2/6。公开处理后mono UAC与功放参考分离；实际板/固件、电气、模式、格式/声道和延迟待补。用户已确认硬件AEC优先，实物/固件和输出链资料仍未确定；Pi的SSH访问不代表R1接入，不预建软件AEC，R03暂停。当前恢复从[R1检查点](../resume-checkpoint-r02-r1-2026-10-14.md)开始，见[本轮checklist](r02-r1-2026-10-14.md)。以下为历史。

**前一包执行顺序（历史）：R02独立时钟/参考缺口/consumer帧需求审计10/10完成；实际AEC路径/consumer选型和声学出口仍待确认。R03后续按用户要求暂停，两者满足现场出口后再进入R05。** 实现`28920fb`，同源默认Embedded335/0/0，无默认260/0/0为重复配置验证；实际CLI两项通过。恢复从[最新检查点](../resume-checkpoint-r02-clock-2026-10-14.md)开始；本轮不是部署AEC或启用硬件的授权，现场门槛见[部署资料](../../operations/hardware-deployment-readiness.md)。
更新：2026-10-14。本表承接 [DU01—DU11](duplex-integration.md)，不是新一轮范围授权。以下六项仍属于已确认首轮目标或明确的后续技术评估；不得用软件测试通过代替真机出口。前端与通用 Embedded 适配继续按 [延期台账](../deferred.md) 处理。

当前 **2 / 6 完成**（R01 `a79cb1f`；R04 `8f5bb83`，见 [R01 checklist](companion.md) 与 [R04 checklist](management.md)）。六项规模不同，不把该数字换算成项目完成百分比。R04不依赖外设的软件包已完成；R02/R03/R05 的实际接入与验收需要台账 H01—H05 对应信息。

2026-10-04 有界补缺口已完成，源码 `010f7aa`；交付身份、V01 调查、当前 Linux 默认 build/run 及 R02/R03 前置见 [本轮交接](../handoffs/2026-10-04/README.md) 和 [恢复入口](../resume-checkpoint-2026-10-04.md)。以下新增勾选只关闭具体软件前置，不增加完成轮数。

2026-10-06 清理已完成，源码 `4434be4`，独立 [清理 checklist 10/10](cleanup-2026-10-06.md)。旧语音/动作入口与无身份回退已删除，实际两路 mixer / scoped cue 已接入并完成当前 macOS/Linux 验证；mixer 不等于 AEC，本轮不增加六轮完成数。恢复从 [当前检查点](../resume-checkpoint-2026-10-06.md) 开始。

2026-10-08 目录与依赖分层本轮 **8/8**，最终源码 `978af9c`，六轮完成数不变。最新恢复从 [当前检查点](../resume-checkpoint-2026-10-08.md) 开始，剩余架构工作见 [L09—L12（0/4）](layering-next.md)。

2026-10-09 L09 固定核心分层 **10/10**，源码 `ffd7af1`，L09—L12 **1/4**，六轮状态不变。恢复从 [当前检查点](../resume-checkpoint-2026-10-09.md) 开始，下一条为 L10。

2026-10-10 L11 跨域事务所有者清理 **10/10**，源码 `60f665c`；L09—L12 **3/4**，六轮仍 **2/6**。恢复从 [L11 检查点](../resume-checkpoint-l11-2026-10-10.md) 开始，下一项 L12；原 V01 与本轮旧关闭测试超时定位继续保留。

2026-10-10 L12 出站契约清理 **10/10**，源码 `d9f9721`；L09—L12 **4/4**，六轮仍 **2/6**。最新恢复从 [L12 检查点](../resume-checkpoint-l12-2026-10-10.md) 开始；下一软件项为 Skills HTTP CRUD reload。Linux 当前 hash 瓶颈已有同字节测量与修复，历史失败边界保留，原 V01 不因此关闭。

2026-10-11 Skills HTTP 索引清理 **10/10**，源码 `7aca2a6`；六轮仍 **2/6**。最新恢复从 [Skills 检查点](../resume-checkpoint-skills-2026-10-11.md) 开始。三个 HTTP 索引缺口关闭，下一软件项为静态 classifier 候选与运行 Registry 的衔接审计。

2026-10-11 Classifier 运行目录衔接 **10/10**，源码 `744cae2`；六轮仍 **2/6**。动态候选、在途定义核对、缓存绑定与取消已同源双平台验收。最新恢复从 [Classifier 检查点](../resume-checkpoint-classifier-2026-10-11.md) 开始，下一软件项为 R04 整体 health 限额审计。

2026-10-11 /health 清理 **10/10**，源码 `29420a5`；六轮仍 **2/6**。整体预算、重复探测、并发和生命周期已同源双平台验收。最新恢复从 [Health 检查点](../resume-checkpoint-health-2026-10-11.md) 开始，下一软件项为 R04 配置目录持久性。

2026-10-11 配置目录持久性与发布恢复 **10/10**，源码 `0954996`；六轮仍 **2/6**。最新恢复从 [配置检查点](../resume-checkpoint-config-durability-2026-10-11.md) 开始。下一软件候选：先审计 `SettingsRegistry::save` 与 `PATCH /api/v1/settings` 的 live apply、内存配置及 `settings.toml` 持久化顺序，定义保存失败与重试的恢复语义，再在授权范围补缺口。保持 API、当前政策和权限；先检查既有实现，不直接扩展本轮发布协议为通用事务框架。

## R01 · Companion shadow、回放与受约束主动表达

- [x] 核对已实现 Appraisal / Relationship / Affect / Regulation 的规范来源与剩余缺口，定义可回放的候选输入和决策证据。
- [x] 先运行 shadow：记录候选、抑制原因和资源预算，不把未经许可的候选直接投递。
- [x] 把准许的主动表达接入规范 Response / Context / Delivery，并遵守原 output owner、暂停、取消和来源失效。
- [x] 验证用户正在说话、记忆删除、权限变化、重启和机会过期时不会陈旧投递或重复表达。
- [x] 记录确定性回放结果、代码提交和真实输出边界，完成独立验收。

## R02 · AEC、音频时钟映射与双讲

- [x] R1官方参考路由与consumer前置核对：明确硬件/主机分工、声道平均的条件门槛、帧/延迟资料与集中待确认；见[R1 checklist](r02-r1-2026-10-14.md)。这是资料出口，实际硬件/consumer与声学出口仍待。

- [x] 独立时钟/参考缺口/consumer帧需求审计：固定batch锚点与nominal offset契约，修sample回退/分块误恢复及旧replay时钟提示；见[10/10 checklist](r02-clock-2026-10-14.md)。未选择AEC consumer或建立物理映射。

- [x] Mac离线模拟：严格生成/只读重放CLI、closed统计下界与capture/post-mix/单consumer/导出重放组合回归完成；见[10/10 checklist](r02-mac-2026-10-14.md)，不等于AEC或真实声学验收。

- [x] 现场前置离线检查、全noop模板及音频设备身份/格式/AEC参考待补资料收口，见[10/10 checklist](field-prep-2026-10-14.md)；没有真实录放音。

- [x] 语音/短提示实际混音与最终 post-mix reference、各源取消/暂停/短写证据；macOS/Linux 离线验证通过，见 [audio HANDOFF](../handoffs/2026-10-06/HANDOFF-audio.md)。

- [x] 核对真实 render/reference/capture/wake/VAD 顺序、既有依赖和离线 fixture；补 native continuity 消费与 KWS reset，当前 macOS/Linux 回归通过，见 [audio HANDOFF](../handoffs/2026-10-04/audio.md)。
- [x] Pi 目标/Ubuntu 已只读核对；用户确认 R1 尚未接入、以后 USB 接 Pi。见 [硬件确认 checklist](hardware-confirmation-2026-10-14.md)，该项不表示音频就绪。
- [x] R02-A：native 双流、单消费者、有界会话/导出与合成 PCM 离线重放完成；旧 source-WAV 路径删除。macOS 默认 Embedded 284/0/0，Linux 本轮未跑，见 [10/10 checklist](r02-a-2026-10-14.md)。没有真实录音、AEC 或声学验收。
- [ ] 固定 H02/H03 的运行账户音频权限、实物/固件、采集/播放设备、采样格式及物理布局。
- [ ] 固定实际 AEC 路线：先核对 R1 参考/处理模式；只有确需主机软件 AEC 时，才在已有独占 reader 基础上确定 consumer、重分帧和独立时钟/漂移映射。1024 样本参考块和上传帧不能充当算法帧。
- [ ] 在已确认路线上接通并核验实际 AEC：硬件路径验证功放参考与处理输出；若采用软件路径则使用已有 native 参考/时钟契约。分别处理真实参考失效、主机证据缺口与恢复，不混作同一故障。
- [ ] 实测同时播放/说话、附和、明确等待/继续、噪声、失联和重开；区分模型标签与声学质量。
- [ ] 记录端到端暂停/恢复延迟及设备配置；不能只报告模型推理耗时。

## R03 · D3 动作资产与执行器反馈

**当前后续暂停（用户2026-10-14确认）。** 板卡/通道/供电/OE/限位/校准资料全部未定；已完成的软件包保留，待用户恢复后推进现场。

- [x] 复用配置/矩阵/批准资产的纯文件检查和PCA静态规则，补资产人工审核及校准现场记录；硬件/校准确认仍禁用，见[现场前置包](field-prep-2026-10-14.md)。

- [x] 复核已有签名命令/唯一 writer/mock 故障；补真实 gRPC 的 unbound/foreign-controller/重连及旧 boot 拒绝，当前 macOS/Linux 回归通过，见 [motion HANDOFF](../handoffs/2026-10-04/motion.md)。
- [x] 用户选择 PCA9685A + MG996R、停止/断连保持、机构校准后允许启动归中；实际接线/校准未验收。
- [x] R03-0 软件：停止固定保持、校准后启动门槛、旧键拒绝、PCA AI 与 SLEEP/多板初始化恢复已完成；macOS 默认 Embedded 263/0/0，Linux 本轮不可用，见 [10/10 checklist](r03-0-2026-10-14.md)。没有真实校准或硬件动作。
- [ ] 固定 H04 的实际板卡、供电/OE/通道、反馈接口、机械限位、机构校准和保持失效边界。
- [x] R03-A 软件：双端本地受信清单、Body v2完整内容签名、同次有界读取校验/解析、原tuple恢复及提示音写入边界已完成；macOS1207/0/5，见[10/10 checklist](r03-a-2026-10-14.md)。
- [ ] 实际审核并部署双端资产清单，核对所选硬件可观测能力；不虚构 PCA9685A 的 MCU boot/ACK/位置反馈，原物理验收仍需外部测量或后续反馈硬件。UART flush/I²C write 不代表物理到位。
- [ ] 把签名动作资产、位置/速度限制及停止策略接入选定执行器。
- [ ] 验证迟到命令、断连、重启、限位和停止失败，保留真实执行与反馈证据。
- [ ] 分开报告接纳、提交、实际位置和完成，不把软件退休或网络 ACK 当作机构到位。

## R04 · CLI / REST 管理诊断与组件收敛

- [x] 按全新安装用例核对配置、设备绑定、政策/记忆管理、执行确认、主动策略及诊断入口。
- [x] 补齐首轮必要操作、当前协议/配置说明和错误反馈，保持不依赖 Web/iOS/Android。
- [x] 核对 C01/C02 的实际用例与进入门槛：本包无需引入新组件；具体版本、权限、取消、资源与恢复验证仍随实际接入开展，不因提案列名整包引入。
- [x] 完成命令/API 回归和开发者操作文档。
- [x] 2026-10-04 有界补审：setup HTTP/CLI 单次完成、Host/Origin/JSON 与关闭期间拒写；health 聚合/目录持久性已核对，剩余增强见 [management checklist](management.md)。

## R05 · Mac + Pi 整机验收

- [x] 当前 Linux ARM64 容器默认 features release/link、无外设 PG/HTTP/gRPC 两轮实际启停/重启与必要测试通过；二进制/输入身份和资源停止均归档。此项是目标部署前置，不能代替下列 Pi 整机出口。
- [ ] 确认 H01—H05，锁定部署清单、模型地址/版本、密钥与全新数据库路径。
- [ ] 从干净环境执行安装、配对、启动、升级/恢复及关闭流程。
- [ ] 使用真实麦克风、声卡、扬声器与选定执行器，验证持续采集、物理停止和恢复听感。
- [ ] 至少 24 小时混合负载，覆盖断网、进程退出、设备重连与资源上界，保存可追溯报告。

## R06 · Server 平台与未来部署形态

- [ ] 对明确指定的其他 Server OS/CPU 建立构建与实际运行矩阵；当前 Linux ARM64 容器证据单独保留。
- [ ] 指定未来设备后，再评估 C04 的设备侧模型或同机多进程部署；当前不测试 Pi 5 承载 Easy-Turn。
- [ ] C05 的同进程组合、进程内传输与动态迁移分别按实际收益和故障边界评估，不因 Cargo feature 存在就宣称支持。

每个变更包继续使用三个 subagent 按独立文件/模块分工，Root 负责接口、组合验证和正常提交。每次收口写明：已完成条目、未完成条目、实际测试命令与结果、固定源码身份、资源状态、下一条可执行任务；发生额度暂停时保存四份 HANDOFF。

## 2026-10-11 运行时 Settings 续轮

本轮 **10/10**，源码 `2f5e2a2`；六轮仍 **2/6**。原子快照与明确失败阶段、同值恢复、privacy SQL／管理员入口和同源双平台验收已完成。最新恢复从 [Settings 检查点](../resume-checkpoint-settings-recovery-2026-10-11.md) 开始。下一软件候选：先审计纯非政策 `PATCH /api/v1/settings` 在等待更新锁／政策读取期间发生管理员降权时的准入重验证，保留现有 SQL actor 核验、API 与权限政策；先复现，再补缺口。

## 2026-10-12 Settings 身份重验证续轮

本轮 **10/10**，源码 `4553e88`，六轮仍 **2/6**。纯非政策 PATCH 最终准入与 8 项真实 HTTP/PG 回归已同源双平台验收。最新恢复从 [身份检查点](../resume-checkpoint-settings-authorization-2026-10-12.md) 开始。下一软件候选：先审计并复现 `GET /api/v1/settings` 等待后返回含敏感配置的设置前的当前身份有效性；保持原 API 和管理员读取边界，不扩大为全系统持续撤权框架。

## 2026-10-12 Settings GET 身份重验证续轮

本轮 **10/10**，源码 `d080710`，六轮仍 **2/6**。最后响应准入、GET六项和PATCH八项回归已同源双平台验收。最新恢复从 [GET检查点](../resume-checkpoint-settings-read-authorization-2026-10-12.md) 开始。下一软件候选：先审计 Settings GET/PATCH 共用 update gate 的排队、各阶段预算和取消所有权。current/final-auth 已有 2 秒，gate/preview/apply 等待尚未故障实证；区分准入前等待与 SQL/desired/module 已发生后的部分成功，不直接套整请求 timeout 或全局回滚，不把客户端断开等同 handler 已取消。

## 2026-10-15 Settings 慢模型、额度与关闭收尾

本软件包 **10/10**，源码`4e99134`；六轮仍 **2/6**。三个LLM配置快照、SoVITS权重额度期限、HTTP/1实际Drop与同值重试、privacy共享锁关闭证据已完成，保留政策/管理员边界/部分成功。Server868/0/5，格式后最终provider51/0/0与无默认检查通过，669提交输入匹配；见[恢复检查点](../resume-checkpoint-settings-waits-2026-10-15.md)。

下一步按用户新指令选择已有软件/部署待办；V01根因、同步文件I/O不可抢占及可信部署边界仍开放，不自动延伸为Settings框架。R02继续硬件AEC优先与资料核对；R03后续暂停；R05必须等待实际音频/机构/部署出口。无新增硬件或外部模型授权。

## 2026-10-15 V01 ServerStartup 初始化取消收口

本包10/10，源码`9e3a934`；成功结果已缓冲后取消仍交付的缺口经RED确认并修复，四项新回归和四项既有回滚通过。一次workspace1270/0/5、server_runtime85/0/0，676提交输入一致；同库约0.248秒0/0，任务容器与卷回收。见[恢复检查点](../resume-checkpoint-v01-server-startup-2026-10-15.md)及[四份HANDOFF](../handoffs/2026-10-15-v01-server-startup/README.md)。

下一软件候选只作有界审计：核对from_config的Database::connect返回owned pool之前的取消与SQLx既有清理能力，先复现再补确认缺口。SQL未知确认、discovery局部失败及旧全局with_fixture异常清理顺序仍未动态证明，不预建全局完成框架。历史V01根因open，无新条件不重复求绿。原期限/默认并发/政策/shadow不变，R03用户暂停，R02资料及R05现场门槛保持，六轮仍2/6。
