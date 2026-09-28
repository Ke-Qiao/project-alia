# 2026-09-28 继续构建入口

分支 `codex/architecture-refactor`。本次已正常本地提交：R01源码 **`a79cb1f`**、R01验收记录 **`75bf97c`**、R04源码 **`8f5bb83`**。没有推送或创建PR；此前主线合入和旧工作树不需要重复处理。

**DU 11/11；R01 CP01—CP07 7/7；R04四组修复4/4、MG05统一验收完成；后续轮次R01—R06为2/6（R01、R04）。** 六项规模不同，这不是整个项目完成百分比。

## 已完成 checklist

- [x] Root与三个subagent继续协作，按独立模块分工；始终只有一个Cargo/PG验证窗口。
- [x] R01受约束主动邀请走规范Context/Response/Delivery，保持原输出owner、两类暂停、来源撤销与预算身份；默认政策仍为shadow，没有更改用户部署政策。
- [x] R04完整HTTP配置模板、实际role/profile及分片校验、原子保存；未认证setup仅loopback，成功响应排空并关闭stalled连接。
- [x] Companion四个REST操作的真实DTO、Bearer、CAS、未知完成和擦除结果进入OpenAPI及终端指南。
- [x] OpenAI显式在线probe每次最多5秒，诊断不包含密钥/URL，等待期间不持有配置锁。
- [x] 设备9个handler统一使用当前owner/admin权限；多渠道要求明确选择，status走实际RPC，未知遥测保留null。
- [x] R04冻结633输入V4：workspace **1053通过/0失败/5忽略**（319.39秒），真实PTY **3/3**、实际CPU Easy-Turn **2/2**，共 **1058项不同macOS测试**。
- [x] R04 macOS/Linux ARM64两端no-default all-targets及fmt通过；Linux仅编译，未执行Linux运行时或PG测试。
- [x] 源码提交`8f5bb83`的633个输入与V4逐项相同，无额外/缺失输入；四份HANDOFF及结果已归档。

本轮下一次应读取 **[R04四份HANDOFF](handoffs/2026-09-28-r04/README.md)**（runtime、api、setup、device-health）、[管理checklist](checklists/management.md)、[六轮checklist](checklists/next-round.md)及实际Git状态。机器状态在 [JSON](resume-state-2026-09-28.json)，冻结身份与逐项结果在 [验收摘要](handoffs/2026-09-28-r04/validation-summary.json)。R01独立验收为630输入、1036/0/5加专用环境5项=1041 unique，不能再加到R04数量。

## 下次按依赖继续

- [ ] **R02 AEC与双讲**：先取得H02/H03的Pi系统、接入方式、供电散热、麦克风/声卡/扬声器型号及布局，再接实际AEC、时钟映射与双讲测量。
- [ ] **R03执行器**：取得H04的控制板、反馈、机构限位及安全状态，再接签名动作资产并验证停止/失联/重启。
- [ ] **R05整机验收**：补齐H01—H05部署机器、模型版本/地址、硬件和测试接入，执行干净安装/配对/恢复及至少24小时混合负载。
- [ ] **R06其他平台/未来部署**：指定目标OS/CPU/runner后补完整运行矩阵；未来设备模型、同机多进程/单进程与动态迁移分别验收，当前不测试Pi5承载Easy-Turn。
- [ ] **V01验证异常跟踪**：V3中3个旧Companion配对场景在25秒内超时；随后同一原workspace binary默认9项并行全过，V4完整回归也通过。V4只加固定阶段和最多1秒无内容/身份状态诊断，首次失败根因仍未确定。若复现，保留阶段/ScanProgress/候选退休原因再定位，不延长期限、不串行化或忽略测试。

硬件信息已询问但尚未收到，不能把信息待补充写成用户取消首轮全双工/AEC或真机验收。前端Web/iOS/Android、Embedded通用适配、邀请历史的后续只读使用及组件接入门槛见 [台账](deferred.md)。C01/C02未因提案列名引入新依赖。

## 资源与验证边界

任务PG `alia-refactor-test-db` 在 **32768** 保留运行，最终非public schema和其他连接均为0；用户5433数据库未操作。实际CPU Easy-Turn在 **8091** 保留运行；Linux V4编译容器已停止，V1仅创建未运行、V2只准备后停止、V3没有Linux快照，历史缓存/快照和旧工作树均保留。三个subagent本包工作均已结束，没有遗留Cargo/PG测试进程。

Provider的5秒是单次probe期限，顺序聚合可能累加。设备公开GetBindingStatus仅说明本次RPC可达，不证明可信body pin、实际配对或硬件健康。真实模型测试数值不是声学端到端或冷启动指标；本轮没有Pi/AEC/执行器/24小时验收。
