# V01 Companion 配对流程与时钟审计 — 2026-10-15

初始分工 `settings_model`，续接复核 `/root/v01_flow`，只读审计基线 `c685e5a`。本分工不修改生产或测试，不运行 Cargo、数据库、模型、SSH 或硬件；Root 统一当前二进制、运行证据与提交。R03 继续暂停，默认 shadow、25 秒成功测试期限、原机会期限及默认并发保持。

## 已完成检查

- [x] 读取 [V01 待办](../../deferred.md) 和 [2026-10-04 runtime HANDOFF](../2026-10-04/runtime.md)，区分旧 V3 缺失 artifact 与 V4/current 成功记录。
- [x] 核对配对 fixture 的实际 25 秒范围、heartbeat、阶段名称与失败取证顺序。
- [x] 核对 scanner 的 shadow/live 分支、候选发现、attempt fence 和单 channel 所有权。
- [x] 核对 actor snapshot/reserve、2 秒 capture 新鲜度、5 秒机会及 mailbox 等待。
- [x] 核对 prepare/admit/COMMIT/输出的独立时钟和原期限，不将静态候选当根因。
- [x] 核对 ScanProgress 与日志过滤能证明/不能证明的范围，并向 Root 提供有界诊断建议。
- [x] 输出可区分证据矩阵，保留当前运行结果由 Root 收口。

本清单 **7/7 为流程审计完成数**，不是 V01 修复或当前配对运行通过数。

## 流程与预算

`tests/duplex_playback/companion.rs::complete_invitation` 的 25 秒覆盖连接 voice client、Open、Ready、启用隔离测试政策、seed/Appraisal、InitiativeAdmitted、TTS、配对提交/撤销或恢复交接，以及最终断言。paired Runtime、ServerBuilder build 和绑定 hydration 在该 25 秒之外。失败摘要在 shutdown 改变状态之前读取，独立最多 1 秒；关闭所有 owner 后才重抛原结果。

成功 fixture 已在 Ready 后启动每 100 ms 一帧的合成静音 heartbeat，随后才启用 Enabled、生成一次来源。此 heartbeat 不是真实录音；测试政策也不改变产品默认 shadow。

生产流程为：

1. `companion/service.rs` 的扫描 interval 为 5 秒、MissedTickBehavior::Skip。每轮 shadow scan 有 5 秒局部预算，然后执行 live retire/discover。live 受 4 个任务、每 channel 单 owner 和 1024 个本进程 attempt/inspected fence 限制。数据库发现每批最多 32 个。
2. `live/repository.rs` 从仍合法且在 Enabled cutoff 之后的来源创建候选并永久记录 source claim；源/权限校验失败、容量限制和 SQL 等待可阻止候选产生。shadow 的 processed/sources 统计不代表这个 live 分支。
3. runner 取得 actor 原机会，`VoiceInitiativeHandle` mailbox 容量 4，snapshot/reserve 请求各有 **5 秒**等待上限。actor 在 credentials/owner await 后排空 capture；最后一帧按 Server `SystemTime` 判断不超过 2 秒，机会同时记录 Server UTC 和单调时钟的 5 秒截止。
4. `prepare_intent` 使用 PG `clock_timestamp()`，要求 `observed_at <= PG now < admit_before`；`admit_before` 固定为候选 expiry、机会 expiry、最后 capture＋2 秒、下一 UTC 日中的最早值。原输出 `valid_until` 仍是候选 expiry，不以重试/暂停刷新。
5. runner 最多对同一 intent 做 3 次 admission reconciliation，间隔最多 100 ms；每次等待受 Server UTC 对原 `admit_before` 的剩余量约束。PG 事务自身有逐 statement 2 秒限制，不等于全事务或连接池 2 秒上界；PG COMMIT deferred constraint 再查原准入期限。
6. Fresh 后 actor reserve 再核对同一机会、scope、speech revision、原 UTC/单调截止、Context 与绑定；fence 后再检查原准入期限。成功后才返回原输出 owner。NoOpportunity 可以释放 attempted fence，但已冻结失败/不确定结果不会取新机会重做；退休和拒绝仍是合法失败结果，不能被改成成功。

时钟应分开解释：阶段 `Instant` 只测同进程等待；runner 的 `remaining_ms` 是 Server UTC 对合同期限的评估；持久准入由 PG UTC 判断；capture 时间是 Server 收到帧的时间，不能拿它宣称设备采样或跨机传输延迟。Server/PG 偏差是待测假设，尚无本轮证据支持“时钟偏差造成历史 V3 失败”。

## 可区分的候选与证据

| 等待/失败观察 | 下一步应保存的最小证据 | 能支持的判断与限制 |
| --- | --- | --- |
| `connect/open/Ready/seed` 阶段超时 | 固定阶段、原 deadline、相同 binary/input、数据库等待分组 | 未到 live admission，不能归因为 2 秒机会过期 |
| `receive InitiativeAdmitted`，无 runner phase | ScanProgress、live pending/source claim/rejected 的无身份计数、service 固定日志 | 区分 scanner/发现/创建前后；仅 `sources_seen=0` 不能判断 live discovery 未运行 |
| `voice_snapshot ready=false` | 该 phase 单调等待、pending 是否仍存活、固定 NoOpportunity 结果 | 可指向无 actor、mailbox/credentials 等待、busy 或 stale capture；当前 bool 不能区分具体拒绝 |
| `freeze_intent ready=false`，candidate `attempt_failed` | freeze 等待、snapshot 后至 PG 检查间隔或无身份 deadline 分类、Server/PG 时间范围 | SQL 不可用、原机会过期或 `observed_at > PG now` 均可能；不得凭统一 reason 宣称时钟根因 |
| `admit outcome=deadline/retired/error/deferred` | attempt、原剩余量、单调耗时、pending/admission state/reason、PG state/wait_event_type 分组 | 区分原 2 秒窗口耗尽、已退休和策略推迟；PG 采样/总 query 耗时不等于连接池等待单项 |
| `admit outcome=fresh` 后收不到 event | 原 admission 存在与 reason、service `original_output` 阶段、fence/reserve 的固定诊断 | 准入已完成；后续 actor 再验证/输出阶段失败不能回填为 admission 未提交 |
| 已收到 event，后续 TTS/media/恢复阶段超时 | 已有固定 stage、一次 response/admission/TTS/output 的计数与 owner 清理结果 | 属于原输出流程；不应调大 scanner/机会 TTL 来掩盖 |

