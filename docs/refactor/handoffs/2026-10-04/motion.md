# R03 软件前置 / 设备 gRPC 信任边界 — 2026-10-04

状态：已完成源码核对、两个限定 mock/gRPC 回归及动作资产文档修正；Root 已完成冻结 V1 的 macOS workspace 实跑，Body 8 项（含新增 2 项）、Motion runtime/servo/plan 44 项、Server Body 16 项和 PG body ledger 7 项均通过，详见末节原始证据。R03 整轮仍开放，未取得执行器现场、控制板重启或物理反馈证据。源码已提交 `010f7aa`，633 输入匹配；Linux 独立矩阵也已完成，见 [Linux HANDOFF](linux.md)，两平台分别计数。

开始核对的 Git HEAD 是 `8c1742287b75720a2530711ce78c447679369884`，当时工作区干净；R04 验证源码为 `8f5bb83f4a313723428abf854c1e663b159c6c45`。本包只编辑：

- `packages/alia-embedded/src/grpc/body_service/tests.rs`：在原 fixture 增加真实 DeviceBinding service 和两项协议回归。
- `packages/alia-embedded/docs/motion-assets.md`：修正严格三值格式和已禁用的 legacy 执行入口，说明签名边界。
- 本文。

没有修改生产协议、准入、默认 shadow、政策、预算、暂停或来源撤销；没有新增依赖、修改设备绑定配置、连接真实设备或发送真实动作。测试中的绑定变更只针对独立临时 SQLite 和 loopback mock 服务。默认的硬件/校准参数没有变动。

## 已实现能力、既有测试与缺口

