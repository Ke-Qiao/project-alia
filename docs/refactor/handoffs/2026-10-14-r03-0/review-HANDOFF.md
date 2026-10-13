# R03-0 独立审查与回归设计 HANDOFF

日期：2026-10-14。分工：`settings_auth_review`。本轮基线由 Root 指定为 `125ea58`；只修改本文，生产及测试文件由 lifecycle / PCA owner 修改，测试执行和同源证据由 Root 统一管理。本分工未启动 Cargo、数据库、Docker、SSH 或任何硬件 I/O，也未提交 Git。

## 审查 checklist

- [x] 读取 stop/start、校准、精确取消、lease、binding 与 RPC observer 实际链路。
- [x] 检查配置文件、库构造、启动、配置序列化及示例的迁移影响范围。
- [x] 独立对照 NXP 官方资料核对 MODE1 / AI / SLEEP / PRE_SCALE / RESTART。
- [x] 审查 PCA 的 RED 寄存器模型，向实现 owner 交付必要的 GREEN 断言。
- [x] 识别并向 Root 提出多板初始化未提交时的部分改变；Root 已纳入本轮。
- [x] 检查现有 Linux 容器脚本、缓存与 native archive 路径的可复用边界。
- [x] 读取 Root 实际 RED 日志并核对见证与失败原因。
- [x] 读取两分工最终 GREEN 源码及测试，确认阻断项收口。
- [x] 核对 Root 最终 macOS 同源结果、Linux 当前不可用记录及资源边界。

当前 **9/9**，是审查分工进度，不代表硬件或 Linux 验收通过。最终 GREEN 静态审查无阻断，macOS 运行结果已独立核对；本分工已停写。

## 已确认的范围

用户要求停止及断连保持，完成机构校准后允许启动归中；当前 PCA9685A + MG996R 没有提供机构校准资料，未来幻尔总线方案继续暂缓。当前默认 `hardware_enabled=false`、没有启用目标板卡。本轮只处理原停止 reset、启动确认门槛、PCA 初始化协议及失败收尾；不改变工具批准、Body 身份、observer 权限或位置反馈层级。

## 生命周期与配置审查

原 writer 用 `previous=center` 初始化软件目标，无条件运行 startup reset，退出时可另建 local permit 执行 shutdown reset。`soft_start_enabled=false` 只把时长变成零，仍会写入中心；它不是禁止启动运动。原 `reset_on_stop=true` 则直接违反保持要求。删除停机 reset 时必须保留原工作等待、单 writer 与 join 所有权。

Root 已认可的实现方向：

- `calibration_confirmed=false` 表示尚无本地人工校准确认。硬件开启且未确认时，在 native backend 构造/打开前拒绝；未确认 mock 可以启动但不开放动作准入、不写中心。该字段是本地确认声明，不是自动测量，原校准文件解析、通道数量、中心限位和全部目标限制仍必须通过。
- 校准后启动只写一次中心目标。只有完整 native 提交成功才建立上次已提交的软件目标；此前保持未知，不用虚构起点宣称物理平滑或限速。
- 删除 stop reset 路径；四个旧 `soft_start_enabled` / `soft_start_duration_ms` / `reset_on_stop` / `reset_duration_ms` 配置键由严格 MotionConfig 解析明确拒绝，不能静默忽略。TOML、JSON、保存输出、示例及 fixtures 同步迁移，不保留重新打开旧归中行为的暗路径。

调用与回归边界：`config::load_config` / `EmbeddedRuntimeSettings` 读取 TOML；library builder 的 `resolve_config_paths` 与 `MotionRuntime::new/new_with_boot` 也接收配置，不能只保护 CLI 文件入口。默认/未确认配置与原校准校验必须在各入口一致。既有 `grpc/body_service/tests.rs` 使用 mock 创建真实 loopback 服务，新增确认门槛后其受控 fixture 需要明确确认，不能为了过测试绕开生产准入。

最少回归已交 lifecycle owner：

