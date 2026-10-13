# 2026-10-14 硬件确认 — Motion 只读审计 HANDOFF

分工：`settings_auth_fix`。本轮只读核对 Motion 的停止、取消、关闭、初始化与反馈证据；只新增本文。未修改生产代码或配置，未运行 Cargo、数据库、Docker、SSH、Git 提交或任何硬件 I/O。源码行号是本轮静态读取位置，后续修复后应以符号名重新定位。

## 已确认与未确认

用户确认现用 **PCA9685A 驱动板 + MG996R**，停止及断连时希望**保持**；启动规则已确认：**完成机构校准后允许启动归中，未完成校准不自动归中**。未来考虑幻尔总线舵机；目前没有原理图或校准数据。这是产品与器件名称确认，不是板卡接线、实机配置或物理性能验收，也不授权当前无条件启动运动。

尚未确认板卡具体型号/芯片顶标、通道映射、OE 接法、独立舵机供电/共地、负载、各轴安全角度/脉宽、实际部署配置。NXP 的 PCA9685 芯片资料可核对当前驱动的寄存器协议，不能替代未知驱动板原理图；不能仅凭 MG996R 标签推定厂家、变体或校准范围。

仓库 `MotionConfig` 默认 `enabled=true`、`hardware_enabled=false`、`servo_backend="serial"`。`create_servo_backend` 在硬件禁用时使用 mock（`servo.rs:123`）；PCA 路径仅是已实现选择之一。本次不声称设备当前已使用 PCA 后端，也不启用它。

## 审计完成数

静态审计 **8/8 完成**；本分工软件测试 **0 项执行**、实机验证 **0 项执行**。完成数不代表 R03 或硬件保持验收通过。

- [x] 区分用户确认、仓库默认值、官方芯片资料与待测硬件事实。
- [x] 核对精确取消、lease 关闭/到期、binding 撤销及 RPC 观察者断开。
- [x] 核对 runtime stop/drop、writer 归中及 backend close。
- [x] 单独识别启动归中，并记录用户确认的机构校准前置条件。
- [x] 核对 PCA 初始化、逐通道写入、短写与取消边界。
- [x] 以 NXP 主源核实 AI/SLEEP 协议，不以成功系统调用代替协议正确。
- [x] 核对 `position_confirmed_actions=None` 与未来反馈后端边界。
- [x] 给出分开的最小修复范围与必要测试，未更改全体状态机。

## 停止与保持的实际行为

| 事件 | 当前代码行为与证据 | 对“保持”的结论 |
| --- | --- | --- |
| 精确取消某一命令 | `runtime.rs:780` 按完整 command identity 取消；`cancel_matching:344` 取消当前匹配 permit、移除匹配队列；最多等 5 秒见到该命令 `writer_quiescent && end`，超时仍保留 native owner | 不主动归中；已进入 native write 先完成，后续写入检查取消。其他有效命令/face 仍可能运行，不能将单命令取消称为全局保持 |
| 当前 control lease Close | `runtime.rs:585` 仅允许准确当前 lease，`State::revoke:376` 撤销 authority、递增 generation、取消 face 与所有命令；不归中 | 关闭准入不等于 native 写入或机械运动已停止；`grpc/body_service.rs:256` 已明确该边界 |
| binding 改变 | `module/builder.rs:462` 的 invalidation 回调调用 `revoke_control` | 沿用 lease 撤销，不安排归中 |
| lease 过期/失去续约 | `runtime.rs:29` 允许授权最长 60 秒；`:84` 在 permit 检查时识别过期，writer/逐通道 native write 前检查 | 是有界授权失效机制，不是“网络断开的那个瞬间已保持”。已进入 native write 仍需等待返回；不把 60 秒当物理停止上界 |
| 单个 RPC/观察者被丢弃 | `grpc/body_service.rs:1` 明确只停止观察，不丢弃 device owner；Observe 在 `:211` 查询已有命令 | 不自动撤销全局 lease。不能让任意观察者断开获得撤销所有命令的能力 |
| 服务端 body owner 正常关闭 | `packages/alia-server/src/channels/embedded/body.rs:948` 保留命令所有者收尾，再在 `:980` 尝试发准确 lease Close；失败记录错误 | 网络不可达时 Close 未必送达，设备依靠既有授权到期；不宣称即时物理断连保持 |
| embedded runtime stop/drop | `runtime.rs:1004` 封闭准入、撤销并等待已有准备任务与阻塞 writer；`:1063` Drop 同样封闭/唤醒 writer。正常 writer 退出循环后，`writer.rs:90` 可安排新的本地 reset | **当前默认与用户要求冲突**：`reset_on_stop=true` 时停止后会新增向 center 的运动 |
| native 错误 | `writer` 的错误提前返回，runtime 保留失败事实并关闭 backend；`runtime/tests.rs:892` 验证不使用不确定 native targets 做 reset | 故障不能伪造保持位置或重试旧目标；当前失败证据需保留 |
| backend close | trait 默认 close 无操作；`DeferredServoBackend::close` (`servo.rs:188`) 取走并 drop backend/file。没有 PCA close 寄存器序列或 OE 控制 | 没有显式关 PWM、归零或断力操作。仅关闭 Linux 文件不能证明物理保持；供电、时钟、OE 与机械状态仍未知 |

