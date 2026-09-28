# Context / Delivery / Runner 故障测试 HANDOFF · 2026-09-28

交接者 `companion_policy_28`。Context/Delivery/runner 定向与 macOS 验证由 Root 运行；本 agent 在 Root 明确移交独占 Cargo 窗口后完成 Linux ARM64 编译，未运行 PostgreSQL、未提交。当前 V1（630 inputs）的 Server/Embedded Linux no-default all-targets 检查均通过，容器已停止、窗口已释放。本文件不代替 Root 最终 workspace 数字、提交或真实设备验收；CP07 由 Root 收口，不在此声明通过。

## 范围与当前结果

继续上轮 CP05 WIP，保持 [主设计](../../companion-live-design.md)、[持久存储契约](../../companion-live-storage-design.md)、[设备契约](../../companion-live-device-design.md) 中的同一原身份和两段期限。Context / Delivery 补全、对应 5 项真实 PG 用例、1 项 purpose 用例，以及 runner 4 项真实 PG + 明确假 actor 故障用例已完成。

Root 的 [companion-v3.log](../../../../run/validation-20260928/companion-v3.log) 为 **38 passed / 0 failed / 0 ignored**，其中上述 10 项全部通过。[result](../../../../run/validation-20260928/companion-v3-result.json) 记录 exit 0、46.70 秒总耗时、测试 7.82 秒、jobs 2、默认线程/栈；**frozen=false**。该次运行发生于冻结前，后续配对整合修改完成后 Root 才建立 V1；不能用这次定向结果代替当前完整冻结验收。

此前真实失败不删除：initial-build 为模块导入错误；companion-initial 为缺少 `TransactionTrait` 导入；companion-v2 编译通过并实跑 **31 passed / 1 failed / 0 ignored**，唯一失败为 Repository 的 pending capacity 用例。我的 5 项 PG + 1 项 purpose 在 v2 已通过；C 修复及扩充后 v3 全部通过。日志与 result 均保留在同一 validation 目录。

## 已实现的授权和内容闭包

- [x] `CompanionInvitation` 只授权已有 `ResponseCommit` / `OutputAdmission`；Memory、Cache、Execution、Appraisal、Affect 等用途全部拒绝。
- [x] purpose 必须恰含一个 `CompanionAdmission`；普通 Generation 携带该 evidence、重复 evidence 或缺少 evidence 均拒绝。
- [x] issuer 从原规范 Appraisal / Context 读取完整 roots，保留原 source event、原时间、conversation/channel、privacy/policy/relationship；只把派生 Context 的 request 换为固定 initiative request，原 Appraisal parent/request 不变。
- [x] admission 首次无锁读取仅用于定位，完整 source roots 在 policy/grant 锁之前验证；再读取锁定 grant 并比较完整值。每次后续 Context 验证都读取当前 policy/grant/source，不能用历史 Allowed 替代当前权限。
- [x] 首次 issuer 另查数据库 `now < admit_before`；已发行 Context 的后续输出只查原 `valid_until = candidate.expires_at`。短机会过后允许保留的原输出继续；新 issuer 和最终输出期限均为半开边界，无扫描/恢复延期。
- [x] prepare、重复 prepare、锁后 stored Response、最终 history content、audio media key/content/scope 都校验固定 typed envelope；额外字段、自由文本、kind/key/scope 变化均拒绝。只准一份 audio representation；text Start/chunk/End 明确拒绝。
- [x] `read_invitation_grant(tx, owner, admission_id)` 是内部只读事实 helper，本身不授予权限；B 在同事务完整 Context 验证后用它核对 control 的 canonical watermark、request、scope 以及 output deadline。
- [x] policy/source/privacy/relationship 撤权后 grant 私密值和 digest 擦除，Response 标记 cancel；原预算/去重不退款，cleanup/Unknown 仍可记录。

