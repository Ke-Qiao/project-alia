# Companion policy 本轮交接（2026-09-26）

Root 收口补记：V3 已完成，workspace 1005/0/5、5 项 ignored 单独通过、两平台 no-default 检查通过；详见 [统一摘要](validation-summary.json)。下文“运行中”保留为组件交接当时状态。

交接者：`companion_policy`。用户要求质量优先，当前目标是 macOS Server + Pi 5；前端、通用 Embedded 适配与其他部署形态继续延期。

**本文件保存交接时的状态：V3 Companion 定向 26 项全部通过，TTS HTTP 定向 9 项全部通过；完整 workspace 仍在运行，未宣布全量通过。** 最终 source manifest、全量统计、平台结果和提交由 Root 统一收口。当前不得修改源码或自行启动 Cargo / PG。

## 已完成的纯策略领域

本 agent 实现并交付的文件：

- [`companion/types.rs`](../../../../packages/alia-server/src/modules/companion/types.rs)
- [`companion/decision.rs`](../../../../packages/alia-server/src/modules/companion/decision.rs)
- [`companion/decision/tests.rs`](../../../../packages/alia-server/src/modules/companion/decision/tests.rs)

纯接口已经冻结：

```rust
generate_candidate(&CanonicalObservation, &PolicySnapshot, created_at)
    -> anyhow::Result<Candidate>
decide(&DecisionInput) -> anyhow::Result<DecisionRecord>
replay(&DecisionInput, &DecisionRecord) -> anyhow::Result<bool>
canonical_digest(&impl Serialize) -> anyhow::Result<String>
```

首轮只有 `FollowUpInvitation`，固定文本为“如果你愿意，我们可以再聊一会儿。”，不调用 LLM，不复制原文或自由格式私密理由，不声称已经实现完整 Affect / Regulation 语义。

默认 Shadow；初值为候选 TTL 60 秒、cooldown 900 秒、UTC 每日预算 3、等待新回应。quiet / paused、来源和政策版本变化、未知身份、占用、机会或 capture 失效等为独立门槛。Simulation 最多得到 ShadowWouldAllow，永不 Allowed；旧 Shadow 候选不能因后来启用政策升级权限。

CanonicalObservation 包含原 Appraisal 引用及原 effective_at。candidate created_at 必须等于 effective_at；dedup / ID 由 owner、source event、固定 kind 和 generator version 决定，不含新 operation、政策版本或 Appraisal ID。来源时间、原有效期与候选 TTL 共同限制寿命，重建和扫描不可刷新。

输入显式提供 as_of、完整冻结政策、OpportunitySnapshot 和 BudgetSnapshot；replay 不读取 SQL、墙钟、模型或运行时状态。严格字段 / 数组 / 字节 / 时间边界，稳定原因枚举及 canonical digest。保留合法 chrono 纳秒精度；真实空闲窗口允许 speech revision 为 0，但仍必须有匹配 capture 时间和新鲜证据。`OpportunitySnapshot::unavailable(as_of)` 用于未知生产事实，模拟 factory 仅供明确离线用途。

**纯策略 11 项测试均在 V3 定向日志中通过**，覆盖：模拟限制、跨政策 / rebuild 的稳定去重、源期限与引用排序、Shadow 不升级、硬门、live capture freshness、半开期限、预算 / cooldown / 回应 / 去重、UTC 换日、JSON 顺序与纳秒 replay、未知字段和越界输入。它们是领域验证，不证明真实输出已经开放。

## TTS HTTP fixture 修复与证据

本 agent 仅修改 [`gpt_sovits/tests/http.rs`](../../../../packages/alia-server/src/providers/tts/gpt_sovits/tests/http.rs) 的 `Peer` 生命周期，未改 TTS provider 生产逻辑、现有断言或任何超时门槛。

V1 四个 TTS 失败都发生于 `Peer.close` 的 3 秒 graceful shutdown，而不是 payload / budget 断言。只读检查本地依赖源码发现：Axum graceful shutdown 等待存活连接，不终止 fixture 故意制造的无限 body；原 handler 不消费 POST body，Hyper 的一次快速 drain 若未读齐会关闭读侧，从而使 pending 响应的收尾依赖不可靠。原 close 把 JoinHandle 移入 timeout，超时后会 detach，Drop 已拿不到 handle。

实施的三个修复：

1. 用 Axum `Bytes` extractor 在回响应前消费真实请求体。
2. 每个真实 Response body 用 `take_until(peer_stop.cancelled_owned())` 包装，只在全部测试断言完成、fixture close / Drop 时终止 pending body。
3. 保持 3 秒关闭期限，以 `&mut JoinHandle` 等待；超时则 abort + await，之后仍明确 panic，不能把强制终止当作通过。

