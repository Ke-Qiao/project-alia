# Root 交接 — 2026-10-06 清理轮

当前任务范围、测试成绩与完成计数以 [清理记录](../../cleanup-2026-10-06.md) 和 [10 项 checklist](../../checklists/cleanup-2026-10-06.md) 为准。本轮完成后不自动推进 R02/R03 真实硬件。

## 已整合

- [x] 三个 subagent 分工：native mixer / 单一语音链 / Body 与 expression 清理；Root 处理共享契约、Server channel/Delivery、cue scope、配置与独占验证。
- [x] VoiceSession v2 / Playback v3 / Body v1 成为唯一语音/动作运行契约；删除旧 protobuf、RPC 注册及无身份通道输出。Media reservation 和 delivery 在类型上必须携带身份。
- [x] cue 保留 Ready 原 scope；未注册 Response 的 cue-only native owner 仍须真正退休。空 Response progress 不代表没有 native owner，不伪造 Response binding。
- [x] 明确通道实例 ID。删除 type alias/default routing，Embedded speech.channel_id 同时进入 Open 与 metadata，缺失或无效 ID 被拒绝。
- [x] 删除 TranscriptCorrection、旧 interrupt 参数、未用 ChannelResponse/broadcast、无 manifest 的 plugin storage fallback 与旧 schema 自动升级。
- [x] 保留 Shadow、当前策略值、临时避让/明确等待、原身份、撤销、真实进度和 Unknown。资产/设备管理 API 仍存在，不能据其成功推断媒体交付或动作到位。
- [x] 当前架构、协议、配置、脚本与示例同步；历史证据/迁移 SQL 保留。

## 验证原则

- Cargo / PostgreSQL 单窗口，由 Root 顺序执行；没有通过增大测试栈、放宽测试期限或将失败改为 ignored 取得通过。
- v1 与 v2 的失败日志和 manifest 保留。v1 四处 fixture/预期错误，v2 三处同一配对 fixture 缺少明确通道 metadata；原生保留与 Unknown 断言没有删除。
- 最终候选 v3 为 619 项冻结输入，manifest `a5264bb79689db17b84934389094fea9a26945c3a5be5e56e81e067f876f03a0`。每阶段验证输入无漂移；最终结果与提交核对由清理记录归档。
- 仅本轮独立数据库 `alia-cleanup-20261006-db`、端口 32769；用户 5433 未访问。Linux 使用只读源码、已有离线镜像和本轮独立缓存；10/4 缓存原件未修改。
- 不把不同平台、默认/无默认 features 的同一测试重复累加为 unique 通过数。真实 PTY 与真实模型依赖的 ignored 各自说明。

## 继续工作

1. 先查看主 checklist、最终 source manifest、Git 状态与资源关闭记录；不要从历史 checkpoint 推断当前源码。
2. 本轮清理之外的待办按 [清理记录后续项](../../cleanup-2026-10-06.md#后续仍开放) 和原 R01—R06 checklist 执行，六轮完成数不因软件清理改变。
3. AEC、Pi 实际录放音、执行器动作、绑定/凭证修改、更多平台及前端继续按明确范围开展；当前软件测试不能关闭这些项。
