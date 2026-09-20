# Companion 首轮实施边界

状态：2026-09-20，下一实施包的设计，尚未实现。依据用户确认的实体陪伴范围与[原提案 8.3–8.4](../../Project_Alia_Architecture_Proposal_2026-09-07.md#83-认知闭环的目标边界)。Companion、真实主动投递和反馈仍是首轮主线；前端与 Embedded 通用适配继续按[延期台账](deferred.md)处理。

## 状态与现有实现

当前没有独立 Companion/Cognition/Opportunity 模块。Rule 与 Scheduler 共用 ActionRegistry，工具已经接入 Execution；Insights 拥有提取、整理、反思与缓存审核。复用 Context 的实际来源校验、Appraisal 的版本化记录及纯 reducer、Expression 的调节逻辑，以及 Execution/Delivery 的持久身份、唯一执行所有者和回执。Companion 不持有输出通道、工具执行器或 native Body writer。

| 状态 | 所有者与约束 |
| --- | --- |
| Observation | 来源适配层提供规范记录 ID、revision/digest、原接纳时间、原版本与有效期；设备报告与用户身份分别表达 |
| Hypothesis | Companion 保存有界结构化解释、解释器版本、证据、置信度和期限；不把推测登记为用户事实 |
| InitiativeCandidate | Companion 保存 owner、主题、理由、去重键、期限、建议渠道及所需身份和隐私条件 |
| Opportunity | 独立评估器读取会话占用、终端绑定及可用性、证据新鲜度和明确反馈；机会不能从关系分数推导 |
| 硬政策 | 持久、按 owner 管理主动联系许可、安静、暂停与预算，使用版本 CAS；Affect 不得放宽这些限制 |

现有 Insights 将 presence 变化关联到 `last_active_user`，见 [`scheduled.rs`](../../packages/alia-server/src/providers/insights/scheduled.rs)。这类值只能用于已有后台用例的上下文，不能成为主动联系的身份保证。EventBus 广播、RAM cooldown、普通 `success` 字段或事件处理时重新取得的 epoch 也不能代替原始事实、幂等回执和持续许可。Companion 不能通过标成 Rule/Scheduler 来进入无 Context 的非 AI Execution 路径；用户显式配置的自动化继续保有自身语义。

## 实施顺序与出口

1. **可重放的 shadow 包。** 为 shadow 建立专用读取/记录用途，不能借 Generation、SourceNotice 或 AppraisalRebuild 取得生产写入或投递许可。首个输入只选实际用户来源及规范 Response/Execution 回执；ResponseCommitted 不表示用户已听到。每次最多处理 32 个 Observation、形成 4 个 Candidate，沿用有界引用和字节预算。固定解释器、显式时钟与录制输入完成 Observation → Hypothesis → 候选 → 独立机会判断 → 决策记录；不调用实时模型、传感器、工具或通道，不改变真实关系状态。模拟机会必须明确标记。
2. **受约束的真实投递包。** 在来源、机会预约、持久去重和预算具备后，先接入一个有明确 owner 绑定的设备与一种主动意图。生成前、内容冻结时、Execution claim 和 Delivery dispatch 前都复核完整来源、硬政策和机会，通过既有执行/投递所有者产生事实。不能为主动行为伪造一条用户聊天消息或重新签发旧来源的版本。
3. **反馈与硬件验收。** 仅以规范 Delivery、设备回执和明确用户反馈更新后续假设；Unknown 不自动重播，没有回复不推断用户厌恶。软件回放与 shadow 通过后，才进入实机表达、静音、撤权、资源占用和时机验收。

shadow 包使用真实 PostgreSQL 验证重复、乱序、重启、两个 worker 竞争、来源改版/清除/过期、政策变化、匿名 presence 与未知回执。每一条路径都验证模型、工具和外发调用数为零，并区分录制输入与现场事实。真实投递另验证预约失效、输出竞争、内容冻结、回执丢失与取消，不能用 shadow 结果代替。

具体迁移编号、API 与模块边界在进入实现包时确定；不为这一设计预先增加 Cargo features、独立服务或通用工作流框架。身份信息不足、机会过期或输出能力未知时，保留候选的可解释拒绝原因，不擅自推断用户在场。
