# R03-0 PCA9685 初始化与恢复 — HANDOFF

日期：2026-10-14；分工：`settings_auth_reproduce`；本轮基线：`125ea58`。本分工只修改 `packages/alia-embedded/src/module/motion/servo.rs`，新增其私有 `servo/pca_tests.rs` 和本文。Cargo、冻结输入、总证据与 Git 提交由 Root 统一执行；没有连接真实 I²C 设备、SSH、录放音、GPIO/OE 或其他硬件动作。

## Checklist 与当前进度

本分工实现交接 **6/6**，Root 实际 GREEN 已完成并独立读回；实机验收 **0 项**。软件恢复提交与寄存器模型不证明舵机位置、扭矩或物理保持。

- [x] 阅读 Motion 审计、backend 构造／提交／关闭调用者、writer 证据消费及 NXP 主源。
- [x] 仅加入私有 I/O seam，保留旧初始化序列，交 Root 取得三项有效 RED。
- [x] 修复 AI、读前快照、EXTCLK 拒绝及单板取消／故障有界恢复。
- [x] 覆盖多板构造未提交时的逆序恢复与恢复失败继续处理其余板。
- [x] 完成 14 项离线寄存器模型测试及 scoped rustfmt；源码交 Root 冻结。
- [x] Root 执行冻结候选验证，独立读回并回填 GREEN；最终证据封存与提交由 Root 统一完成。

## RED：实际缺口已经复现

Root 运行 `cargo test -p alia-embedded --lib module::motion::servo::pca_tests --offline --locked -- --nocapture`，结果 **0 passed / 3 failed / 0 ignored**，退出码 **101**，17.21 秒，默认测试并行度，输入无漂移。冻结 manifest SHA-256：`3776a602beb56de64be8c33bd2da3d4fae642b9ae604efe3fd02ec7c761225e3`。原始记录位于 `run/validation-20261014-r03-0/red-pca.{json,log}`，同时保存 `red-source.patch` 与 `red-source-manifest.json`。

三项均运行真实生产 `Pca9685Device::initialize`，当时只把私有设备类型改为默认 `File` 的 `Read + Write` 泛型，没有修复旧顺序：

1. 初始化后实际调用 `set_pwm(2, 0x0123, 0x0345)`：模型四寄存器实际得到 `[3, 0, 0, 0]`，目标应为 `[35, 1, 69, 3]`。AI 未开启导致数据重复写入同一寄存器。
2. 在成功写 SLEEP 后精确触发取消：返回取消错误但 MODE1 为 `0x91`，稳定部分 `0x11`，没有回到原 `0x0b`。
3. 注入 PRE_SCALE 写失败：同样遗留 SLEEP，并丢掉原 MODE1 低位。错误没有被吞掉，但退出前没有必要恢复。

模型事件在真实寄存器事务处触发，不用 sleep 猜取消时机。测试中的所有寄存器值和错误均为合成数据。

## 已实现的协议与所有权边界

