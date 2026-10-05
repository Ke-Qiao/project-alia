# 动作 / 表达清理交接（2026-10-06）

负责人：`motion_protocol_1004`。本轮实现已交回 Root；源码保持冻结。总体命令、测试结果、冻结输入与最终提交由 Root 维护在[本轮记录](../../cleanup-2026-10-06.md)和[主 checklist](../../checklists/cleanup-2026-10-06.md)，本文不另行推断验证通过，也不重复历史成绩。

## 已完成的清理

- [x] 删除旧 `ExpressionService`、`expression.proto`，以及 `MotionRuntime` 中 `execute_expression_json`、`cancel_expression_json`、`capabilities_json` 拒绝壳。共享模块注册与 proto 构建由 Root 整合；动作执行保留 signed Body v1。
- [x] 删除无 canonical 来源的 affect observation / mutable reducer API、插件能力与请求响应变体，并更新示例 manifest。保留 canonical appraisal、原始 Context、planner/safety、校准、范围限制与唯一 writer。
- [x] 删除旧 `completion_acknowledged`、`executed_actions` 和 `Completed` / `TimedOut` 表达结果分支，以及无 Body 事实的完成推断。旧结果与插件能力表示不再通过宽松反序列化兼容。
- [x] 删除无原始 Response 的执行包装和合成 UUID 回退；缺少或为 nil 的 canonical Response ID 在接触 adapter 前返回本地 `Skipped`。
- [x] 删除旧表达 trace 重建及已无生产者的 affect / plan / STT correction 字段；表达 trace 只保留实际生产的 `tts_provider_adjustments`、`sentence_expression_llm`。API 过滤由 Root 同步，旧 STT 链由另一个 agent 删除。
- [x] 更新表达、Body 与 pipeline 测试 mock：通过当前身份契约报告事实，不恢复无身份输出默认回退。文本内存接纳保留 `written=None` 和 `Unknown`，不伪造原生写入证据。
- [x] 迁移 `server_runtime/delivery.rs` 两个关闭 fixture：`NativePlayback` 持有注册、身份与退休状态；普通关闭测试通过 `begin_voice_output` + `bind(response)` 绑定并持有原 VoiceOutput，dispatch 携带原 VoiceScope；panic wrapper 复用 inner 的注册与退休。

关闭 fixture 保留原 `spawn_blocking` gate、native JoinHandle 等待、超时后依赖存活、迟到结束后仍保留关闭失败历史，以及独立子进程中的 panic 隔离断言。修改目的是让测试进入原本要验证的 native 阻塞路径，没有降低等待或隔离要求。

主要实现位置：

- [Embedded Motion runtime](../../../../packages/alia-embedded/src/module/motion/runtime.rs)
- [表达结果类型](../../../../packages/alia-server/src/expression/types.rs)、[orchestrator 与测试](../../../../packages/alia-server/src/expression/orchestrator.rs)、[当前表达文档](../../../../packages/alia-server/docs/expression-runtime.md)
- [Server Body adapter](../../../../packages/alia-server/src/channels/embedded/body.rs) 与 [Body 测试](../../../../packages/alia-server/src/channels/embedded/body/tests.rs)
- [关闭 fixture](../../../../packages/alia-server/tests/server_runtime/delivery.rs)

## Body 证据与 Unknown 边界

`Accepted` 只证明原命令被接纳；`Submitted` 只证明达到所声明的软件提交边界。即使 `writer_quiescent=true`，也不能推出控制板已执行、执行器到位或机械安全停止。`position_confirmed_actions` 仍必须为 `None`，不增加虚构反馈。

表达结果核对原 Body 命令的 owner、plan、source、scope 与动作回执。只有有效 Body 观察同时具备结束记录和 `writer_quiescent=true`，才可退休原 writer；终态仍为 `Unknown` 时也必须保留这种事实区别。没有 Body 的本地 `Skipped`、或只有身份而没有 Body 证据的 `Unknown`，都不是退休证明。迟到 checkpoint 可以补充原命令事实，不能据此重新提交动作。

相应测试已加入或改写：缺失 canonical Response 不触达 adapter、有效终态 Unknown 的原 writer 退休、旧完成字段与伪造位置反馈拒绝。它们的实际运行结果以主 checklist 为准。

## 实际检查与资源状态

- 已只读核对相关旧入口调用图、当前 Body / 表达来源与完成判断，以及清理后的引用。
- 已对负责文件执行定向格式化和 `git diff --check`。最后一次 fixture 迁移执行的是 `rustfmt --edition 2024 --config skip_children=true packages/alia-server/tests/server_runtime/delivery.rs` 与该文件的 `git diff --check`，均 exit 0。
- 本 agent 未运行 Cargo、PostgreSQL、服务或硬件动作，也未自行核定本轮总体测试数字。没有新增常驻资源需要清理。
- 本文是源码冻结后的文档交接；未修改政策、绑定、凭证、默认 Shadow 或硬件参数。

## R03 后续 checklist

- [ ] **动作资产内容签名**：现有命令签名绑定文件名与参数，不等于签署本地资产 bytes / 版本 / 摘要；待明确实际资产发行需求后补充最小可信内容规则。
- [ ] **控制板协议**：明确固件 boot epoch、ACK、队列、断连 / 掉电 / 重启与看门狗行为。Embedded 进程 boot 和 host writer instance 不能替代控制板开机代次。
- [ ] **真实反馈与限制**：控制板、执行器型号、位置反馈、硬件限位、电流 / 热限制、负载及安全姿态仍未知；不能从示例校准推出现场安全。
- [ ] **有人监督的停止 / 恢复验证**：先明确安全停机策略，再分别验证 Server、Embedded、控制板重启与 ACK 丢失；软件取消与 writer 退休不能代替物理急停或实物静止证据。
- [ ] **真机与长期负载证据**：在明确硬件与授权后继续 R03 / R05；本轮 mock / gRPC 软件回归不关闭这些项目。

后续接手顺序：先查看主 checklist 的最终验证和提交，再按具体 R03 缺口补证据；无需重建本轮已删除的兼容入口。整体回归与发布状态由 Root 收口，本 agent 在本文交接后停止修改。
