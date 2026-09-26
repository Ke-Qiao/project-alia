# R01 · Companion shadow、回放与受约束主动表达

更新：2026-09-26。DU01—DU11 已合入且 605 个输入与验证候选一致。R01 shadow / 回放基础已完成定向验收，V3 软件验证已通过；首轮其它 R02—R06 和用户延期项不变。

- [x] **CP01 契约与状态归属**：已核对已有路径并记录 [契约](../companion-contract.md)、候选/机会/决策归属、来源与删除边界；三个实现包已分配。
- [x] **CP02 纯策略与可回放输入**：显式时间/版本/证据，固定抑制原因与预算；11 项纯策略测试通过，回放不调用实时模型/传感器。
- [x] **CP03 持久候选与权限**：唯一去重键、有效期、来源失效、用户政策、重启不重发及有界状态；9 项真实 PG 测试通过。本项为 shadow 持久化，live pending / admission 另归 CP05。
- [x] **CP04 Shadow 运行装配**：真实生命周期和受限入口，默认只记录决定，不产生 Response/媒体/动作/外部副作用；6 项真实 Runtime / HTTP / PG 测试通过，覆盖重启扫描、8 请求容量、取消排空、提交前超时后的原身份重试。
- [ ] **CP05 规范投递整合**：明确允许后走原 Context/Response/Delivery，交互占用与新语音优先，原权限/过期前后重验。
- [ ] **CP06 故障与回放验证**：相同输入可复现；删除/撤权、重启、重复、队列饱和、机会过期与语音竞争均有实际证据。
- [ ] **CP07 完整验收与提交**：适用真实 PG/软件双端/平台验证，记录 source manifest、边界、正常提交与下一步。

当前 **4/7**；CP05 未完成，CP06 的 shadow 部分已有证据但 live 故障矩阵未执行，CP07 的完整 R01 验收仍开放。三个 subagent 分别负责纯策略、持久来源、语音机会，Root 负责装配和组合验收。产品默认先 shadow；当前 API 和生产 actor 均拒绝 live 输出，不能因引用架构提案而擅自开启现有设备输出。

本阶段冻结 V3：619 个输入，manifest `8ea6d4f7cc63b82d22b8c584c8c0b426b11e04f952f64bd15b7776d352cc18a1`。完整 workspace 1005/0/5；5 个 ignored 已另行通过（真实 PTY 3、实际 Easy-Turn 2），合计 1010 项不同测试。定向 26/26、TTS HTTP 9/9 不重复累计。macOS 和 Linux ARM64 的 Server / Embedded no-default all-targets 检查通过；本轮 Linux 只编译，未重复 Linux 测试。详见 [验收摘要](../handoffs/2026-09-26/validation-summary.json)。

CP05 后续必须逐项完成：

- [ ] 专用 CompanionOutput Context，只允许固定意图的 ResponseCommit/OutputAdmission，保留原 Appraisal 来源与新 initiative request 身份。
- [ ] 候选在 TTL 内等待/重评独立机会；政策、同主题、预算与唯一 Response 在同一事务准入，未知结果不重发。
- [ ] 设备 capture watermark 与主动 Register/Start 原子验证，保护“本地已说话但上行帧仍在网络中”的竞争。
- [ ] 正向 paired reserve→Response→固定邀请 TTS→Delivery，以及来源/机会撤销、近端说话、暂停/恢复/取消和超时/丢 ACK 测试。
- [ ] 完成以上后才开放持久 `enabled`；中间构建维持 snapshot-only 和无媒体 shadow。
