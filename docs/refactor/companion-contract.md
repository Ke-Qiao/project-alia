# R01 Companion 契约与实施边界

更新：2026-09-26。DU11 已合入 `948b3c4`；本文件定义后续实现契约，勾选与实际证据见 [checklist](checklists/companion.md)。

## 首轮闭环与状态归属

现有规范 Appraisal 是可撤销的解释记录，不是对用户情绪的事实声明。Companion 从既有记录及其完整 Context 根读取有界输入，生成固定的 `FollowUpInvitation` 候选；不新增伪造用户消息，不自行解释新的传感器或原文。首轮固定邀请不调用 LLM，也不宣称已经完成全部 Affect/Regulation 候选语义；使用的规则、参数和版本必须冻结。

`modules/companion` 拥有用户政策、候选、纯决策和持久回放；`memory/context` 继续拥有 SQL 规范来源验证及隐私/关系 epoch；`interaction` 的存活 VoiceSession 拥有真实占用状态和输出 generation；`delivery` 保有唯一 Response、媒体 owner 和暂停/取消事实。装配层只负责启动、限流与关闭。

## 默认行为和固定参数

- 默认 `shadow`，可持久关闭；启动配置不得覆盖已持久的用户政策。
- 工程初值：候选 TTL 60 秒、两次准入间隔 900 秒、UTC 每日最多 3 次，等待新回应期间不再次主动联系。上述是有边界的参数，不是已对真实设备开启的设置。
- 显式静音/暂停、来源失效、当前交互占用和未知身份是硬性抑制，不受关系分数抵消。用户确认的临时避让与明确暂停区分继续沿 DU11 执行。
- 候选创建时间来自规范记录的原时间；有效期取源有效期与候选 TTL 的较早者，重试不能刷新。
- 候选去重身份跨政策版本稳定。shadow 结果不占用真实投递预算，不可升级为 live capability。

## 纯决策与回放

输入冻结原 Appraisal 引用、政策及版本、候选、显式 `as_of`、独立机会和预算；结果包含稳定原因码与输入/结果 digest。回放不读实时模型、传感器、墙钟或当前运行状态。

机会区分明确模拟与存活会话验证两种来源。模拟只支持 shadow；生产缺少会话、capture 或认证证据时记录未知并抑制，不能假设空闲。Presence 不证明绑定用户在场。

任何 API 上传的 JSON 都不是 authority。受认证入口只接收调用者自己的政策、operation ID 和已有 Appraisal ID；完整 epoch、roots、时间和可用能力由服务器读取。批次最多 32 条，重复/越界/未知字段明确拒绝，事务固定 worklist，未知提交结果只能重试原 operation。

当前 REST 入口均使用现有 Bearer 身份，并重新核对数据库中的 active account：

| 入口 | 请求与结果 |
| --- | --- |
| `GET /api/v1/companion/policy` | 读取调用者当前 revision 与完整 policy；默认 shadow |
| `PUT /api/v1/companion/policy` | `{expected_revision, policy}`；CAS 冲突 409，当前 `enabled` 明确拒绝 |
| `POST /api/v1/companion/shadow` | `{operation_id, appraisal_ids}`；1—32 个非空且不重复的原 Appraisal ID；同 operation 与相同 worklist 返回原冻结回执 |
| `GET /api/v1/companion/replay/{operation_id}` | 当前权限复核后读取原冻结输入并重放纯决策；来源已擦除或过期返回 410 |

请求超时或结果未知使用 503，调用者保留原 operation 或政策 expected_revision，不能通过新身份绕过幂等。入口不接收 owner、机会、预算、scope、来源文本、Context 或外部权限声明。当前测试证明提交前超时及已提交回执的幂等读取；没有把它们等同于实际数据库 commit ACK 丢失的故障注入。

## 持久化与生命周期

政策使用 CAS；同一 operation 改 intent 拒绝。候选/决策在同一来源验证事务写入，原 Context 用途不会因此扩大。默认 shadow 不发行输出 ContextLease，也不新增 Appraisal/Affect、Response、Delivery、Execution 或媒体。

后台工作只做有界 SQL 扫描和纯决策，SQL 承担恢复/去重，EventBus 不能成为权限事实。每次扫描处理有界批次；关闭时先原子关闭接纳，再取消并等待 worker 和全部已接纳的公开请求排空，数据库在这些所有者之后释放。取消某次 stop 等待者不能丢失排空义务。公开入口最多 8 个在途请求，扫描进度仅记录不含身份或私密数据的计数与固定错误类别。缺少真实机会使用数据库决策时间表示 Unavailable，不构造宿主时钟观察。阶段内可以通过受认证的有界入口验收，不以新增轮询或模型调用掩盖缺失的产品逻辑。

本阶段 shadow 为每个规范来源一次取样，忙碌/未知机会也记录抑制；它不代表候选已实现等待空闲后重新评估。CP05 需要单独的 pending→机会重评→原子准入/过期状态机，不能将 shadow 去重墓碑当作 live 待办队列。

隐私 epoch、关系清除和来源撤销/修订必须在对应事务擦除私密冻结证据及派生候选。仅保留 opaque identity、状态/计数等去重 tombstone；已擦除记录不得导出输入、内容 hash 或从日志重建。历史回放读取仍复核当前 owner 和来源权限。

## 真实主动语音的额外门槛

必须经过候选/政策/预算原子准入、独立且新鲜的会话机会和规范 Context/Response/Delivery。此链路与组合测试完成前，API 拒绝设置 `enabled`，启动时旧 enabled 政策也不产生媒体。

VoiceSession actor 提供只读 snapshot 和有界 reserve mailbox；验证原 owner/channel/session/boot、近期 capture、无活动 utterance/检测/重试/存活输出，await 后排空已到输入并重新核对。只有 actor 分配 generation，返回原 `Arc<OutputProducer>` 及取消身份。机会 deadline 只能缩短；超时或丢 ACK 不另起 scope/Response，不自动重放 unknown 输出。

wire 增加显式 `InitiativeAdmitted` 和能力协商。旧 peer 不支持则拒绝主动接纳；不伪造 Easy-Turn Complete。Embedded 验证会话、boot、watermark 与新 epoch 后沿原 fence，保留近端说话优先及显式继续/取消。

新增的软件风险必须在 live 前解决：设备已经捕获新 speech、帧尚在网络中时，Server drain 无法看到该事实。现有设备 fence 会替换旧 control，capture watermark 本身不会建立 Hold。因此首次主动 Register/Start 必须在设备 capture 所有者的锁内验证原机会 watermark，并为注册后新 speech 保留本地阻断，覆盖 Fence、SessionEvent 和 Register 不同 RPC 的到达次序。只检查 SessionEvent 或 Server 快照不足以证明安全接纳。

## 并行归属与验收

1. `companion_policy`：纯类型、候选、决定和 deterministic replay 单元验证。
2. `companion_replay`：持久 repository、规范来源装载、迁移与擦除、真实 PG 用例。
3. `companion_delivery`：VoiceSession 机会/预约、显式 wire 接纳、Embedded 软件端 fence。
4. Root：服务/API/装配、接口复核、完整有界测试、source manifest、正常本地提交。

CP01—CP07 的完成只按实际代码和测试证据勾选。真实 PG/软件 loopback 不等于 Pi 真机、扬声器已听到、AEC、执行器或 24 小时验收。Web/iOS/Android、通用 Embedded 适配、Pi 运行 Easy-Turn、未来同机部署的实测继续延期并保留原 backlog。
