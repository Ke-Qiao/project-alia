# 2026-09-26 恢复入口

用户已恢复额度并授权继续构建。当前分支 `codex/architecture-refactor`，原基线 `3530395`；DU11 已合入 `948b3c4`，**DU 11/11**。当前正在 R01，**CP01—CP04 完成：4/7**，CP05—CP07 剩余 3 项；R01—R06 整体仍 **0/6 完成**。这不是用户要求的暂停，也不是项目完成声明。

## 先读这些文件

1. [当前数字状态](resume-state-2026-09-26.json)、[Companion checklist](checklists/companion.md)。
2. [本轮四份 HANDOFF 和统一证据](handoffs/2026-09-26/README.md)：Root、Policy、Repository、Voice。
3. [CP05 总体设计](companion-live-design.md)、[存储事务契约](companion-live-storage-design.md)、[设备本地语音门契约](companion-live-device-design.md)。后两份从本轮只读草案归档，均未实施、未验收。

R01 已实现纯策略、规范来源 shadow 候选、持久政策/CAS、受权限保护的冻结回放、擦除与容量界限、真实后台扫描与关闭、REST 管理入口，以及只读 voice 机会基础。默认 shadow，API 和生产 actor 拒绝 live 输出；没有主动发声或新增外部动作。

## 准确验证数

V3 619 输入，manifest `8ea6d4f7cc63b82d22b8c584c8c0b426b11e04f952f64bd15b7776d352cc18a1`。

- macOS workspace：**1005 pass / 0 fail / 5 ignored**；5 个 ignored 随后分别在真实 PTY（3）及实际 Easy-Turn/gRPC（2）通过，共 **1010 unique tests**。
- Companion 定向 26、TTS HTTP 9 已包含于全量，不能再次累计。
- macOS 和 Linux ARM64 的 Server、Embedded no-default all-targets 检查均通过。Linux 本轮只编译，未执行测试。
- V1 的 5 项失败和 V2 的编译失败均保留；V3 source drift 为空。真实 PG 使用专用 32768 容器数据库，清理后仅原 3 个 schema，其他客户端 0。

本轮模型首次 adapter 推理约 11.17 秒，后续三个语义样本约 327—532 毫秒；服务未重启，不能称为冷启动或声学性能验收。真机 Pi、声卡/AEC/双讲、执行器位置及 24 小时仍未验收。

## 下一步 checklist

- [x] DU11 正式合入与 605 输入核对。
- [x] CP01—CP04 的 shadow 基础和统一 V3 软件验证。
- [ ] 在当前分支正常提交本基础包，并把提交号补入机器状态。
- [ ] 三名 subagent 实施 CP05：专用 Context/固定 Delivery 内容、live SQL 准入与预算、设备 capture gate 与原输出恢复；Root 装配 runner 并验证完整配对路径。
- [ ] 完成 CP06 live 故障矩阵：来源/政策失效、队列与期限、数据库/设备 ACK 未知、近端 speech 竞争及暂停/恢复。
- [ ] 完成 CP07 全部 R01 验收与正常提交后，才将 R01 勾选为 1/6。

用户已经确认临时避让与明确暂停应区分，不需要再问；Easy-Turn 当前仅在服务端，前端和通用 Embedded 适配继续 [延期台账](deferred.md)。后续 [R02—R06](checklists/next-round.md) 与真实硬件待确认项保持开放，不因软件测试通过自动勾选。

继续时先核对 Git 和本文件最新机器状态，按下一条未完成 checklist 执行，继续使用三个 subagent。不要重新合并旧 duplex overlay，不把当前验证数混入其他平台或旧轮次。