以上源码简称均指 `packages/alia-embedded/src/module/motion/`，另有明确路径的除外。

软件层的“停止保持”可承诺的是：取消生效后不再发起后续目标运动，保留已提交/不确定通道的真实证据，不主动归中或 disable。即使驱动寄存器正确，也不能用它承诺机械已到位、刹停或掉电保持。

## 1. 停止默认归中：直接的策略冲突

`config/types.rs:690` 使用 serde `default_true`，`:733` 的 `Default` 也为 true；`config/embedded.example.toml:183` 显式 true。`writer.rs:90–100` 等原工作结束后创建新的 `Permit::local()`，再调用 `reset(reset_duration_ms)`。该动作不借用已撤销的远端 lease，因而远端取消不阻止它。

`writer.rs:263` 的 reset 目标是每通道 `center_angle`，不是零度。仓库中心默认 90 度、reset 最小时长默认 1200 ms（`config/types.rs:865–872`）；它们均不是当前硬件的校准值。

现有 `runtime/tests.rs:662` 的 `shutdown_waiter_drop_keeps_join_and_reset_runs_only_after_entered_write_returns` 显式打开 reset，并断言“启动、旧在途帧、停止 reset”共 3 次；这是已被测试固化的旧选择。修复不能只删除该断言而丢掉“stop waiter drop 后仍保留 join、native 单写者、幂等 stop”的保护。

**最小后续范围：** 单独收敛停止契约、serde/default/示例与对应测试。若产品要求固定保持，应移除或明确拒绝会重新启用自动归中的旧配置路径，而不是只把示例改 false 后留下旧 true 配置可生效；由 Root 按本轮迁移范围决定。不要在此改 RPC 观察者所有权、按目标精确取消或 I²C 短写语义。

最少验收：在已进入 native write 时 stop，确认 stop 尚未证明 quiescent；释放旧 write 后，无新 center/disable 写入；stop waiter 被丢弃后后续 stop 仍可 join；重复 stop 无额外写入。对显式旧 `reset_on_stop=true` 配置给出已决定的拒绝/迁移行为。

## 2. 启动归中：已确认须先完成机构校准

`writer.rs:31` 将软件 previous 初始化为中心角，但这不是读取实机姿态。`:35–48` 无条件创建 local startup reset；`soft_start_enabled=false` 仅令 duration=0，仍执行 reset 并提交 center 帧。故仅改 `reset_on_stop` 不会防止进程重启后的自动运动。

用户已确认完成机构校准后允许启动归中；未完成校准不自动归中。当前没有校准资料，现有无条件 startup reset 尚不符合该前置条件；**R03-0 未实现**，机构校准前应保持 native 启动运动禁用。后续只围绕校准准入、startup/reset、ready 的定义及无已知 previous 时首个显式命令的速度规划作局部设计，不将中心默认值写作安全姿势。即使校准完成，也不能用虚构的 previous=center 证明从真实当前位置平滑启动，不能顺带重写 lease 状态机或改变停止/断连固定保持的要求。

## 3. PCA9685 AI 位：确定的静态寄存器协议缺口

相关实际函数：`servo.rs:399` `Pca9685Device::initialize`、`:442` `set_pwm`、`:479` `write_many`、`:113` `write_i2c_transaction`。

当前 initialize 先写 `MODE1=ALLCALL=0x01` (`:407`)，再读 `old_mode` (`:411`)，睡眠/设置频率后恢复 old_mode (`:418`)，最终写 `old_mode | RESTART | ALLCALL` (`:421`)。**bit 5 AI 从未开启**。之后 set_pwm 以 `[base, on_low, on_high, off_low, off_high]` 为一次 I²C transaction，依赖寄存器地址自增。