由 Root 执行的 [TTS V3 日志](../../../../run/validation-20260926/tts-http-v3.log) 为 **9 passed / 0 failed / 0 ignored / 269 filtered out**；[result](../../../../run/validation-20260926/tts-http-v3-result.json) 的 exit_code 为 0，source_drift 为空。该 agent 只运行过此文件的 rustfmt 和 diff whitespace 检查，未独立运行 Cargo / PG。

## 请求排空只读复核

对 Root 的 [`companion/service.rs`](../../../../packages/alia-server/src/modules/companion/service.rs) 和 [`server_runtime/companion.rs`](../../../../packages/alia-server/tests/server_runtime/companion.rs) 做过只读语义复核，没有修改这些文件。

复核确认：

- `admit` 在同一接纳 mutex 中检查 closed / cancel、取得容量和注册 TaskTracker token；`close` 在同一锁内关闭入口与 tracker，避免 `TaskTracker::close` 本身不阻止新 token 的竞争。
- `stop` 等待原 worker 的 `&mut JoinHandle`，等待者被取消时不会 take / 丢失任务所有权；后续 stop 仍等待同一请求集合。
- Producers 阶段排空后再关闭数据库依赖。调用方取消释放自己的请求 token，不另造后台任务去重试未知提交。
- 新增三个测试验证真实 SQL 阻塞下的 stop 排空和重复等待、8 个容量占位及取消释放、HTTP 提交前超时后使用原 operation 重试。

这次只读复核没有捕获 TaskTrackerToken 的错误 import 路径；V2 编译实际失败，不能把语义复核描述成编译通过。Root 随后修正为 `tokio_util::task::task_tracker::TaskTrackerToken`，并重冻结到 V3。

三个测试的证据边界仍需保留：HTTP 用例是 **提交前超时**，不是提交 ACK 丢失；没有人为强制 close / admit 同步竞争。Root 后续增加了 opaque ScanProgress 和真实第二来源处理的 scanner 重启进度证据，本 agent 按分工未重复审阅这轮修改。

## 验证历史必须完整保留

| 运行 | 实际结果 | 证据 |
| --- | --- | --- |
| V1 workspace | 996 passed、5 failed、5 ignored；Cargo exit 101 | [log](../../../../run/validation-20260926/workspace-v1.log)、[result](../../../../run/validation-20260926/workspace-v1-result.json)、[去重统计](../../../../run/validation-20260926/workspace-v1-counts.json) |
| V2 Companion 定向 | E0432，`tokio_util::task::TaskTrackerToken` unresolved import；编译失败，没有测试通过结论 | [log](../../../../run/validation-20260926/companion-v2.log)、[result](../../../../run/validation-20260926/companion-v2-result.json) |
| V3 Companion 定向 | lib 20 passed（纯策略 11 + repository 9），runtime 6 passed，共 26；exit 0，source_drift 空 | [log](../../../../run/validation-20260926/companion-v3.log)、[result](../../../../run/validation-20260926/companion-v3-result.json) |
| V3 TTS HTTP 定向 | 9 passed；exit 0，source_drift 空 | [log](../../../../run/validation-20260926/tts-http-v3.log)、[result](../../../../run/validation-20260926/tts-http-v3-result.json) |
| V3 workspace | 交接时运行中；最终结果由 Root 补充，不作全量通过声明 | [正在写入的日志](../../../../run/validation-20260926/workspace-v3.log) |

V1 的 5 项失败为：4 个 TTS HTTP fixture close 超时，以及 `companion_runtime_discovers_canonical_sources_and_joins_worker` 的 scanner 等待超时。不能把这次真实失败抹掉或改标为全通过。

冻结依据：

- V1 source manifest SHA-256：`29fa6e46b174edfa71765d0fb0e71d8bbed1073aab281a3a1a7cfbb6178855d1`。
- V2 source manifest SHA-256：`eaa32696907d0653908b136c9919a22dd03f41406c044d01d52532ad677b2235`。
- V3 source manifest SHA-256：`8ea6d4f7cc63b82d22b8c584c8c0b426b11e04f952f64bd15b7776d352cc18a1`。

本 agent 已提供 [统计脚本](../../../../run/validation-20260926/count_workspace_tests.py) 和 [使用方法](../../../../run/validation-20260926/count-workspace-method.txt)。按每个 Cargo Running / Doc-tests 边界只计最后 summary；V1 共 66 Running + 5 Doc-tests，排除子进程的 1 passed / 276 filtered out，另标注无 summary 的子进程启动。脚本通过纯日志与合成截断父 summary 的校验，不执行 Cargo 或 PG。