源码所有权：`modules/memory/context/{types,repository,companion}.rs`、`companion/invitation.rs` 和其 `tests.rs`；`modules/delivery/repository.rs` / `repository/text.rs`。最小只读 helper由 B 的 control 路径接用。本 agent 没有修改 Core、proto、Embedded 或 voice actor 逻辑。

## Runner 故障测试的真实边界

新增 `modules/companion/runner/tests.rs`，runner 主文件只追加 `#[cfg(test)] mod tests`。复用 C 的真实 schema Fixture / LiveRepository、原 `VoiceInitiativeHandle` mailbox、未初始化的 `DatabaseConversationBackend` 和计数包装的 NoopTts。

四项用例分别为：

1. 一次 Snapshot → 真实 Fresh 提交 → fake actor 明确拒绝 Reserve。
2. 相同真实提交后丢弃 Reserve oneshot reply，模拟未知预约回执；**没有假称 native owner 已创建**。
3. Reserve 在途时取消 runner，原 request token 收到取消，结束后真实 grant/Response 被退休并保留 charge。
4. 保存完全相同的 immutable intent；C 的仅测试 `stage_for_test` 执行原生产事务但暂不 commit。通过 `pg_blocking_pids` 证明 runner 已等待原事务后再 commit；runner 只能返回 Existing，不发送 Reserve、不调用 TTS。

前三均断言只出现原一次 Snapshot/Reserve，原 token 被取消，只有一条 admission/Response，无 Delivery/control/history/Execution 输出；再以原 intent admit 只能得到 6 个 opaque 字段的 Existing，不能重建 permit，也不退款。第四证明真实事务提交分支，**不等于实际数据库 commit ACK 丢失注入**。

这些是 fake mailbox 故障测试，不能冒称 real VoiceSession actor、paired PlaybackService、模型、真实 Pi、native 声学播放或全双工恢复验收。Root 的完整 paired 用例是另一个证据范围：冻结前 [paired-full-v2](../../../../run/validation-20260928/paired-full-v2.log) 的 **9/0/0** 覆盖完整 duplex 软件配对回归，包括固定邀请、TTS 中来源撤销以及 H1 Resume ACK 在途时更新 H2 speech 的原 owner 接管；这里的 detector/TTS/native 样本仍是受控 fixture。

## Root 只读审阅与反馈

独立审阅 Root `runner.rs` / `service.rs` 及完整 paired 测试，反馈四项问题，Root 已报告修复：

- [x] 同 channel 的多个 run 不得并发 snapshot 覆盖第一条已冻结机会；Root 加入 channel 排他，v3 中对应 service 用例通过。
- [x] shadow 串行 snapshot 的 32 × 5 秒最坏耗时不得挡住 live 扫描；Root 将 shadow 与 live 调度分离且保留受管任务。
- [x] paired 撤权后不能以已经立即成立的 retired 状态作“不会迟到发声”的完成见证；应在原任务排空后再验证零 media。
- [x] runner 只把 Submitted / Completed 作为成功收口，不能对 Pending/Sending/Observing 提前 disarm。

Fresh 唯一消费、Existing 不输出、未知事务保持原 intent、原 cancel 贯穿 actor/TTS/Delivery、后续媒体保持固定输出期限的主路径未发现另外的权限绕过。实际行为仍以 Root 最终冻结验证为准。

## 明确未完成 / 不作声明

- [x] Root 已冻结 V1 的 630 inputs，本 agent 完成 Linux 只读快照的 host/container 两次文件哈希核对。
- [x] Root/B 完整 paired 软件路径及 H2 恢复定向已通过 9/0/0；本组件假 mailbox 测试与这份配对证据分开记录。
- [x] Linux ARM64 V1 Server/Embedded no-default all-targets 编译通过；此轮没有 Linux 测试执行，不累计为通过测试数。
- [ ] Root 汇总 workspace 全量、实际 Easy-Turn、PTY、macOS/Linux 证据及正常提交；不能由本组件定向或冻结前配对通过替代。
- [ ] 既有 `materialize` 历史读取只接受 Generation parent，故本轮已持久化的固定 invitation 不会自动进入后续 LLM history；逐条跳过，不导致整轮 materialize 失败。后续需明确设计只读消费边界，不扩大专用 Context 的 Memory/Affect/Generation 权限。
- [ ] 真实 Pi 5、设备音频、AEC/双讲、尾音、恢复完整性和 24 小时运行仍单独验收；Web/iOS/Android、通用嵌入适配、Pi 上 Easy-Turn、同机部署验证维持原延期范围。

