# CP05 Voice / device capture gate handoff — 2026-09-28

状态：本组件代码、定向回归及 scanner → actor → TTS → 配对 PlaybackService 纵向回归完成。2026-09-28 06:58:31 UTC 最后一轮完整配对测试通过，源码交还 Root 冻结；本交接记录尚未包含冻结后的全工作区或跨平台验收。没有启动用户设备或部署。本文不能单独作为 CP05 已完成或真机通过的证明。

## 已落地 checklist

- [x] OutputBinding 的 optional InitiativeCapture 固定原 initiative/request/捕获水位；None 省略 JSON 字段。Core 与 protobuf codec 保留并验证原身份。
- [x] Open/Ready 和 Register ACK 明确要求 initiative_capture_gate_version=1；旧 foundation bool=true 但 version=0 不允许 initiative，不降级普通输出。
- [x] 原 PlaybackSession mutex 序列化真实 capture、Register 与 Start。Register 前更新的 speech 拒绝且不分配 native owner；Register 后包括 Start 前 speech 关闭原 Arc 控制门，保留可恢复原输出。
- [x] local_capture_hold 表达临时本地事实，不伪造 Server hold_id/detection/Applied。首次 local deadline 固定，后续 speech 不续期；原正式 Pause 接管只缩短期限。
- [x] Fresh Resume 同时清除被覆盖的 local/formal gate，原 scope、buffer、cursor 均保留。ExplicitWait 仍只能 ExplicitContinue；旧 Applied Resume 查询遇新 speech 返回当前 Held，不能开门。
- [x] Server 原 OutputProducer 对“旧 Resume 已 Applied、当前只有新 local Hold”在原 owner 中产生 H2/Pause；短控制队列承接，不换 scope、不重建 Response、不重播音频。纯 Gate 单测验证接管及不续期。
- [x] move-only LiveAdmissionPermit 是 reserve 请求唯一构造输入，实例取同一 VoiceInitiativeHandle；actor 复核专用 Context/admission/current source/expected scope，并在建立输出前严格检查 admit_before。
- [x] wire N 固定为 permit 原 opportunity.through_sequence；新 silence 不更新 binding，新的 speech/revision 仍拒绝首次 reserve。建立后的原 producer 受固定 output deadline 约束，后续 Resume 不刷新机会。
- [x] Control register/admit 经统一 Context 当前授权复核，再复用 A 的 checked grant reader 校对原 request/initiative/context/scope/N。原控制期限不得宽于 grant.valid_until；此路径不把短 admit_before 当持续输出期限。

## 本组件真实验证

证据目录：`run/validation-20260928/`。这些是开发期间的定向检查，`frozen=false`，不宣称 source manifest 全量一致。所有命令使用 jobs=2、默认线程与栈、未设置 DOCS_RS、真实 Sherpa archive。没有增加依赖或伪造 native 构建。

| 命令 | 结果 | 证据前缀 |
| --- | --- | --- |
| `cargo test -p alia-core -p alia-proto --lib --locked --offline` | Core 31 + Proto 5，0 failed，0 ignored；exit 0 | `voice-core-proto-v1` |
| `cargo test -p alia-embedded --lib initiative --locked --offline` | 4 passed，0 failed，0 ignored；exit 0 | `voice-embedded-v1` |
| `cargo test -p alia-server --lib initiative --locked --offline` | 9 passed，0 failed，0 ignored；exit 0 | `voice-server-v1` |
| `cargo test -p alia-server --lib applied_resume_handoff --locked --offline` | 1 passed，0 failed，0 ignored；exit 0 | `voice-handoff-v1` |

此组共 50 个唯一通过测试，包含本次新增的 8 个测试；不要与后续重复运行相加作为唯一测试数量。Server initiative 使用真实任务 PostgreSQL `127.0.0.1:32768/alia_refactor_delivery_final`；组件定向测试结束时实际查询非 public 系统外 schema=0、其他连接=0，该快照不代表之后配对测试或 Root 冻结验收的状态。当前 agent 没有运行中的 Cargo、PG 测试、模型服务或验证容器，也没有 Git 提交。

