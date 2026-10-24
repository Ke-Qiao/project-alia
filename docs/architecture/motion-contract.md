# Motion：短时动作命令与回执

Motion 是 `embodiment` 下的有界短时动作域，承接表达动作及现有设备唯一 native writer。它支持点头、鞠躬、注视目标和经双端批准的动画；不表示导航、位置反馈、任意机器人命令或长时 Operation 已完成。

## 模块和接口

- `crates/alia-core/src/embodiment/motion.rs` 定义 `MotionCommand`、`MotionKind`、作用域、控制 lease 与 `MotionProgress`。
- `crates/proto/proto/motion.proto` 定义 `alia.motion.v1.MotionControl/Exchange`，消息为 `SignedMotionPayload`。
- `packages/alia-embedded/src/grpc/motion_service.rs` 验证并接纳签名命令；`module/motion` 持有唯一设备 writer。
- `packages/alia-server/src/channels/embedded/motion.rs` 管理原命令、签名调用、观察与精确取消；`modules/embodiment/motion_delivery` 保存持久事实。
- 查询接口为 `GET /api/v1/responses/{id}/motion-deliveries` 和 `GET /api/v1/motion-deliveries/{id}`，继续按当前用户和原 Response 归属授权。

外层 RPC namespace 为 v1，严格 JSON 契约为 version 2。请求和设备响应分别以 `alia-motion-v2-request` / `alia-motion-v2-reply` 域分离 Ed25519 签名覆盖原始字节；响应回显原 nonce、设备身份与版本。消息上限仍为 64 KiB。

## 身份、准入和安全边界

设备身份固定到 Server channel 的 `expected_device_id` 与 `expected_device_public_key`。两项必须来自可信设备本地信息；发现服务和未签名状态不能更新 pin。绑定、当前用户、规范 Response、持久账本、原 voice/nonvoice scope 和设备本地安全校验仍共同决定准入。

命令身份由 device boot、server instance、control generation 和单调 sequence 构成。旧 generation 不能重开，相同身份和内容只能读取原事实，变造内容或复用退休序号被拒绝。设备只缓存有限回执，但序号水位不随淘汰丢失。

动作最多 32 个，名义动作合计最多 5 秒，展开轨迹最多 3,600 帧。进入 native 输出前校验有限数值、映射、校准、限位、速度、资产内容与完整轨迹。`motion.calibration_confirmed` 默认 false；未确认的模拟设备不输出启动帧，未确认的硬件在打开 backend 前失败。确认后仅在完整提交启动中心目标后建立上次软件目标；这不是实际姿态或平滑归中的证据。

动画清单默认空，最多 64 项，固定单叶 `.anim` 文件名、非零 version、SHA-256 和 1—65,536 字节长度。Server 与 Embedded 使用各自受信配置中的同一内容身份。一次有界读取完成摘要验证与解析，准备后保留内存轨迹。模型选择文件和播放参数不能新增批准，也不能改变已记录命令的资产身份。详见 [动作资产与本地校准](../../packages/alia-embedded/docs/motion-assets.md)。

唯一 native writer 统一启动、表达、人脸跟随与关闭。准备并发上限 4，动作队列 8，回执缓存 64；人脸目标只保留最新项。绑定撤销、lease 到期、voice fence 和本地关闭均关闭旧准入。取消等待在途 native 写入退出；5 秒等待超时保留 owner 和 Unknown，不表示电机已停止。停止不追加归中或 disable 写入。

## 持久事实与取消

Server 以原 `(response_id, phase)` 固定动作意图。新发送必须先在 PostgreSQL 原子提交命令、唯一 dispatch permit 和审计；最多 64 个活跃动作记录，每个 Response 最多 32 条。短租约续期不能延长绝对观察期限。丢失 ACK 后只能查询或精确取消原身份，重启和目录变化不能授权重播。

`Submitted` 仅证明已在串口主机写入、I²C 寄存器写入或模拟边界提交。当前 backend 是开环，`position_confirmed_actions` 必须未知；`writer_quiescent` 也只表示软件 writer 已退出。表达解除软件占用需原回执的终态与 quiescence，不能拿本地 skip、超时或另一个动作的回执代替。

Unknown 保留原状态和已知前缀；迟到回执写入独立检查点，不能重写冻结的结果。原命令与回执 JSON 仍可读取，事实可读不等于当前有权输出。数据库表为 `motion_deliveries` 和 `motion_delivery_late_checkpoints`，表达交互结果字段为 `motion`。

## 从 Body 命名升级

Server 与 Embedded 必须协调停机并升级：退休的 `alia.body.v1.BodyControl` endpoint 和 `alia-body-v2-*` 签名域没有别名或兼容验证器。旧客户端不能调用新 Motion endpoint；旧签名不能被当成新请求。升级前结束现有软件 owner，未知物理结果仍需依原设备事实处理。

新增迁移 `m20261019_000001_rename_body_delivery.sql` 重命名两张表、索引、约束以及四种审计事件的 envelope 名称，保留命令、回执、dispatch identity 和状态。既有迁移文件及其校验和不变。迁移不会重签原始证据、把 Unknown 改成成功或生成新的派发许可。该迁移完成后，旧 Server 版本不再兼容数据库对象名，回滚需要单独的数据库恢复安排。

历史设计与旧版本测试记录保留在 [D3 历史文档](../refactor/body-command-design.md)；实际机构校准、限位、保持和位置结果仍以现场验证为准。