V1 命令为默认 features 的 `cargo test --workspace --locked --offline --no-fail-fast`，不是 all-features 或 Linux target 运行。ignored 包括 3 个真实 PTY 和 2 个真实 Easy-Turn / 官方音频用例。软件 PG、loopback、目标编译、Pi 真机、声学双讲 / AEC 和长期运行必须分别记录。

## 下一步 CP05：设计已保存，未实施 / 未验收

完整设计见 [companion-live-design.md](../../companion-live-design.md)，行为约束见 [companion-contract.md](../../companion-contract.md)。Repository agent 另保存了 [CP05 repository 契约草案](../../../../run/validation-20260926/cp05-repository-contract.md)，实施前由 Root 协调接口一致性。当前不能因存在设计、旧 Enabled 配置或纯决定 Allowed 而开放用户设备。

Context 最少必要修改范围：

| 文件 | 必需改动 |
| --- | --- |
| `modules/memory/context/types.rs` | `ContextPurpose::CompanionInvitation`、`ContextEvidence::CompanionAdmission { id, intent_digest }`；只授权已有 ResponseCommit / OutputAdmission |
| `modules/memory/context/repository.rs` | 专用 purpose 恰有一个 admission 的结构检查，evidence 排在完整 source roots 之后，逐次调用当前 admission / policy 验证 |
| `modules/memory/context/companion.rs` | `issue_invitation_in_transaction(tx, owner, admission_id)`；从规范 SQL 自行装载，保持完整根、原时间和 epoch；现有根枚举遇到不允许的二次派生必须显式拒绝，不能静默漏根 |
| 新独立 migration + `modules/platform/migrations.rs` | admission / pending 持久契约、不可变身份和撤销闭包；更新当前 `context_leases_purpose_check` 以接受专用 purpose，不能只增加 Rust enum |
| 相应 PG 契约测试 | 新 request 与旧 parent 共存、用途限制、完整依赖、精确内容、政策 / 来源竞争撤销和幂等 |

`context.rs` 已有 `pub(crate) mod companion`，通常无需修改导出；不扩大 Generation，不修改原 Appraisal parent header，不顺带放宽普通历史 materialization。

Delivery 必须联动限制的范围：

- `modules/delivery/repository.rs` 的 prepare、已存 Response 再验证、`mark_generated_in_transaction` 的 **final_content**、media prepare；绑定完整固定 envelope、output key、kind、admission 和 expected scope。不能只检查 text 或 prepared content。
- `modules/delivery/repository/text.rs` 的 chunk / End：若允许文字出口，就按 UTF-8 字节位置限制固定 literal；否则显式拒绝该 medium。
- 逐次 OutputAdmission / ResponseCommit / Resume 等验证必须读当前持久 Companion policy 与 active admission，不能只相信冻结 decision JSON；撤权仍允许取消、清理和迟到事实记账。
- Companion repository 同事务占用最后预算、发行专用 Context、复用 `DeliveryRepository::prepare_in_transaction`，commit 后才允许 actor reserve / TTS。Fresh permit 不可从 Existing、查询或未知 ACK 再构造。
- 真实 generation 仍由原 VoiceSession actor 唯一分配。新 initiative request_id 不改变 Appraisal row 与原 parent 的 request_id；现有 parent validator 已支持新派生 lease 保持完整 roots。

设备端仍有必须关闭的竞争窗口：speech 已在本地捕获但尚未上传时，Server drain 不能看到它。首次 Register / Start 必须在 capture owner 的同步屏障内验证原 opportunity watermark，并保留登记后的本地阻断，覆盖 Fence / SessionEvent / Register / Start 乱序。已有输出的近端语音必须沿原 owner 建立 **可恢复临时避让**，新鲜语义确认附和后恢复原未提交媒体；ExplicitWait 等明确继续。不能用永久取消替代已确认的可恢复规则。

## 恢复工作时的检查

- [ ] 先读取 Root 最终 V3 全量及平台结果，不把本文的运行中状态当成最终验收。
- [ ] 保留 V1 / V2 失败及对应 source manifest；最终统计排除子进程重复 summary。
- [ ] 等 Root 完成统一提交并明确解除下一代码包冻结后，再分配 CP05 文件所有权。
- [ ] CP05 先对齐 durable admission 与 Context / Delivery 精确内容接口，再接真实 actor / 设备门；不提前开放 Enabled。
- [ ] 真实设备、AEC、双讲、长期运行和延期平台仍按各自证据验收。

本次 handoff 写入仅修改本文件；未运行 Cargo / PG，未提交，之后待命。