| 目标能力 | 当前源码 | 既有测试 / 本次补充 | 证据边界 |
| --- | --- | --- | --- |
| 请求/回执签名、目标身份和时效 | `grpc/body_service.rs`、`binding/state.rs::with_verified_body`、`crates/transport` 的域分离签名；严格 JSON 原字节签名，30 秒 request 时效，真实 Bound key 和 bearer scope | Embedded `forged_target_signature_timestamp_and_binding_retirement_prevent_writes`；Server `signed_exchange_rejects_wrong_nonce_device_version_and_pinned_key` | 已实现；本轮先读源码。测试不等于真实设备 pin 已核对。 |
| device pin 不被公开状态替换 | `channels/embedded/channel.rs::{body_device_pin,remember_bound_device}`；`body.rs::SignedBodyClient` 核验配置 key、nonce、device、version | Server `unsigned_binding_status_cannot_populate_a_body_pin_or_replace_its_configured_key`、`missing_trusted_pin_rejects_raw_body_submission_before_ledger_or_control` | 前者是直接 channel 状态测试，后者及 signed exchange 使用脚本设备的真实 loopback gRPC；不能写成全真实 Embedded 配对。 |
| 公开可达不等于绑定授权 | `DeviceBindingService::get_binding_status` 公开；Body Exchange 再在 binding 读锁中核验当前 Bound/server key 与具体签名 | 本包新增 `public_status_reachability_and_reconnect_cannot_authorize_unbound_or_foreign_body_control` | V1 macOS 实跑通过；同时使用真实 DeviceBinding 与真实 EmbeddedBodyService，Motion 是 mock。 |
| boot、control generation、sequence 和重复命令 | `alia-core::body::{BodyControlLease,BodyCommandIdentity}`；`MotionRuntime::{open_control,submit,reject,observe}` | Embedded `signed_body_submission_is_observed_without_position_or_replay`；Motion `duplicate_identity_reuses_original_receipt_and_changed_intent_is_rejected`、`queue_and_receipt_cache_are_bounded_and_retired_sequences_cannot_replay` | 身份是 boot/server instance/generation/sequence 元组；`source_command_id` 是原业务关联，不是另一个可换皮重放 ID。缓存淘汰不清序号水位。 |
| 替换 Embedded boot 后旧状态不能重放 | Builder 共享新 boot，Motion 检查原 lease/boot，Body Open 检查设备当前 boot | 本包新增 `replacement_runtime_boot_rejects_original_control_and_command_without_replaying` | 顺序关闭/创建两个 mock runtime，保持相同 fixture keys、换新 boot；不宣称真实进程 kill、SQLite 故障恢复或控制板掉电通过。 |
| ACK 丢失、观察断开与精确取消 | Server `EmbeddedBodyRuntime` 保留 owner，最多一次 Submit；随后 Observe/Cancel 原命令，迟到证据独立保存 | Server `lost_submit_ack_is_unknown_and_exact_cancel_never_replays`、`first_cancel_loss_retries_original_command_without_repeating_submission`、`five_second_command_keeps_owned_observation_after_initial_waiter_is_dropped` | 真实 gRPC + 脚本设备；丢的是协议回执，不是 MCU 硬件 ACK。 |
| Server 重启恢复不重新获得 dispatch | `BodyDeliveryRepository::{lookup_origin,claim,recover_expired}`；先查原 Response/phase 再开控制，事务先保存原身份 | PG `concurrent_claims_grant_one_physical_permission_and_preserve_original_identity`；Server `original_response_record_is_restored_without_opening_control_or_resubmitting` | 前者新 server/boot/sequence 仍返回原账本；后者用预置 store 恢复原事实。均不是完整 OS 重启/升级实验。 |
| 绑定/voice scope 撤销与旧取消隔离 | Binding → Playback → Motion 的同步准入顺序；本地 fence/revoke 退休旧 permit | Embedded `original_voice_scope_and_exact_old_cancel_cannot_reach_replacement`、`token_expiry_cancels_native_plan_without_observer_renewing_it`；Motion `replacing_control_cannot_overlap_native_output_or_let_old_close_revoke_new_owner` | 现有 HandoffPending body 用例通过 `update_for_test` 改状态；本包另补真实 Unbind/Bind RPC，但未宣称完整 handoff RPC 验收。 |
| 有限队列、唯一 native writer、停不下来不假成功 | `module/motion/{runtime,writer,servo}.rs`：准备 4、队列 8、回执 64；writer/prepare join 保留 | Motion `cancellation_deadline_keeps_native_owner_and_accepts_late_quiescent_checkpoint`、`shutdown_waiter_drop_keeps_join_and_reset_runs_only_after_entered_write_returns`、`native_panic_is_unknown_and_no_replacement_writer_is_spawned` | 软件 mock 阻塞/失败证据；不推断电机物理静止。 |
| 串口/I²C 部分提交事实 | `ServoWrite` 分完整通道/不确定通道/host accepted bytes，错误后不重试动作帧 | Servo `serial_partial_failure_has_uncertain_channels_and_does_not_retry`、`serial_full_host_write_remains_a_fact_after_flush_failure`、`i2c_short_transaction_is_never_retried_as_a_suffix_register_write`；Motion `native_partial_failure_preserves_complete_and_uncertain_channels_and_seals_writer` | 纯 mock Write/backend；没有 MCU ACK、编码器或现场限位证据。 |
| 动作/能力/本地资产校验 | `BodyCommand::validate`、`plan::{prepare,read_animation,expand}`、`NeckMapper`；动作最多 32、名义总时长 5 秒、native 展开有界 | plan 的 5 项格式/总量/符号链接/节奏/坐标测试；Motion `invalid_native_configuration_and_calibration_are_rejected_before_backend_construction` | 文件名/参数经命令签名；本地动作文件内容未签名。历史校准不等于选定机构已校准。 |
| 主动固定邀请不得旁路驱动身体 | `BodyDeliveryRepository::{claim,authorize}` 拒绝 `source_notice`，仍要求原 Response source/owner/channel/scope | PG `fixed_source_notice_cannot_authorize_a_body_command`、`revocation_stops_admission_but_keeps_actual_submitted_facts` | 现有规则保留。本轮没有把动作接入无人值守主动邀请。 |

上述源码枚举为 Motion runtime 29、servo 10、plan 5，原 Embedded Body RPC 6、现为 8，Server Body 16，PG body ledger 7。末节已逐组核对 V1 日志中的通过结果；这些是不同目标/层级的子集，不能再加到 workspace 总数，也不能与旧 R01/R04 数重复相加。Motion 的另一个 `neck_mapper` 测试也在 Embedded lib 通过，但不包含在此处 runtime/servo/plan 的 44 项小计中。