1. stop 发生在已进入且被屏障阻塞的 native write；停机等待者取消后原 owner 仍存在；释放原写入后不新增 center/disable；再次 stop 可 join 且幂等。
2. 未确认 mock 启动零写入且 unavailable；硬件配置未确认在构造前拒绝；已确认但校准不合法仍拒绝。完整启动提交之前不能发布 ready，失败/取消保留未知事实。
3. 严格拒绝四个旧键，并核对默认/示例/保存输出；校准后合法 startup 的中心提交只出现一次。
4. 保留已存在的取消期限后迟到 quiescence、start waiter drop、join error、preparation owner、部分 write、panic 和停止前未启动零写入回归，避免重写这些已覆盖的机制。
5. Root 追加要求已交 lifecycle owner：lease Close / `revoke_control` 在已有阻塞帧时分别验证只保留 startup + entered 两次写入；drop 实际 observation 不取消原命令或 lease，重新观察仍能获得原身份最终提交。测试不以丢弃等待者代替原 owner 清理。

## 实际 RED 证据

已读取 `run/validation-20261014-r03-0/red-stop.*`、`red-startup.*`、`red-pca.*`。三次运行采用同一 manifest `3776a602beb56de64be8c33bd2da3d4fae642b9ae604efe3fd02ec7c761225e3`，均退出 101，`source_drift=[]`，不是编译错误。

| 故障见证 | 实际结果 |
| --- | --- |
| stop 保持 | 释放原 native write 后总计 3 帧，而保持只允许 startup + entered 共 2 帧。 |
| 未确认校准 startup | 实际提交 1 次中心，预期 0 次。 |
| PCA burst | 通道四寄存器得到 `[3, 0, 0, 0]`，预期 `[35, 1, 69, 3]`；AI 缺口被模型实际观察。 |
| sleep 后取消 | 遗留 MODE1 `0x91`；去除状态 RESTART 后 `0x11`，不等于原 `0x0b`。 |
| prescale 失败 | 注入写失败后同样遗留 MODE1 `0x91`，原模式未恢复。 |

五项均为预期 RED。三阶段耗时依次为 stop 0.34 秒、startup 0.14 秒、PCA 17.21 秒（含编译）；这里不推断物理执行器结果。

## 授权与断连边界

| 事件 | 现有边界，后续仍须保持 |
| --- | --- |
| 精确 cancel | 只取消完整 identity 所属命令；不能撤销替代 epoch/lease，已进入 native 调用仍由原 owner 收尾。 |
| lease Close / replace | 校验准确当前 lease；旧 Close 不能撤销新 owner。准入撤销不等于物理停止，已进入写入仍需返回。 |
| binding revoke | builder 的现有输出撤权回调调用 `revoke_control`，取消命令和 face，不新增中心运动。 |
| 授权到期 | 在 permit/frame/逐通道写入前检查，过期不能 renew 复活；最长 60 秒授权不是物理停止上界。 |
| RPC / observer drop | 只丢弃观察者，不撤销原设备 owner；不让任意 observer 获得全局停止权限。 |
| runtime stop / drop | 封闭准入、取消原工作、唤醒 writer，保留准备任务及 native join；stop 不发起新的 lifecycle 运动。 |

现有 runtime old-lease/fence/expiry 回归和真实 gRPC token-expiry/binding/foreign-controller/旧 boot 回归可继续验证这些未改语义。R03-0 不需要新网络心跳或统一撤权框架。

## PCA 协议与恢复审查

