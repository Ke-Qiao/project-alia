# D3 身体动作命令设计（待实施）

状态：2026-09-08 只读设计已收敛，尚未实施 command-v1。本文记录下一包边界，不表示协议、硬件停止或位置反馈已经验收。D1 已对缺少 `expression-command-v1` 能力的旧适配器拒绝执行身体动作；不能仅添加能力宣告来绕过此保护。

目标是打通命令身份、设备唯一写入者和真实回执观察。保留当前校准文件、限位数值和硬件配置默认值。不假定 Pi 5 上的实际控制板、固件或舵机具有位置反馈，也不开展 Easy-Turn 性能验证。

## 现有代码与可复用边界

以下是设计时源码的静态观察，路径与实现会随后续包变化；并非本设计新增测试的结果。

| 位置 | 已观察行为与处理建议 |
| --- | --- |
| `crates/proto/proto/expression.proto` | 只有 capabilities/execute/cancel JSON 包装；请求没有 command identity、scope、sequence 或 typed progress。需建立明确协议。 |
| `packages/alia-embedded/src/grpc/expression_service.rs` | 一次 bearer auth 后，由 RPC future 直接 await motion 执行；取消只有 reason。改为向 Motion owner 提交并观察精确命令。 |
| `packages/alia-embedded/src/module/motion/runtime.rs` | `busy + cancel` 没有精确所有权；取消立即清 busy，旧 execute 结束也无条件清状态。人脸跟随先检查 busy 后写入，有竞争窗口。表达、跟随、启动和关闭动作必须进入同一 body writer。 |
| `packages/alia-embedded/src/module/motion/servo.rs` | 串口同步 `write_all/flush` 无 MCU ACK 或位置回读；I²C 按通道同步写入，可能部分成功。保留实际 native worker owner，报告提交边界，不能宣称物理完成。 |
| `packages/alia-embedded/src/module/motion/neck_mapper.rs` 与 runtime 限速插值 | 可复用映射、当前校准与限速；补齐有限值、尺寸和动作总量校验。`last_angles` 是上次命令值，不是实测位置。 |
| `packages/alia-embedded/src/binding/state.rs`、`crates/transport/src/lib.rs` | 令牌签署 Server、scope、时间，不绑定设备或动作。D2 已补 Bound 校验与绑定读锁内同步接纳，可以复用；D3 还需保留绑定身份、版本和动作内容证明。 |
| `packages/alia-embedded/src/module/speech/duplex.rs` | 本地 Ready/fence/close 已拥有语音作用域，但只约束播放；需接身体动作。 |
| `packages/alia-embedded/src/module/registry.rs` 与 lifecycle | 保留 cleanup owner 和真实 join 的模式可复用；停止动作必须纳入同一生命周期。 |

## 1. 身份与有限内存反重放

Builder 为每设备进程生成一个共享 `device_boot_id`，注入 Speech、Motion 与服务。当前 boot UUID 仅在 SpeechPipeline 内生成，需提升归属。

每个 boot 只保留一个当前绑定 Server 的 `BodyControlLease`。`OpenControl(expected_generation, server_instance_id)` 通过 CAS 推进设备维护的 control generation。更换、关闭或撤权立即淘汰旧 generation；同一旧 Open 不得重新打开已关闭 lease。替换 admission 可以立即失效旧身份，但新 writer 必须等待旧 writer 真正退出；停止未确认时保持隔离、拒绝新身体写入。

同一 lease 内所有用户与 voice/nonvoice 命令共用单调 `command_sequence`。`command_id` 必须可验证地绑定 boot、control generation、sequence；`plan_id` 独立用于业务关联。否则旧 UUID 换新序号仍可能绕过有限缓存。

设备只保留 active command、有限回执与最高已接纳 sequence。相同身份、相同 intent 返回已有观察；同身份不同内容拒绝。低于水位的命令即使回执已经淘汰，也只能返回 retired/result_unavailable，绝不重执行。序号溢出关闭 admission，禁止回绕。内存容量不能依赖 LRU 淘汰后把旧命令当成首次执行。

## 2. 原作用域、绑定与签名

命令包含目标设备、绑定/control generation、用户、完整身份、原始作用域、优先级、动作内容摘要和协议版本。作用域明确区分：

- Voice：复用 `VoiceScope`，匹配本地 Ready 建立的 session 和当前 epoch。Complete、Wait、session close 同时淘汰旧身体命令；control lease 不能绕过语音 fence。
- Nonvoice：独立 `scope_id` 和用户归属，受同一 control lease 约束，不制造假的 VoiceScope。全局 sequence 水位承担反重放，无需无限保留 nonvoice scope 集合。

复用现有 Ed25519 工具，对有长度上限的明确协议 payload 加域分离签名。设备回执也绑定并签署同一身份。当前 gRPC 服务没有 TLS 配置，仅签 bearer token 不能证明具体动作内容未被替换。签名验证应针对收到的明确 payload 字节，禁止以不稳定 JSON 重序列化规则推断相同内容。

认证和接纳之间检查同一绑定版本。HandoffPending、解除绑定、密钥更换及令牌到期关闭旧 admission，精确取消其命令；不能只在 RPC 开始检查一次。保留明确的本地开发模式标识，不将模拟认证证据当生产身份验收。

## 3. 单一 body writer 与真实停止