新增 8 项：

1. `initiative_local_hold_survives_old_applied_resume_and_transfers_to_next_hold`
2. `initiative_capture_requires_original_clock_and_local_deadline_never_refreshes`
3. `initiative_none_preserves_binding_json_and_some_binds_the_original_zero_watermark`
4. `initiative_binding_requires_exact_capability_and_local_hold_survives_codec`
5. `initiative_capture_before_registration_rejects_without_allocating_a_native_owner`
6. `initiative_register_then_speech_before_start_retains_original_media_for_cpal_and_alsa`
7. `applied_resume_handoff_keeps_owner_closed_and_never_refreshes_original_deadline`
8. `initiative_registration_requires_canonical_capture_and_original_output_deadline`

Embedded 两项使用真实 Session/Control/PlaybackSender/PlaybackBuffer，并用线程屏障制造 capture 与 Register/Start 的锁竞争；调用真实 CPAL callback buffer 与 ALSA partial-write 记账辅助路径。已进入的一帧仍可完成，后续样本受门控制，Resume 只继续未提交部分。这是 native 软件提交边界的测试，没有真实麦克风、扬声器或 ALSA 设备，不证明声学静音、AEC、实际双讲识别或 Pi 性能。

## 完整配对纵向验证

复用 `tests/duplex_playback.rs` 的原 Runtime/Device/Proxy，并在 `tests/duplex_playback/companion.rs` 接入真正的 ServerBuilder、VoiceSessionService、Companion scanner/runner、Delivery、PlaybackService 及 PostgreSQL。设备状态通过实际 DeviceBindingService 握手装入原 EmbeddedChannel，入站 VoiceSession 使用匹配绑定公钥的签名 token；没有绕过生产身份校验。

| 命令 | 结果 | 证据文件 |
| --- | --- | --- |
| `cargo test -p alia-server --test duplex_playback companion:: --locked --offline` | 正向及源删除 2 passed，0 failed，0 ignored；exit 0 | `paired-companion-v3.log`、`paired-companion-v3-result.json` |
| `cargo test -p alia-server --test duplex_playback applied_resume_lost_ack --locked --offline` | H2 竞争 1 passed，0 failed，0 ignored；exit 0 | `paired-companion-h2-v1.log`、`paired-companion-h2-v1-result.json` |
| `cargo test -p alia-server --test duplex_playback --locked --offline` | 完整 9 passed，0 failed，0 ignored；exit 0；20.46 秒 wall，测试 15.39 秒 | `paired-full-v2.log`、`paired-full-v2-result.json` |

证据均位于 `run/validation-20260928/`。完整 9 项包含原 6 项和本次 3 项；上表定向与完整运行有重叠，不累计为 12 项。`paired-full-v1` 也通过 9 项；v2 是补强唯一 Response/admission 和 Register 原 binding 断言后的最终开发检查。它仍为 `frozen=false`、`source_drift=null`，不能代替 Root 前后核验源码 hash 的冻结报告。较早 `paired-companion-v3` 的 `source_drift=[]` 是旧开发 runner 的占位值，没有执行 hash 验证，含义同样不是冻结通过。

H2 场景先从原 160 个 PCM source frame 中提交 40 个，再上传 speech 建立 H1。第一份 fresh 检测返回 Backchannel，H1 Resume 已在真实设备服务 Applied 后，代理暂扣 ACK；此时新 speech 先进入原本地 Session，暂缓上传，后续 silence 也暂停上传以保持捕获序列连续。断言原 H1 已结束而 local Held 存在、提交门关闭，再释放代理并丢失 ACK。原 Delivery 使用 Observe 取得旧 Applied 与当前 Held，产生同 binding/backend/scope 的 H2/Pause。随后只上传原来暂存的 speech，一份 fresh 检测返回明确“继续”，以 ExplicitContinue 恢复 H2。最终仅提交剩余 120 帧，总计 160 帧，原 native generation/cursor 不变；一次 TTS、一次媒体 Push、唯一 Response、唯一 admission、两条 chat message、零 execution effect。