NXP 官方 PCA9685 Rev. 4 第 14 页 §7.3.1、Table 5 说明：MODE1 bit 5 的 AI=0 禁用地址自增，AI=1 才在读写后自增。当前多寄存器 burst 与初始化不匹配；成功写入 5 bytes 本身不能证明四个 PWM 目标寄存器按计划更新。这是代码与主源协议对照结论，尚未在用户板卡测量。[NXP PCA9685 数据手册](https://www.nxp.com/docs/en/data-sheet/PCA9685.pdf)

**最小后续范围：** 限于 PCA 寄存器初始化与测试，正确设置并在 sleep/restart 周期保留 AI；保留逐通道取消、一次 I²C 写事务、短写不重放后缀，以及 owned blocking writer。若为测试需要 seam，可在同文件把实际寄存器 read/write 序列接受私有可注入 I/O，测试生产 initialize 与 set_pwm 的组合；不要只测试常量或在测试中重写初始化逻辑，不新增依赖/平台/总线舵机框架。

现有 `servo.rs:688` 检验短写不重试，`:696` 检验部分/不确定通道，`:710` 检验逐通道授权，`:729` 检验延迟打开设备；均没有覆盖 PCA MODE1 初始化与 PWM burst 地址。新增最少一项寄存器模型用例，要求实际初始化后连续四寄存器更新正确，并核对过程中 AI 未丢失；原短写和取消用例继续保留。

## 4. 初始化 SLEEP 后取消：独立的恢复边界

`servo.rs:414` 成功设置 SLEEP 后，`:415` 或 `:417` 的取消检查可能直接返回；PRE_SCALE 写失败也可在恢复前返回。`Pca9685Device` 没有 Drop 恢复寄存器，初始化失败最终只 drop 文件。即使 PWM 寄存器保留，也不能声称继续输出原来的 PWM。

NXP 同页说明 SLEEP=1 关闭振荡器；第 15 页 §7.3.1.1 描述从 sleep 恢复通道的步骤。因此在已运行板卡重新初始化时，这个取消点可能破坏“保持输出”的预期。[NXP PCA9685 数据手册：MODE1 与 Restart mode](https://www.nxp.com/docs/en/data-sheet/PCA9685.pdf)

**最小后续范围：** 保留 native writer 所有权，明确“已成功进入 sleep 后”的最短必要收尾由原初始化 owner 承担；取消阻止首个目标帧与之后的动作，但不能在自己造成的中间设备状态后直接遗弃恢复。需区分取消、一次寄存器错误与结果不确定；恢复失败仍报告原/恢复错误及未知输出，不能盲目重放目标、擅自 disable 或声称恢复成功。具体恢复写序列需结合保存的 MODE1/频率状态设计，不能只把取消检查删除后把全部初始化都称安全。

最少故障用例：取消发生于 sleep 成功后、prescale 成功后及恢复前；注入 prescale/恢复写失败；断言必要收尾次数有界、无目标 PWM frame、无短写后缀重试、错误不伪造已恢复。该修复可与 AI 修复共享很窄的寄存器测试 seam，但两项证据和验收应分别记录。

## 反馈证据与未来总线舵机

NXP 将 PCA9685 定义为 16 通道 PWM 输出控制器，输出端无输入功能；读取其输出寄存器不产生机械位置反馈。[NXP PCA9685 功能说明，第 1–2 页](https://www.nxp.com/docs/en/data-sheet/PCA9685.pdf)

TowerPro 的 MG996R 官方页列的是 JR/Futaba 兼容的 RC 舵机连接方式，没有提供可由本项目读取的总线位置反馈协议。这不证明任意同标签变体的所有电气细节；仅能确认当前已知链路没有独立位置反馈证据，不能把舵机内部控制或目标脉宽当作主机测得位置。[TowerPro MG996R 官方产品页](https://towerpro.com.tw/product/mg996R/)

当前 `BodyProgress` 的 `position_confirmed_actions=None` 正确：`crates/alia-core/src/body.rs:268` 说明 v1 为开环，`:290` 强制未知；`runtime.rs:176` 构造 None。`ServoBoundary::I2cRegisterWrite` 和 SerialHostWrite 只报告提交边界，`writer.rs:115` 的 previous 只保存软件已提交目标。保持这套区分；本轮不能新增虚构的 position-confirmed 成功。

幻尔总线舵机只是未来候选，没有具体型号/协议，不应现在添加驱动、地址规则、电压或反馈能力假设。确定型号后再核对反馈字段、时效、取消/失联策略与合同版本；v1 当前强制 None，未来真实反馈不能直接把字段改成 Some 绕过验证。

## 分开的后续 checklist

- [ ] 停止：实现已确认的保持契约，收敛旧自动 reset 配置并保留 native 清理所有权测试。
- [ ] 启动（R03-0 未实现）：落实已确认的校准准入；完成机构校准后允许启动归中，未完成校准禁止自动归中。当前无校准资料，不得以中心默认值替代实机姿势。
- [ ] PCA 协议：修正 AI 位，并以实际生产初始化/PWM 序列的寄存器测试证明。
- [ ] PCA 取消：封闭 SLEEP 后取消/失败的恢复边界，保留未知结果与错误证据。
- [ ] 断连：明确测试的是 binding revoke、lease 到期、server owner Close 还是不可达网络；记录实际检测与停止写入界限，不授权任意 RPC observer 撤销所有工作。
- [ ] 实机：补板卡接线/供电/OE、通道和校准依据后，单独验证保持、重启、取消、断连和负载表现；本次没有执行。
- [ ] 反馈：当前保留 None；总线舵机型号和协议确定后再立项真实反馈，不在本轮扩大驱动范围。

这些是待办，不是本分工已完成的代码修复。硬件校准、断连物理表现和持续扭矩均未测量；尤其不能把当前默认 mock 或已有提交回执当作 PCA+MG996R 实机通过。