依据 [NXP PCA9685 Rev. 4 数据手册](https://www.nxp.com/docs/en/data-sheet/PCA9685.pdf) §7.3.1、§7.3.1.1 和 §7.3.5：AI 控制地址自增，PRE_SCALE 只在 SLEEP 时可写，清 SLEEP 后需至少 500 µs 再处理 RESTART／PWM；RESTART 是状态及写一触发位。EXTCLK 具有保持特性，当前驱动的 25 MHz 计算不支持未知外部时钟。

- `Pca9685Device<Io = File>` 是私有窄 seam；实际 native open 仍返回 File。生产与模型共用同一 initialize、wake、restore、set_servo_angle、set_pwm 和单次 I²C transaction 实现；没有新依赖、公共 API、通用硬件抽象或测试专用生产钩子。
- 首个可能改变寄存器内容的写入之前，保存真实 MODE1 和 PRE_SCALE。先检查 EXTCLK，遇到已启用的外部时钟就拒绝，不能默默用错误时钟计算频率，也不尝试 reset 清位。
- 成功初始化保留原 ALLCALL／SUB1–3 等模式选择并开启 AI，AI 在本次 sleep、wake、restart 中保持；不再先把 MODE1 重写为 ALLCALL 后才读“旧值”。
- `wake` 只有在清 SLEEP 的完整写事务成功后才等待 500 µs、读当前 MODE1 并按当前 RESTART 状态处理。短写／错误不会继续假定振荡器已运行；这是软件步骤保证，不是硬件时钟测量。
- 清理责任从首次尝试写 MODE1 开始，包含该写返回错误或短写的情况。失败／取消时由原同步设备所有者调用 `restore`，不把中间状态交给另一个 task，也不丢弃正在执行的 native 系统调用。
- 恢复先尝试建立原模式的 SLEEP；只有完整提交成功才写原 PRE_SCALE。无论恢复频率是否失败，原本 awake 的设备仍尝试恢复原运行模式。原本 asleep 的设备不会因恢复而被唤醒。恢复错误按阶段收集，保留原错误和恢复错误，并明确 `output state unknown`。
- 每板恢复最多四次配置数据写事务和一次 MODE1 读回，不循环重试，不重放目标 PWM，不对短写追加后缀。这个界限是事务次数，**不是阻塞 I²C 系统调用的墙钟完成上界**。

`initialize_pca_boards` 被真实 `Pca9685ServoBackend::new` 调用：保留已完成初始化板的原快照；后板 open／初始化失败、取消或构造提交前最后一次取消检查失败时，按逆序恢复此前板。某一板恢复失败不会提前放弃其余板，错误全部保留；当前失败板由其自身 initialize 恢复，不重复恢复两次。

整个构造成功返回后，快照不再作为跨命令回滚凭据。其后取消仍由原有逐通道准入阻止目标写入，不把已经提交的初始化重新解释为全局原子事务。backend close、全局 lease、绑定撤销、观察者断开规则、PWM 目标算法、开环 `position_confirmed_actions=None` 均不在本分工修改范围。

初始化失败仍沿既有 `Result`／`ServoWrite.error` 路径上报，不虚构已提交目标通道。恢复日志中的 “restoration submitted” 只表示必要寄存器恢复事务的主机提交；即使模型恢复成功，也不据此声明板卡真实输出或机械状态已确认。

## GREEN 的 14 项回归

原三项 RED 回归保留。其余 11 项覆盖：

- 原 awake/asleep、原 AI 开／关、ALLCALL/SUB 位保存，真实通道 15 burst 与 AI 全程保持。
- SLEEP、PRE_SCALE、wake、restart 各阶段取消；原本 sleep 的早期取消没有任何 wake；目标 PWM 区保持。
- 各配置写阶段的写前错误、写后错误、短写；原模式／频率恢复，事务数有界且没有后缀重试。
- 初始 MODE1/PRE_SCALE 读取失败与 EXTCLK 拒绝时无寄存器数据写。
- 恢复 SLEEP、PRE_SCALE、wake、restart 的错误／短写保持双重原因和 unknown；恢复 wake 失败不继续读回或 restart。
- 成功初始化后的取消阻止首目标帧；真实 PWM 短写仍产生准确不确定通道且不发下一通道。
- 后板 open 失败的逆序恢复，后板初始化失败／取消恢复所有尚未提交板，某板恢复失败仍继续恢复更早板，以及最后一板完成后在构造提交检查处取消。

GREEN 模型相较 RED 增加了活动 PWM 状态：只有正在运行的 PWM 进入 sleep 才产生 RESTART；原本 sleeping 且 RESTART=0 不被误建模为活动通道。恢复断言比较稳定配置位，对原 awake 另断言 RESTART 已消费；不把 RESTART 当作普通可写回的快照位，也不修改生产逻辑迎合模型。模型没有声称模拟完整芯片、总线电气、真实刷新周期或振荡器稳定时间。

Root 已运行 `cargo test -p alia-embedded --all-targets --offline --locked`，默认 features／测试并行，mock 和 loopback 环境。结果为 **lib 199/0/0、integration 64/0/0，共 263 passed / 0 failed / 0 ignored**；其中上述 PCA **14/0/0**，包括原三项 RED，全部通过。退出码 **0**，89.4 秒，冻结输入 **657**，manifest SHA-256 `6c79ccc09484f47fd69ca8e5e93a243cb08a4b53dacfe36c3c02f02d349875ff`，`source_drift=[]`。本分工独立读取 `run/validation-20261014-r03-0/green-embedded.{json,log}` 核对结果，未自行占用 Cargo 锁或另开测试窗口。

Root 报告 Embedded 格式检查通过；全仓格式检查仍有未修改的 Server 基线 import 排序差异，不在本分工扩修。当前 Linux Docker daemon stopped，本轮没有 Linux 执行结果，不能用本地 GREEN 代替跨平台或实机证据。

## 恢复检查点

源码保持 Root 冻结版本，GREEN 结束后仅回填本文。当前无本分工未解决的软件测试失败；下一步由 Root 统一归档全部证据、独立审查结论与正常提交。

板卡具体型号、OE／供电／共地、通道映射、机构校准和实际电气条件仍待核实。本轮没有启用硬件、改变实机部署或证明停止保持；实机启用必须在 R03-0 软件收口和既有硬件前置条件满足后进行。
