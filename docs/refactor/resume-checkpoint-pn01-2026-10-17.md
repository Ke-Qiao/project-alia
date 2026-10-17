# 恢复检查点：PN01 Server 插件权限

2026-10-17；分支`codex/architecture-refactor`，基线`15604e3`。**PN01完成，本轮6/6；插件实施1/4。** [Checklist](checklists/pn01-2026-10-17.md)、[四份HANDOFF](handoffs/2026-10-17-pn01/README.md)、[原始证据与受测文件摘要](handoffs/2026-10-17-pn01/evidence-index.json)。

## 已交付

- 原插件list／usage／export／purge为管理员管理入口，保持原响应结构。敏感读取在取数等待结束后复验当前active/admin。
- 本人入口`/api/v1/plugins/{name}/storage/me` GET usage／DELETE，以及`/me/export` GET，固定认证用户UUID与`scope=user`；拒绝自报owner／scope，不涉及channel／conversation成员推断。DELETE只接受可选collection。
- 删除在同一事务完成DELETE的等待后，以`FOR SHARE`复验当前账号并持锁至commit；失权rollback，不出现“返回403却已删除”。本人路径只要求active，管理路径还要求admin。
- 生产只改插件API和共享存储查询；复用CAS、TTL、retention及原Execution批准，无新schema、依赖、隔离后端或全局鉴权重构。

## 必要验证

基线真实RED **0通过／2失败**：普通用户export返回200且包含他人值；purge返回200，将9条夹具记录删至1条（8条目标插件数据被删除）。修复后同两项403、无泄露且9→9。

最终macOS默认features定向HTTP **7/0/0**（85个无关用例按本轮范围filtered，3.32秒）；既有storage **4/0/0**（无filtered，0.07秒），共**11个不同通过用例**。三项真实PG锁等待均有进入等待见证，降权／失活后403且全数据快照不变。明确传入任务PG，真实存储测试没有因缺环境变量而跳过。原期限、fixture并发及libtest默认并发未改。

未重跑workspace、Linux／release和不相关模型／硬件矩阵；这是当前改动的定向软件验证，不追认旧release包含本次修复。测试后临时PG为0 fixture schema／0其他连接，任务容器已自动删除。没有触碰共享服务、Pi、真实录放音或执行器。

## 下一步：优先实际接入R02

用户要求减少无关测试／整备，必要软件准备结束后尽快接硬件。**就首个R1 USB接入而言，不再安排新的泛化软件准备轮次。** 既有单consumer、mixer、有界音频记录、离线重放和取消／清理证据继续复用；PN02—PN04、通用MCP／SDK／市场不是无插件音频试验的前置条件，保留为后续扩展目标。

下一轮先由用户把R1通过USB接入已确认的Pi，再核对USB／音频设备、原生格式与节点权限。实际采集前落实目标Pi可运行产物、固定设备／无插件／motion关闭配置、停止清理和录音保留窗口；播放及硬件AEC参考连接需补齐扬声器／功放和厂商电气资料。真实自然对话链单独落实精确模型和素材，不阻挡前面的接入。见[硬件资料及分层门槛](../operations/hardware-deployment-readiness.md)。本轮没有执行现场动作。

默认shadow、probe=false、当前政策和执行批准不变；R03继续暂停。历史V01根因仍open，无新证据不重复求绿；R01—R06仍2/6，不把定向软件通过计为硬件／整机通过。插件剩余实现为PN02包与生命周期、PN03表达ABI、PN04实际隔离及未经审核示例，均未取消。