## online 与输出准入的实际调用链

1. REST `api/devices.rs` 先复核当前 owner/admin，随后限时执行 `binding_status`，把本次可达且非空的 ID 映射为 `online=true`。CPU/内存/温度仍为 `null`。这个响应只供诊断，没有成为输出控制 gate 的输入。
2. Body channel 要求配置 ID/key 与状态相符，且持久 store、已启动 owner 存在；`capabilities()` 还要经过签名 `Inspect`。即使公开状态谎称已绑定或匹配 key，真实 `Inspect/Open/Submit` 仍必须通过设备当前绑定 key 和具体命令签名。
3. Embedded Submit 在有限文件准备前认证，准备后再次在同一个 binding 读锁内准入；Motion 核对 boot/lease/sequence，Voice 命令另受本地 Playback 作用域 gate 约束。未经接纳不能通过公开状态自行建立 BodyCommand。
4. Playback 的 Register/Control/Push 另经 `with_verified_grpc_auth` 与当前 VoiceScope、output registration；Push 等待/运行中持续核验原绑定 token，绑定撤销和本地 fence 关闭原输出。
5. `execute_expression_json` 的无作用域 legacy 路径已明确拒绝，不是绕过签名 Body 的入口。这里确认的是已追踪的音频/身体准入；不把公开诊断升级成整个设备所有接口的独立认证审计。

Body pin 和语音绑定是现有不同的契约边界；本轮没有把 body pin 的验收结论扩大为新的媒体端到端加密/设备认证保证。

## 新增两项回归的精确范围

`grpc::body_service::tests::public_status_reachability_and_reconnect_cannot_authorize_unbound_or_foreign_body_control`：

- 复用原 mock Motion 与临时 SQLite，同一个 loopback server 注册真实 BodyControl 和 DeviceBinding。
- 通过签名 bearer 的真实 Unbind RPC 解除 fixture 绑定；公开无 bearer 的 status 仍返回非空设备 ID，`is_bound=false`。
- 用另一固定测试 key 经真实 Bind RPC 绑定其他 Server；公开 status 仍可达且 `is_bound=true`，但 owner 为其他 Server。
- 每次变化后建立新 TCP/gRPC 连接；原 Server key 的 Inspect/Open/Submit 都必须返回 FailedPrecondition，旧 command 没有 observation、control lease 仍为空。
- 未模拟 REST；R04 REST owner/admin/mapping 证据保持独立。

`grpc::body_service::tests::replacement_runtime_boot_rejects_original_control_and_command_without_replaying`：

- 第一 mock runtime 实际接纳命令并精确取消，取得 signed quiescent 事实；完成 gRPC、Playback、Motion 的原 cleanup 后才创建第二实例。
- 第二实例使用相同固定 fixture device/server keys，但新 boot。旧 Open/Submit/Observe/Cancel/Close 全失败且 control state 不变化，旧记录不能冒称 Rejected 或 Completed。
- 新 boot 下新 lease/命令可以接纳，位置反馈仍为 None，避免“全部禁用”造成假通过。
- 使用两个临时独立 store，验证 boot 契约，不替代带持久状态的进程重启/控制板重启。两个实例均 `hardware_enabled=false`，startup/stop 不接触真实串口/I²C。

两项保留原 fixture 的 15 秒外层期限和 cleanup，没有新增 sleep、强制串行或 production 截止时间调整。

## 接纳、提交与物理反馈

| 可报告事实 | 当前意义 | 不可由此推出 |
| --- | --- | --- |
| Accepted | 原身份已进入有限队列，具备命令所有权 | 驱动已写、控制板已接收、机构已经移动 |
| Started / 部分提交 | writer 已开始，保留完整通道与不确定通道边界 | 整帧到达或物理状态已知 |
| Submitted 且 writer_quiescent | 所有计划帧已在声明的 `SerialHostWrite` / `I2cRegisterWrite` / `Simulated` 边界提交，软件 writer 不再提交该命令 | MCU 已执行、角度到位、重力负载安全、已听到或已看到 |
| Cancelled 且 writer_quiescent | 原软件 writer 不再提交，保留取消前的真实前缀 | DAC/机构缓冲清空、机械已停、断力或保持哪种更安全 |
| Unknown | 缺乏明确结束或停止证明，原前缀与迟到 checkpoint 仍可查 | 没有发生、可以重新发、可以创建替代 writer |