现存日志/统计有两个确定的观察缺口，而非已经确认的生产故障：`ScanProgress` 只记录 shadow scan；fixture 的日志过滤仅启用 `companion::runner`，因此看不到 `service` 的 live discovery unavailable、scan batch deadline 和 `original_output` 失败阶段。建议由拥有测试的分工有界补齐固定 namespace/无身份计数，仍保留失败查询 1 秒总预算，不记录 owner/source/device ID、private grants、内容或原始 SQL 错误。

对 `service` namespace 的全部五个现有 tracing 点已经逐一核对：`scan_failure` 的 stage 只有 prune/discovery/evaluate，class 只有 invalid_input/conflict/forbidden/erased/unavailable；scan deadline 和 live discovery 警告没有动态字段；`RunOutcome::Failed` 的 stage 只有 freeze_intent/original_output；Observed 的 `DeliveryStatus` 是不携带 payload 的固定枚举。此处没有原始 error、私有身份或文本，因此测试过滤器可有界增加该 namespace；不是建议开启整个 Companion/数据库/HTTP debug。

对失败 SQL 的最小建议仅增加两项，不增加额外 active/年龄/身份列：

- `companion_source_claims` 按 `origin_mode`（schema 限定 shadow/enabled）分组 count。它记录不可更改/删除的源占用，Enabled claim 与 live pending 同事务提交；Shadow claim 会令后续 live discovery 去重跳过。claim 不是 admission，也不是输出完成，需与已有 pending/admission 分组一起看。
- `companion_rejected_sources` 总 count。它只记录 `load_companion_source` 判为 Revoked/PolicyChanged 后持久化的 owner/appraisal 排除钉，shadow/live 两支都会写；普通 NoOpportunity、SQL 不可用、容量失败和机会 Expired 不在此计数。不能把这个数字写成“所有拒绝次数”。

已有 appraisal 总数、response 总数、pending/admission state/reason 与 PG wait 分组继续复用。当前隔离 fixture 只有该案例的自建数据；这些计数不供生产跨用户查询或暴露。若已有证据无法定位具体分支，下一次按真实失败追加必要诊断，不预先把整张判定表复制进测试。

现有确定性过期回归 `commit_checks_original_short_admission_deadline_without_refreshing_output_deadline` 已覆盖持有真实事务直到原准入截止、COMMIT 拒绝、Context/Response/grant 回滚及同 intent 后续退休；应复用它，不能另造延长期限的“成功”测试。当前正常批次即使全过，也不能证明旧 V3 初次超时已修复；失败 artifact 缺失、当时资源争用和时钟状态仍是历史证据缺口。

本分工没有修改 Settings、Companion 生产代码或期限。最终运行数和当前诊断变更以 Root HANDOFF 为准。

## 续接收口复核

重新核对 scanner、runner、live repository、actor mailbox 和最终 `companion.rs` 差异后，未发现此次诊断修改改变机会、重试、策略、并发或 owner 清理。当前修改落实了上述确定的观测缺口：仅增加 `service=warn` 固定命名空间；将摘要标明为 `shadow_scan`；增加 source claim 的 shadow/enabled 分组与 rejected marker 计数；内层 timeout 和 panic 均在 cleanup 前输出原最多 1 秒的脱敏摘要。成功路径与最终原 panic 恢复方式保持。测试修改没有将 shadow 统计解释为 live scanner 进度。

本分工直接按最终 source manifest 重新计算全部 **669** 个输入的字节数和 SHA-256，当前内容与 `27d16d5b28bc4b0dcc60a92b2522d72a330fb56e2da9a1ace2483835ad769093` 清单逐项一致。复读已存在的原始日志与 JSON，未重跑测试：同一 baseline binary 默认并行两批各 **9 passed / 0 failed / 0 ignored**；诊断版一次 workspace 上下文 **1245 passed / 0 failed / 5 既有 ignored**，其中 duplex 为 **10 / 0 / 0**，新增 SQL/隐私回归通过。两批 baseline 不重复计入最终 unique total；编译 `build_jobs=2` 不是 libtest 并发数，测试并发保持默认。

现有证据没有复现旧 V3 的 25 秒超时，故 **V01 继续 open**。确定修复的是失败取证缺口，不能据此宣称旧生产等待或历史资源争用已修复。`service=warn` 不包含 Observed 的 debug 事件；SQL 汇总也不能分解 pool 等待、扫描步骤或 actor 拒绝原因。外层 45 秒直接 drop、初始化之前和 cleanup 阶段不在新增内层取证覆盖内；这些边界继续明确保留，不为一次未复现调查增加框架或生产埋点。

本分工未修改源码、未运行新的 Cargo/PG/SSH 命令。最终资源回收、证据归档、commit 身份及总 checklist 由 Root 统一核验；待再次出现真实失败或取得原 failing artifact/可匹配负载时，沿上述阶段矩阵继续定位。