独立依据为 [NXP PCA9685 Rev.4 数据手册](https://www.nxp.com/docs/en/data-sheet/PCA9685.pdf) §7.3.1、§7.3.1.1、§7.3.5（第 14、15、25 页）：AI 控制连续寄存器访问；PRE_SCALE 必须在 SLEEP 中修改；内部振荡器唤醒后需要至少 500 µs 再 restart/访问 PWM。RESTART 是状态与触发语义，不能像普通配置位保存恢复。EXTCLK 为 sticky 状态，当前固定 25 MHz 算法不支持它，应在首次数据写入前拒绝。

原 initialize 先写 `MODE1=ALLCALL` 再读 old mode，既未开启 AI，也已经覆盖真实原模式。恢复设计必须在首次数据写入前读取原 MODE1/PRE_SCALE；对读失败或不支持状态不追加修改。原 awake 的取消/失败恢复原频率、低模式位并按实际 RESTART 状态恢复运行；原 asleep 则保持睡眠，不擅自唤醒。恢复是原 native owner 的必要收尾，不受已取消目标 permit 中途遗弃。

最少故障覆盖已交 PCA owner：

- 初始化后由真实 `set_pwm` 发起 burst，寄存器模型按实际 AI 状态决定地址自增；不能仅断言常量。
- sleep 成功后、prescale 后与恢复前取消；prescale 错误；最初 sleep 的短写/错误也可能已经生效，不能假定未进入 sleep。
- awake 恢复须检查 RESTART 触发后状态，而不是把该 bit mask 掉后就声称通道恢复；asleep 恢复不得出现 wake。
- 一次 I²C transaction 短写不得重放后缀。恢复有界且不写目标 PWM；恢复失败保留原错误与恢复错误，报告输出未知，不伪造已恢复。
- 构造多板 backend 时，后一板失败/取消会留下前一板已完成的模式/频率修改。Root 已要求在整体 commit 之前保存各板原状态并有界逆序恢复；整体成功 commit 后的取消只阻止首个目标，不承诺回滚已完成初始化。测试与错误文案须区分这两个时点。

RED 寄存器模型已只读核对：使用私有 `Read + Write` seam 调用生产 initialize/set_pwm；按 AI 改变指针、awake 时忽略 PRE_SCALE 写，并具备注入 sleep 后取消和 prescale 失败的能力。原 RED 的恢复断言不完整验证 RESTART，已明确要求 GREEN 补齐；软件模型仍不能证明真实振荡器、板卡或机械保持。

## 最终 GREEN 静态审查

Root 冻结 657 个输入，manifest SHA-256 为 `6c79ccc09484f47fd69ca8e5e93a243cb08a4b53dacfe36c3c02f02d349875ff`。已读取生命周期七个修改文件，以及最终 PCA 生产实现和全部 14 项寄存器测试；本分工没有修改这些文件。

- 生命周期：旧配置键均移除且 strict schema 拒绝；native 校准门槛在校准文件加载和 backend 构造之前，未确认 mock 没有 mapper/worker/准入能力。`previous=None` 仅在首个完整提交后建立。stop 不再存在 reset 分支，原准备任务/native join 和精确身份行为保留。
- 配置回归同时覆盖 TOML/load/save 与 JSON `EmbeddedSettingsManager`。被拒绝的旧 JSON 键不改变当前 settings snapshot 或磁盘文件；例子默认 hardware/calibration 均 false。
- 新的 Close / revoke 测试分别取得真实 ProbeServo 进入见证，释放原写入后恰为 startup + entered 两帧；observer drop 后重新观察同一 identity 得到 Submitted，原 lease 仍有效。当前测试没有把 RPC transport 断开当作已撤销原工作。
- PCA 的生产 constructor 真实调用多板 helper，单板失败负责自身恢复，再逆序恢复此前完成的板；即使某个恢复失败仍继续其余板。整体 commit 前与 commit 后取消有独立用例，后者不回滚已完成初始化。
- 14 项 PCA 测试用真实 initialize/set_pwm/constructor helper 处理模型；模型按 AI、EXTCLK sticky、`pwm_running` 与 RESTART 状态执行。故障矩阵覆盖各阶段 Before/After/Short，原 sleep 的早期取消不 wake；经历 wake 后取消则恢复 sleep。awake 恢复明确检查 RESTART 消费后的值。
- 恢复 SLEEP 写失败不会追加 PRE_SCALE；恢复 wake 写失败不会继续假定可读运行状态/restart；错误保留原 cause 与 recovery unknown。PWM 短写仅一次事务，原通道标 uncertain，不重放后缀。

未发现当前实现的阻断问题。500 µs 等待位置已静态核对，寄存器模型不测振荡器实际时序。恢复次数有界也不构成 OS I/O 系统调用耗时上界；原阻塞 writer 必须保留到已进入调用返回。日志使用“restoration submitted”，不能提升为机构或真实输出已恢复。

## Linux 验证可复用入口

已读取历史 `docs/refactor/handoffs/2026-10-12-settings-read-authorization/evidence/linux-v3-command.json`、`linux-arm64-v3/evidence/prepare-source.py` / `run-stage.py`。历史镜像固定为 `sha256:6cd7a7a06976f8651cdb57fcde457e9dc7e2abe20587900dd750e1f373a25c70`，运行记录为 ARM64 Debian 12、Rust/Cargo 1.95.0。仅确认下列本地目录仍存在，未执行 Docker 或确认镜像当前可用：

- `run/validation-20261008-layering/linux-arm64-v1/cache`
- `run/validation-20261010-l12/registry`
- `run/validation-20261008-layering/linux-arm64-v1/native`

原脚本硬编码历史目录、container 与数据库，不能原样再次运行或覆写归档。本轮可复用 Root frozen manifest、只读 source snapshot、既有 registry/native archive、`--offline --locked`、前后源哈希、命令/退出码/耗时和最终容器清理。Embedded lib / config 回归无需 PostgreSQL，去掉旧 DB 网络及环境依赖即可；不挂载 `/dev`、不启动 Embedded 部署进程、不映射音频/I²C/UART。默认 feature 会链接 Sherpa native archive；`--no-default-features` 检查是额外构建配置，不能替代默认测试。

建议 Root 执行当前同源的 Embedded 全部 lib、`--test config` 和 no-default 检查，尤其保留 Linux cfg 下的 PCA/ALSA 编译覆盖。真实 loopback gRPC fixture 是本地测试，不需要网络模型、目标 Pi 或执行器。以上仅为可行路径，结果须由 Root 实际执行后填写。

已读取 Root 保存的 `run/validation-20261014-r03-0/environment.json`：Docker image inspect 退出 1，daemon 不可达；`orbctl status` 退出 1、输出 Stopped。默认启动可能恢复上次全部运行机器，因此本轮不自动启动未知机器。这是当前 Linux 执行缺口，历史容器通过不能替代本轮修复后的 Linux 结果。本分工没有启动容器/部署。

## 最终运行证据核对

已独立读取 Root `green-embedded.log/json`、`green-embedded-format.json`、`green-format.log/json` 和 `preexisting-format.json`，并重新计算 frozen manifest 及其中全部 657 个输入的 SHA-256，当前无漂移。manifest 为 `6c79ccc09484f47fd69ca8e5e93a243cb08a4b53dacfe36c3c02f02d349875ff`。

| 验证 | 实际结果 | 审查范围 |
| --- | --- | --- |
| macOS 默认 features `cargo test -p alia-embedded --all-targets --offline --locked` | exit 0，89.4 秒；lib 199 + integration 64 = **263 passed / 0 failed / 0 ignored** | 独立从各 `test result` 重新求和；PCA 14 项、校准/startup/stop、Close/revoke/observer、旧 lease/expiry、native owner 和配置拒绝测试逐项为 ok。 |
| Embedded scoped fmt | exit 0，0.18 秒，无源漂移 | 修改所属 package 格式通过。 |
| workspace fmt | exit 1，0.84 秒，无源漂移 | 只报告未修改的 Server `tests/server_runtime/delivery.rs` import 顺序。独立以 `git show 125ea58:…` 与当前字节比较完全相等，SHA-256 为 `f1c3bbabf0a575161e8a1cadd043d6cb94fb0b57b19fe788cd3a52a6e78d6709`；不顺带修改无关文件。 |
| 本轮 Linux / Pi | 未执行 | Linux 运行环境不可用；未用历史结果替代。没有连接 Pi、打开 PCM/I²C/UART 或启动执行器。 |

本次实际运行的构建配置是默认 features；没有另行声称本轮 no-default 验证。Root 的 Cargo 运行已以 exit 0 完成；没有为本轮启动 Docker、数据库或部署进程。局部 SQLite、临时目录、loopback fixture 及 native worker 的清理回归包含在通过测试中，未据此声称真实硬件资源清理已验证。

## 当前限制

生产源码已完成最终静态审查，实际 macOS 默认全目标测试通过；Linux 当前缺口明确保留。当前不启用硬件、不添加依赖、不推进幻尔驱动、R03-A 资产绑定、R02-A 或新的 Server 权限工作。软件保持表示不追加目标/归中/disable 及保留真实提交证据，不等于机构停止、到位、持续扭矩或掉电保持。