v1 `position_confirmed_actions` 必须是 `None`，`BodyProgress::validate` 拒绝伪造位置确认；Server `ExpressionExecutionResult::from_body` 的 `completion_acknowledged` 为 false。现有串口 File write/flush 不读 MCU ACK；PCA9685 寄存器写成功也没有舵机位置传感器读回。

## 明确仍开放的 R03 项

- [ ] **签名动作资产**：现在只签文件名和动作参数，没有签本地文件 bytes/版本/内容摘要；未建立受信任的资产发行、替换或回滚流程。本轮记录准确边界，不新增供应链平台。应在选定真实动作资产与发布需求后，确定最小内容绑定/可信来源规则。
- [ ] **控制板断连/重启**：没有选定固件的 boot epoch、ACK/队列、看门狗、禁用/保持协议；软件 `device_boot_id` 是 Embedded 进程代次，`BodyBackend.instance_id` 是主机 writer 实例，不是控制板开机代次。
- [ ] **机构反馈和限制**：控制板/固件、舵机型号、位置/速度反馈、硬件限位、电流/热限制、重力负载和安全姿态未知；不得套用示例 PWM/角度范围开展动作。
- [ ] **实际停止/恢复**：需明确掉线、Server/Embedded/MCU 分别重启时控制器行为，再在有人监督的低风险工况测量。现在的软件取消不替代物理急停。
- [ ] **真机与 R05**：USB/UART → Arduino、PCA9685/mock 只是已有路线。未验证当前 Pi 接线/固件，未发送真实动作，不关闭整轮 R03，也不把 mock 通过用于无人值守主动身体输出。

## 验证记录与后续步骤

已执行：`rustfmt --edition 2024 packages/alia-embedded/src/grpc/body_service/tests.rs`，exit 0；指定文件 `git diff --check`，exit 0。Agent 没有持有 Cargo/PG 窗口，也没有创建服务/容器/后台进程。

Root 本轮实际执行的命令是：

```sh
cargo test --workspace --locked --offline
```

Agent 只读核对了 [原始日志](../../../../run/validation-20261004/workspace-v1.log)、[command/result](../../../../run/validation-20261004/workspace-v1-result.json)、[逐目标计数](../../../../run/validation-20261004/workspace-v1-counts.json) 和 [冻结输入](../../../../run/validation-20261004/final-source-manifest-v1.json)。结果为 **1062 passed / 0 failed / 5 ignored**，exit **0**，541.76 秒，633 个冻结输入 SHA-256 **`a1c7dc3d6787cf2b86144b60c17b5aaaaad65025d1695a1326194c826c8ba0d0`**，前后 `source_drift=[]`。运行时间为 2026-10-04 10:25:18—10:34:20 UTC（18:25:18—18:34:20 Asia/Taipei）；Cargo jobs=2，`RUST_TEST_THREADS` 和 `RUST_MIN_STACK` 均未设置。命令使用 workspace 默认 feature 解析，没有 `--no-default-features`、`--all-features` 或额外 `--features`，不能冒称所有 feature 组合均通过。

| 实际 Cargo 目标 / 二进制路径 | 目标结果 passed/failed/ignored | 本文逐条核对的子集 |
| --- | --- | --- |
| Embedded lib，`target/debug/deps/alia_embedded-e7e0f3f5ffe0d9c6` | 167/0/0，26.16 秒 | `grpc::body_service::tests::` 8 项全部 `ok`；`module::motion::runtime::tests::` 29、`servo::tests::` 10、`plan::tests::` 5 全部 `ok`。 |
| Server lib，`target/debug/deps/alia_server-928ea6d47fbd3e06` | 313/0/3，17.35 秒 | `channels::embedded::body::tests::` 16 项和 `channels::embedded::channel::body_pin_tests::` 2 项全部 `ok`。 |
| `tests/body_delivery.rs`，`target/debug/deps/body_delivery-d10d49daf0542a0e` | 7/0/0，1.45 秒 | 真实任务 PostgreSQL 的 7 个 ledger 测试。 |
| `tests/server_runtime.rs`，`target/debug/deps/server_runtime-1dc044b93c32fdc1` | 53/0/0，72.80 秒 | 包括原 R04 REST 设备权限/status 映射；仍不把其可控 channel fixture 说成真实硬件。 |