MotionRuntime 成为有界命令 owner，统一表达、人脸跟随、启动回中和关闭回中。人脸跟随只保留一个最新目标槽位，低优先级可丢弃；普通表达不建立无限队列。取消与本地停止使用独立控制信号，不排在满动作队列之后。

沿用 Low/Normal/High 时，设备仍独立检查优先级、频率和安全条件。本地停止最高优先级，远端不得凭任意字符串取得此权限。抢占须等待旧 writer 不再提交，才放行新 writer。

串口/I²C 的同步调用进入单一、保留句柄的 native worker。RPC drop、观察断开、超时或 panic 不能丢失唯一 owner。native 写入阻塞时报告 Unknown，禁止新命令进入；生命周期报告 Incomplete 并继续保留 join，不以超时或 abort 宣称硬件已停止。

原 `reset_on_stop` 只在旧 writer quiescent 后作为独立生命周期动作执行，沿用原配置。精确取消不偷偷变为回中动作。没有设备停止/断电协议时，只能证明软件不再提交，不能证明电机物理静止。

## 4. 回执只报告真实边界

最小事实链为 Accepted → Started → Submitted；另有 Rejected、Cancelled、Failed、Unknown。只有具有实际位置反馈的 backend 才能报告位置确认完成。

Submitted 明确标注边界：串口主机写入、I²C 寄存器写入或模拟写入，并保留已提交动作/帧及部分通道信息。当前串口和 PCA9685 实现均不能证明舵机到位。取消 ACK 精确包含目标身份、是否匹配、writer 是否退出以及最后提交位置；旧取消不影响新命令，没有当前命令也不等于原命令完成。

首次写入前完整校验有限数值、动作数、动画文件大小与帧数、总时长、循环数和实际映射尺寸。保留校准及限位值；现有频率上限必须在设备真正执行。无传感器时不把 `thermal_limit:false` 当作已测得安全。已有上次命令值仅用于开环规划，不能用它证明重启后的实际速度或位置。

## 5. Server 观察与恢复

建议 RPC：OpenControl、Submit、Watch、Cancel、CloseControl。Submit 快速返回接纳事实；Watch 可重新观察同一已知命令，不触发重发。

D1 当前两秒执行超时不能直接承载最长五秒动作；Accepted 后也需持续观察才能解除本地占用。实例 orchestrator 应保留有界观察 owner，按精确身份更新状态。Submitted 可以结束软件提交阶段，但 `completion_acknowledged` 仍为 false。

Server 重启、断线或丢 ACK 后只查询原命令事实；无法观察则 Unknown，不把同一计划包装成新身份重放。设备重启使旧 boot 失效，不由此推断旧动作完成，也不自动发出未经确认的新回中/校准动作。

当前 Delivery identity/progress 是播放类型。接入身体证据时须增加明确 body variant，记录命令关联、实际提交与未知结果，不能借用音频 sample 数表示动作。此项属于 root 的共享契约和整合范围，不在本只读设计中修改账本。

## 实施文件所有权建议

| 独占包 | 文件范围 |
| --- | --- |
| Embedded Motion | `module/motion/{runtime,servo}.rs`、必要新 `command.rs` 及测试；`neck_mapper.rs` 仅补验证，不改校准数据 |
| Embedded 命令 RPC | `grpc/expression_service.rs` 及真实 loopback 测试 |
| Server 表达 | `expression/{orchestrator,types}.rs`、实例观察 owner 与测试；Embedded client 待当前 endpoint 包冻结后接入 |
| Root 共享契约 | `crates/proto/proto/expression.proto`、Core command 类型、transport 签名辅助；Builder 共享 boot、绑定版本、Ready/fence/close body hook；Delivery body 证据类型 |

实现顺序：先冻结共享身份与回执契约，再并行实现 Motion owner/真实驱动边界与 RPC/Server 观察，最后接生命周期、作用域和账本进行端到端故障测试。不能在完整命令路径就绪前宣告 command-v1 能力。

## 必须失败测试与硬件边界

1. 相同命令重试及同身份变造内容均不重复写；回执淘汰后旧 sequence 仍拒绝，序号溢出不回绕。
2. 旧 Open、旧 boot、旧 voice epoch、已关闭 nonvoice/control lease 无法复活；并发 Open 只有一个 CAS 成功。
3. 两个 producer 竞争时只有一个 writer；旧 cancel、旧 completion 和旧观察断开不影响新命令。
4. 满队列不阻塞控制；同步写阻塞时不虚报 cancel ACK、不丢 join，停止未确认不得启动新 writer。
5. 部分 I²C 写失败保留已发生证据；开环提交永远不变为位置完成；无传感器不伪造安全测量。
6. End/Submitted 后仍能读取最终命令事实；丢 ACK、Server 重启、设备重启均不自动重放物理动作。
7. 绑定撤权、HandoffPending、密钥变更、签名变造或错误目标设备阻止写入；RPC drop 不遗留无 owner 的执行。

这些首先用 fake driver、真实 gRPC 和可控阻塞 native worker 验证。尚待实物确认：实际控制板与固件、串口命令 ACK/队列机制、舵机反馈能力、输出禁用/保持策略，以及断电和失联安全行为。软件协议与模拟控制测试通过，不等于物理停止、位置完成或硬件安全验收。