撤权场景在原 TTS 合成期间删除源，待原 runner/output 完成 shutdown 后再次断言从未发出第一媒体，并保留原计费/裁决事实。正向场景检查实际注册 binding 对齐原 Context、Response、scope 与 admission capture 水位。

早期失败与修复已保留原日志：

- `paired-companion-v1`：编译成功、两例失败，VoiceSession 报 `embedded device public key is not available`。fixture 只配置了 expected trust pin，未通过正常握手装入运行时 bound peer。已补实际 DeviceBindingService，并由 `binding_client()` 走原握手；没有削弱身份认证。
- `paired-companion-v2`：编译成功、两例失败，fixture 解析 shipped embedded example 遇到重复 `[identity]`。fixture 改用编译内默认配置以免绑定测试依赖示例文件；Root 独立修正示例并添加真实配置 loader 回归，纳入最终 workspace 验收。

该纵向测试的 PostgreSQL、双向 gRPC、身份绑定、actor、控制持久化及设备 Session 都是真实路径；TTS、TurnDetector 结果、native frame 提交由可控 fixture 提供。它证明软件协议与原媒体归属下的竞争处理，不证明 Easy-Turn 实际推理、真机扬声器停声时间、声学 AEC、双讲识别或 Pi 性能。

## 设备软件矩阵对齐

已按本轮源码与日志更新 [设备实施合同](../../companion-live-device-design.md)：删除“production gate 仍必须 false”“actor 最后取新 capture 水位”“配对尚未运行”等旧状态，分别改为实际 live 依赖接线与 Shadow 默认策略、permit 的不可变原 N、已通过的组件及完整配对矩阵。

- [x] Core 状态、Proto 身份/能力、Session Register/Start 锁竞争、CPAL/ALSA 软件提交、事件重排、actor 封闭 Harness、真实 PG 控制准入、完整正向/H2/撤权及原配对 6 项均对齐到实际测试证据。
- [x] 对原计划“fresh Resume 先于第一 Start”和“initiative local Hold 恰遇满队列”完成只读生产路径审阅：前者复用已测试的 Core `released_through`/Start validator 和原注册 owner；后者由原 capture 同步写 HELD atomic，媒体回压前已释放 Session 锁，与 formal Hold 共用 native gate。两项没有独立新生产分支或当前发现的实质风险，不需为穷举新增配对排列，不作为 CP06 阻塞；完整代码/测试对应记录在设备合同第 7 节。
- [x] 明确区分“组合路径审阅完成”和“该独立排列已实跑”；没有把组件通过推导成 CP07 全通过。

## 验收与后续 checklist

- [x] 完整 scanner → durable Fresh → 原 VoiceSession actor → 原 OutputProducer → 固定 TTS → 真实配对 PlaybackService 正向通路。
- [x] 配对 gRPC 丢 ACK / Observe 在 H1 Applied 后本地 N+1 speech 的竞争：同 owner H2/Pause 接管、fresh Continue 正确恢复，不永久退休也不困在旧 H1。
- [x] TTS 过程中撤权等待原 runner/output 收尾后再检查无第一媒体。
- [x] 原完整配对 6 项与本次 3 项全部通过；开发期 Cargo/PG 窗口已交还 Root，无活动测试进程，组件源码不再修改。
- [ ] 整体源码冻结后 macOS workspace、实际 ignored 场景及 Server/Embedded no-default + Linux ARM64 检查，由 Root 统一计数和记录。
- [ ] 真实 Pi5 + macOS 音频、VAD/AEC、双讲、尾音、恢复完整性及长时间运行另行验收；前端和一般嵌入端适配保持延后。

Root 已将 production builder 的 live 依赖及 actor gate 接入待验收工作树，以上端到端测试由真实路径驱动。默认策略仍是 Shadow；内部 snapshot-only actor Harness 继续保持关闭门。完整 CP05 结论需同时引用 Root 冻结验收及剩余范围，不能用接线或本组件报告代替。