Root 另委派本 agent 准备并运行 Linux ARM64 验证。以下先保留环境和源码准备过程，最终实际执行结果见本节末尾。

Linux 环境准备见 [environment-summary.json](../../../../run/validation-20260928/linux-arm64-v1/evidence/environment-summary.json)。新容器 `alia-cp-arm64-20260928-v1` 准备阶段只运行 idle sleep；复用已存在且旧容器无差异的 native image `sha256:6cd7a7a06976f8651cdb57fcde457e9dc7e2abe20587900dd750e1f373a25c70`，无需 commit 旧容器。Rust 1.95.0 `aarch64-unknown-linux-gnu`、ALSA 1.2.8、DBus 1.14.10、protobuf 3.21.12 已读取确认。旧 43 GiB logical cache 用 APFS `cp -cR` 复制到新目录，旧容器、旧 cache 和 registry 未改写。environment-summary 记录的是准备初期源目录仍为空的时点；之后的源码复制状态见下面 source-prepared 记录，不能混作已编译。

新 evidence 目录中的 `prepare-source.py MANIFEST` 只接受已有 Root freeze 且逐文件核对；`run-checks.py` 保留 source/Cargo.lock 前后检查，顺序编译 Server/Embedded `--no-default-features --all-targets --locked --offline -j2`。两步均已实际执行，未创建新的 source freeze。

07:01 UTC 补记：已按 Root 指定的 [V1 manifest](../../../../run/validation-20260928/final-source-manifest-v1.json) 复制 **630 inputs**，SHA-256 为 `58c76e82adf71fed96fa6b113f51185412dd3bd0619a3db747835f3d81accd4a`。host 与新容器只读挂载各逐文件验证全部通过，见 [source-prepared.json](../../../../run/validation-20260928/linux-arm64-v1/evidence/source-prepared.json) 和 [container result](../../../../run/validation-20260928/linux-arm64-v1/evidence/source-prepared-container-result.json)。仍未运行 Cargo / 测试 / PG；Root 正持有 macOS workspace 独占验证窗口。

07:19 UTC 最终执行结果：Root 结束 macOS Cargo 后明确移交窗口，`run-checks.py` 顺序完成两项实际编译，见 [result-summary.json](../../../../run/validation-20260928/linux-arm64-v1/evidence/result-summary.json)。

| 实际命令 | 结果 | Cargo 报告的编译时间 |
| --- | --- | --- |
| `cargo check -p alia-server --no-default-features --all-targets --locked --offline -j2` | exit 0 | 18.84 秒 |
| `cargo check -p alia-embedded --no-default-features --all-targets --locked --offline -j2` | exit 0 | 4.92 秒 |

host/container 的 630 输入前后验证全部通过，`source_drift=[]`，manifest hash 保持 `58c76e82…ccd4a`；Cargo.lock 前后均为 `845d709734b382d24baaec28bd9d69a524e92c4b41366184de02f0ef7a322dfc`。这里是本地 ARM64 Linux 容器的 native 编译，未运行 Linux 测试或 PostgreSQL，也不是 Pi 真机验证。

本次容器已停止，见 [docker-final-state.json](../../../../run/validation-20260928/linux-arm64-v1/evidence/docker-final-state.json)；源码快照、新 cache、原环境和所有日志均保留。已向 Root 明确报告独占 Cargo 窗口释放。
