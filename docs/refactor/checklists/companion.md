# R01 · Companion shadow、回放与受约束主动表达

更新：2026-09-28。当前 **CP01—CP07：7/7**；源码已正常提交 `a79cb1f`，630 输入逐项与冻结 V1 一致。默认政策保持 shadow，真实 Server 装配现在支持 owner 显式启用 Enabled；没有启动用户设备或更改用户政策。

- [x] **CP01 契约与状态归属**：候选/机会/决策、来源与删除边界见 [契约](../companion-contract.md)。
- [x] **CP02 纯策略与可回放输入**：显式时间、政策版本、完整证据及预算；确定性回放不调用实时模型或传感器。
- [x] **CP03 持久候选与权限**：shadow 持久去重、有效期、来源失效、政策与有界状态；live 另由 CP05 管理。
- [x] **CP04 Shadow 运行装配**：真实生命周期、owner REST 入口、容量与关闭排空；shadow 不生成 Response、媒体或动作。
- [x] **CP05 规范投递整合**：原子 admission/专用 Context/唯一 Response，原机会 reserve、固定邀请一次 TTS 和规范 Delivery；设备原 capture watermark 防止尚未上行的新语音被抢占。
- [x] **CP06 故障与回放验证**：删除、撤权、重启、重复、预算/容量、机会过期、reserve 拒绝/未知/取消、TTS 期间撤权、近端语音、H1 ACK 丢失后 H2 恢复均有软件测试证据。SQL 未知提交用真实事务 barrier 验证，未做 PostgreSQL 物理 commit ACK 断网。
- [x] **CP07 完整验收与提交**：630 输入 V1 全量、真实 PTY、模型、平台检查与证据归档通过；`a79cb1f` 已提交且输入一致。

## CP05 已实现出口

- [x] `CompanionInvitation` 只允许 ResponseCommit / OutputAdmission；完整 Appraisal 来源根与新的 initiative request 固定不变。
- [x] TTL 内等待独立机会；政策、来源墓碑、同主题和不可退款预算与唯一 Context/Response 在同一事务准入。Fresh permit 不可序列化/重建，Existing 只核对原事实。
- [x] 分离 `admit_before`（提交与首次 reserve 的短期限）与 `valid_until`（原候选输出期限）；Hold、恢复及重试都不延长期限。
- [x] 精确 v1 capture capability；Register / Start 在真实 PlaybackSession 锁内检查原 N，本地新语音先 Hold，随后正式 Pause 接管。
- [x] paired scanner → actor reserve → Context/Response → 固定邀请 TTS → Delivery；删除来源后零首媒体，原 producer/native owner 不被换新。
- [x] H1 Resume Applied 后新本地 speech、丢失 ACK、Observe、H2 Pause、fresh ExplicitContinue：原 cursor 40 + 120 = 160，唯一 Response/admission/TTS/Push。
- [x] Runner 原意图最多三次有界对账；同 channel 一次/global 四次；扫描轮转排除已尝试项；停机取消并排空原 owner。
- [x] 完整 Runtime 的 Enabled 持久 CAS、owner 隔离与重启覆盖；默认 shadow。

## 本轮证据

冻结 V1：630 输入；manifest SHA-256 `58c76e82adf71fed96fa6b113f51185412dd3bd0619a3db747835f3d81accd4a`。完整 `cargo test --workspace --locked --offline` **1036 passed / 0 failed / 5 ignored**，493.98 秒，默认栈/默认测试线程，运行前后输入无漂移。按 66 Running + 5 Doc-tests 的父 summary 计数，排除一个嵌套子进程通过项，不重复累加定向测试。

开发期定向 Companion 38、Runtime/HTTP 7、Core/Proto/Embedded/actor 50、完整 duplex 9 均通过；首次 import 编译失败、SQL 擦除预期错误、paired 身份握手与示例重复 identity 失败分别保留。最终计数只使用统一冻结轮次。

实际 PTY 3/3 和 Easy-Turn 2/2 已补跑，合计 **1041 项不同 macOS 测试**。macOS 与 Linux ARM64 两端 no-default all-targets 通过；Linux 本包仅编译。所有阶段源码无漂移，fmt 通过，测试 PG schema/连接均已清理。

上轮 619 输入 / 1010 unique 仅属于已提交的 shadow 基础 `98f237c`，不表示本轮 live 源码曾经过那次测试。当前 [恢复入口](../resume-checkpoint-2026-09-28.md) 和 [四份 HANDOFF](../handoffs/2026-09-28/README.md) 记录统一证据。

## 保留边界

- [ ] Pi 真实麦克风/声卡/扬声器、AEC、物理静音/恢复听感与 24 小时整机运行，归 R02/R05。
- [ ] 执行器位置反馈和真实 D3 动作，归 R03。
- [x] 2026-10-17：普通 Generation 对已提交邀请历史的只读引用完成，源码`4e2968f`；原邀请不扩权，来源/真实TTL/隐私与当前政策保持，见[本轮10/10](invitation-history-2026-10-16.md)及[契约](../companion-invitation-history.md)。
- [ ] 前端、通用 Embedded 适配、设备侧模型和同机部署分别按用户原决定延后，不能把 Cargo 编译通过当作部署验收。