两个新增测试的完整名字在原日志分别第 51、80 行，均为 `ok`：

- `grpc::body_service::tests::public_status_reachability_and_reconnect_cannot_authorize_unbound_or_foreign_body_control`
- `grpc::body_service::tests::replacement_runtime_boot_rejects_original_control_and_command_without_replaying`

只读回填时核对上述实际产物的 SHA-256（路径来自 Cargo 原始 `Running` 记录，不新增测试运行）：

| 产物 | SHA-256 |
| --- | --- |
| Embedded lib test | `a64fb1808d13925f7e819a0df55eac792a059df20f9acbd7c52b68909f4d7237` |
| Server lib test | `ec1953ec4b8cc60721305fa3f9c91bb0e1fb3b00c1070db0d13a126db8ca187f` |
| body_delivery test | `01f424007fe6966ef676fff720a147e7d5e772c04d13df19084546e09561d124` |
| server_runtime test | `1aedd1db8e5188e4463b31e6246a464fd64f2c46dc4dfea051be9874cc589654` |

原日志 SHA-256 为 `beb8172f600b72fdc923552809e19683f20c277be06c3b8eaef853052915ab47`。逐目标计数排除了 1 个嵌套 child summary（1 passed），5 ignored 未记为通过；不把本轮通过数加到 R01/R04 旧数字。旧建议中的 `cargo test -p alia-embedded --lib grpc::body_service::tests --no-default-features --locked --offline` 没有在本轮被单独执行，本节没有将它列作已通过命令。

Root 请求的 setup 限定只读审查另发现并修正了 CLI 竞争输家错误和外部取消后 drain 中继续写盘的窗口。新 `bootstrap::setup::tests::cancellation_rejects_an_accepted_save_whose_body_finishes_during_drain` 在同一 V1 原日志为 `ok`；测试以 HTTP 100 Continue 确认请求进入 body 等待，再取消并补完 body。这属于 Root 的 setup 交付，不额外计入 Body 子集。

后续仅在修改或失败需要时复用现有定向过滤器，不为了增大数字再重复执行。PG ledger 运行必须明确指向任务隔离 PostgreSQL，禁止用户 5433。Linux 默认 feature、实际二进制与 578/0/3 的独立矩阵见 [Linux HANDOFF](linux.md)，不与 macOS 累加。

- [x] 源码/测试先核对，区分真实 endpoint、脚本设备、mock driver、PG 和实物。
- [x] 两个具体协议证据缺口已补测试，无 production 修改。
- [x] 旧资产文档与实际 parser/入口一致，未假称文件内容已签名。
- [x] 源码交回 Root 冻结，V1 workspace 前后输入无 drift。
- [x] 从原始日志/JSON 回填实际 macOS 命令、默认 feature 语义、exit、目标数量和两项完整测试名；核对产物身份，没有将建议定向命令写成实跑。
- [x] 正常源码提交 `010f7aa5ae2c39a5c90c7f22979391a6b1bbbf23`；[committed-source 核对](committed-source-verification.json) 的 633 项均匹配。
- [ ] 根据真实硬件资料继续 R03 未关闭项；本轮没有强行选择停止策略。

停止后的资源状态：本 agent 未运行新增服务、Cargo 或数据库会话，无需关闭额外资源；Body fixture 的 gRPC、Playback、Motion 关闭/join 已包含在本次通过测试中。三个 agent 已交回验证窗口；Root 核实本轮 PG/Linux 容器均已停止，任务 schema/其它连接为 0，见 [资源状态](resource-cleanup.json)。
