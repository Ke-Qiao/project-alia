# Root / Runtime HANDOFF · 2026-09-26

当前分支 `codex/architecture-refactor`，DU11 合并 `948b3c4`。本轮已验证 R01 shadow 基础，**CP 4/7、后续轮次 0/6**。当前没有用户额度暂停请求；本文件持续保存可恢复进度。

## 已完成行为

- `CompanionService` 只持有 SQL repository 与 VoiceInitiativeHandle；shadow 路径没有 LLM/TTS/Delivery/执行器能力。每 5 秒扫描最多 32 条原来源，稳定 operation 去重；缺少机会由 SQL 决策时间表示 Unavailable。扫描诊断只有 opaque 计数与固定 stage/class。
- 公开请求最多 8 个。admission mutex 将 closed/capacity/TaskTracker token 原子排序；close 先阻止新请求，stop 保留原 worker JoinHandle，并等待全部请求 token 排空。取消 stop 等待者不会丢失收尾责任；数据库后于 Producers 释放。
- 四个 REST 入口仅使用 active account 的原身份；请求不接受外部 owner、roots、机会、预算或 scope。政策 CAS，operation/worklist 固定，读取仍复核当前来源权限。enabled 当前明确拒绝。
- ModuleRegistry / AppState / ServerBuilder / RuntimeCleanup 接通真实生命周期；VoiceSession 提供 snapshot-only 机会。生产 `initiative_output_enabled=false`，不能由服务装配打开。
- 6 项 Runtime 用例覆盖 owner/CAS/restart、HTTP 回放和擦除、真实扫描重启、8 请求容量、取消排空、提交前 SQL 阻塞超时后的原 operation 重试。扫描重启以第二真实来源的持久回执作为进度证据。

## 验证与保留失败

统一结果见 [validation-summary.json](validation-summary.json)。冻结 V3 619 输入，source drift 为空。完整 macOS workspace 1005/0/5；真实 PTY 3 与实际模型 2 另行通过，共 1010 unique。两平台的 Server/Embedded no-default all-targets 编译通过。fmt 与 diff 检查通过。

V1 996/5/5：四项旧 TTS HTTP fixture 在关闭阶段超时，修复为消费请求体、仅 teardown 时结束刻意不结束的响应 body，以及保留 JoinHandle；未放宽 3 秒期限或修改生产 TTS。另一项 scanner 10 秒期限失败，在单次和双进程诊断中未复现，不能声称已确定根因。补充了固定诊断、缺少机会时的同数据库时钟输入和重启覆盖；V3 定向及全量均通过，原日志保留。

V2 编译失败来自 Root 的 TaskTrackerToken import 路径错误，V3 仅纠正为 `task_tracker::TaskTrackerToken`。没有把代码审阅当成编译证据。

实际模型服务未重启；首次 adapter 调用推理约 11.17 秒，随后 gRPC Complete/Backchannel/Wait 约 532/327/415 毫秒。测试按原期限通过，但不能把它们当冷启动或端到端声学性能通过。R05 仍需测量和约束启动/闲置后的首请求性能。

测试 DB 保留原三个 pipeline_delivery schema，其他客户端 0；没有操作用户 5433 数据库。两个任务 Linux 容器已停止，模型服务、任务 PG、旧工作树与缓存保留。

## 后续有界分工

- Policy agent：专用 CompanionInvitation Context、完整来源证据校验及 Delivery 固定内容限制；普通 Generation 不得伪装邀请。
- Repository agent：独立 live pending、跨 shadow/live 来源去重、不可退款预算、唯一原子 admission+Context+Response、Fresh/Existing 与真实事务竞争测试。
- Voice agent：不可变 initiative capture binding、能力协商、设备锁内 Register/Start、本地可恢复 Hold、迟到 Resume 与更新 speech 竞争，保持原 producer/native owner。
- Root：实际 runner 和服务生命周期、API live readiness、三个接口整合、规范 TTS/Delivery 路径与最终软件配对测试；未通过前维持 live 关闭。

提交未知后的证明已选定：READ COMMITTED，所有准入事务持有同一 pending 行 FOR UPDATE 至 commit。重试先取同一锁，再发新 SELECT；存在回执只返回 opaque Existing，无 permit；确认 absence 且原 pending/intent 有效才可按原身份重试。原意图跨未知结果保持，旧 instance 重启退休，不重新 snapshot 换机会。不得把普通提交前 timeout 用例称为 commit ACK 丢失。

实施顺序与未完成项目以 [Companion checklist](../../checklists/companion.md) 和三份 CP05 设计为准。前端、通用 Embedded、Pi 上 Easy-Turn、同机部署实测和真实硬件/24 小时验收仍按原台账处理。
