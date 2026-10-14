# R02 / R03 现场前置 — 部署独立审查

日期：2026-10-14。Root 指定基线 `283f38f` clean。分工 `settings_auth_review`，先只读审查；Root 随后授权编写 `config/embedded.staging.example.toml`、`docs/operations/server-embedded.md` 与本文。没有运行 Cargo、Git、SSH、Docker、模型、设备枚举或硬件；当前现场状态不由本分工重新实测。承接已完成 R03-0、R02-A、R03-A，避免重复软件改造。

## Checklist

- [x] 读取当前 R03-A 恢复检查点、硬件部署清单和历史 Pi 工具链证据。
- [x] 检查 Server/Embedded 无前端操作、local-development、配置示例和当前启动入口。
- [x] 跟踪 Embedded CLI / Builder / registry 的文件、网络、模型和设备启动副作用。
- [x] 区分配置只读检查、无设备启动与真实现场验收。
- [x] 向 Root 与两位 owner 提供实际缺口、最小离线交付及必要部署输入。
- [x] 按 Root 最终分工审阅离线交付与文档修订，不扩大到部署或插件隔离。
- [x] 读取 Root 实际离线验证证据和最终交接，保留现场未验证边界。

当前 **7/7** 为本分工进度。R02/R03 的现场条件与 R05 出口仍开放，不换算项目完成百分比。最终格式检查、证据归档和 Git 提交由 Root 收口。

## 现场已有事实与最少待补项

当前 Mac 是 Server，Pi5 / 8GB、Ubuntu24.04.4 / aarch64、`ke-qiao@192.168.10.221` 及 5V5A 供电规格已由用户确认。历史只读盘点确认 host key / 既有身份登录、Pi 型号/内核/RAM/磁盘；单次温度不代表散热长期达标。R1 尚未 USB 接入，扬声器/功放未选；PCA9685A + MG996R 未给实际接线、供电/OE、通道、机械限位和校准。停止/断连保持、完成实际校准后允许启动归中已确认，不重复询问。

历史 `pi-toolchain-inventory.json` 仅证实 `~/.cargo/bin/rustup toolchain list` 输出 stable-aarch64-unknown-linux-gnu active/default。此前非交互 PATH 未发现 rustc/cargo/protoc，不可简化成“Rust 未安装”。后续真正部署需要固定实际 Rust 版本（仓库 mise 为 1.95.0）、调用环境及 protoc 路径；默认 Embedded 还涉及已选择的 native libraries。当前没有工具链安装授权动作，也不以 Mac 构建代替 Pi 构建。

Root 可集中补充：Mac/Pi 服务账户与独立工作/配置/key/state/plugin/log目录；是否现场已接入 R1、对应稳定 USB/ALSA 标识与服务账户权限；散热与实际音频/执行器资料路径。模型服务端点、版本、权重和独立数据库身份仍待用户确认，不自动选服务或收集凭据值。

## 当前启动入口不是只读检查

仓库与隐藏/ignored 名称搜索未找到项目自有独立 start/stop 脚本。当前实际入口是：

| 入口 | 实际行为与边界 |
| --- | --- |
| `cargo run -p alia-embedded`、package.json `embedded` | 进入正常 CLI，可能创建/修正 key、保存配置并启动所有选定模块 |
| package.json `dev:backend` | 并发启动数据库、Server 与真实 Embedded；不能当无设备预检 |
| package.json `dev` | 还启动延期的 frontend，不适用本轮现场前置 |
| mise `db` / package `db` / `db:stop` | 操作现有 compose 的固定 `alia-postgres`、5432 与开发卷，不是每轮隔离数据库/资源归属工具 |
| `setup-embedded-vision-pi.sh` | 默认 sudo apt 安装、下载模型、Cargo check，`check_camera` 还通过 v4l2-ctl 查询设备节点；即便跳过安装/下载/构建也不是完全不访问设备的预检；`--run` 启动整个 Embedded |

没有把系统/依赖库自己的 launch 文件误当项目启动脚本，也没有运行上述入口。若用户指的是仓库之外的个人启动器，需提供准确路径后再审查，不能猜测或全盘搜索个人文件。

## CLI / Builder 副作用顺序

1. `main.rs` 加载 Embedded 配置；缺文件使用 compiled defaults。普通启动会 `identity::ensure_device_keypair`，可能创建 key 和父目录；即使 key 已存在，`load_device_keypair` 仍调用 `set_permissions(0600)`。`load_device_public_key` 也经过同一权限写入路径，不能用于“纯只读”身份预检。
2. device_id 缺失或 config 不存在时，CLI 会保存配置。`ALIA_EMBEDDED_CHECK_KWS_WAV` 虽可提前退出，但会读取真实 WAV 并创建 sherpa 模型，属于模型文件推断工具，不能替代无模型检查。
3. `EmbeddedBuilder::from_config` 只做路径解析与 speech 配置验证；`build` 则启动有 owner 的异步工作：绑定 TCP listener，创建/迁移 SQLite，构造/初始化模型与模块、扫描插件 manifest、加载或 dev-seed binding。
4. registry.start 先开 playback，再 motion、face/vision debug、可选毫米波 sensor，随后开始 native capture、可能的 speech worker，最后 discovery。`build().await` 成功前已经启动这些工作；不是等到 `node.run()` 才打开设备。
5. PluginRegistry.scan 只载入 manifest、登记 Installed，本身不启动插件代码；但仍不应因没有插件自动启动就把整个 Builder 归为只读。

示例 `[modules]` 空表使用默认 sherpa wake/VAD、keyword、CPAL capture/playback、YuNet、mDNS；vision 默认 enabled，构建缺 feature 时启动拒绝，不是自动降级 noop。motion 的 `hardware_enabled=false` / `calibration_confirmed=false` 阻止真实执行器启动，但不阻止采播、摄像头和发现网络。capture 在绑定前已经开始，不能以尚未配对当作不录音保证。

Server 普通 `ServerConfig::load` 还可能执行 publication recovery 并修改发布状态，已由另一 owner 独立确认；纯只读双端清单检查不能直接调用它。配置语法通过、native枚举成功、health成功也不能替代 PCM/电气/机械验收。

## 可离线补齐的最小完整交付

建议 Root 选择一个明确的“已有配置静态检查”入口，复用配置类型和纯验证函数，不进入 key helpers、Builder.build、数据库、模块工厂、模型或设备构造。输入必须显式存在且为可读取配置文件；缺失应报告 missing，不能将默认配置伪装为已审核文件。报告可限于模块选择、会打开的 I/O 类别、路径来源和已验证/未知前置，不输出完整配置或凭据。Server 端需要只读解析/合并时，不能触发 publication recovery；存在恢复记录时明确待运行时恢复，不替它执行。

无设备启动模板应明确这是另一个**会写盘和监听 loopback** 的验证步骤：所有七个 `[modules]` 槽位设 noop，motion.enabled=false / hardware=false / calibration=false，vision / vision_debug / hmmd disabled，discovery no-op 且 mDNS/BLE禁用，dev.skip_binding=false；固定独立目录和 loopback `:0`。无需真实 wake/VAD 模型、camera、ALSA、I²C/UART或绑定。它只能证明本地配置和生命周期，不报告真实设备可用。Root 已授权本分工准备该模板，交付如下；本分工没有执行启动。

最少验证建议：静态检查成功/失败都不创建 key/config/state/plugin目录、不chmod已有key、不调用 native 开放路径；missing config与旧字段真实拒绝；默认样例应明确列出真实采播/vision/discovery会启用，不能因为motionfalse而判为无设备；同名配置检查不修改普通运行默认行为。若实现无设备模板，复用现有mock/lifecycle测试验证无capture/playback/motion/camera/serial打开，真实socket/SQLite仅在隔离临时目录中由Root执行并收尾。

## 基线文档实际缺口（本轮已修订）

- 原 `local-development.md` 称开启 Embedded `[speech].enabled`，但当前 SpeechConfig 只有 channel_id / pre_roll_ms / upload_queue_frames，deny_unknown_fields 会拒绝该旧键；audio owner 已移除旧建议。
- 原 `local-development.md` 和 Embedded `configuration.md` 常用目录列 `run/audio-debug/`，旧自动 AudioDebugRecorder 已删除；audio owner 已移除该旧目录，并保留显式有界 evidence 的操作边界。
- 原 `server-embedded.md` 开头称需确定 Pi 系统，已有 Ubuntu24.04.4 实测；已更新为当前硬件资料并保留尚缺音频/执行器/部署输入。
- 原常规 cargo/便利任务的启动步骤缺少“即刻启动选定设备”的说明；已明确正常启动与只读检查的区别。

这些是已检查实现后确认的文档/入口缺口。本轮不新增系统服务、依赖、平台、真实音频采集、机械动作或自动购买/部署。Root 统一问题、实施分配、构建证据与提交，本分工继续只读复核最终交付。

## 已授权交付与静态验证

已编写 `config/embedded.staging.example.toml`，显式七个模块槽位为 noop；motion.enabled / hardware_enabled / calibration_confirmed、vision / vision_debug / hmmd、mDNS / BLE 均 false，approved_animations 为空；listen_addr 为 `127.0.0.1:0`。state / key / plugin / cue 各有不同的 `staging/` 相对路径，speech.channel_id 为 `pi-living-room`，dev.skip_binding=false、physical confirmation 保持。身份留空由未来正常运行按既有规则生成，不借用已有部署 key。头部说明复制到新私有目录，不把模板当真实 Pi 设备配置。

已更新 `server-embedded.md`：确认 Pi 系统资料、正常 startup 在 Builder.build 内已打开模块的副作用、通用样例与 staging 的不同用途、Root 正在实现的 `--check-config <path>` 只读入口、构建写入与运行检查的区别，以及前台 Ctrl+C / 精确 PID SIGTERM 的操作和清理证据边界。标明 pnpm 并发开发启动及视觉 setup 都不是只读/隔离验收。没有执行这些示例命令。

静态验证仅用 Python `tomllib` 解析模板并断言七槽 noop、所有设备/发现关闭、loopback:0、speech channel和四条不同相对路径，结果通过。不是 Rust 类型解析、不是运行测试；Root 将统一实际验证新 CLI 与配置。两份交付文件已通知 Root 停写；后续仅独立审阅 main/preflight 与 motion helper，按反馈修改自己的文档。

## 最终候选独立审阅

已读回 Root 的 `main.rs` / `preflight.rs` / lib export，以及 motion owner 的 `inspection.rs`、原 runtime 验证提取、原 plan 文件读取入口。`--check-config` 在创建 Tokio、日志、环境 KWS 检查、identity helpers 与 Builder 之前同步返回；显式文件不存在会失败，配置读取有 1 MiB 上限并在 Unix 拒绝 leaf symlink / special file，未借用普通 `load_config` 的 missing-default 行为。路径解析只应用原静态规则，检查 speech 合同和 motion 文件；报告只列选定字段，明确不验证模块实例、模型、设备权限、网络/数据库或 Server 清单。

motion 检查复用原 bounds / PCA 地址规则；校准文件使用 nofollow/nonblock 普通文件读取并限制 1 MiB，复用 NeckMapper 解析与输出数、center 范围验证。未确认且缺校准保持 `not_checked`，确认但缺文件失败。非空批准清单复用既有同 fd 资产 hash/bytes 验证及同一 buffer frame parser，没有生成新 pin、批准动作或构造 backend / worker。报告保持 `physical_state=not_observed` 和 PWM 参数仅静态检查，不声称实际机构校准或频率测量。普通 runtime 仍按原 hardware gate 执行 PCA 地址验证，未改变运行政策。

没有发现阻断当前离线范围的问题。已向 Root 建议集成见证：同时设置无效 KWS-WAV/普通配置环境变量，证明显式 CLI 先返回；成功、missing、格式错误均不产生 key/state/plugin/cue/配置目录，既有 key bytes/mode 与配置 bytes 不变。这些回归由 Root 编写和运行，本分工没有运行 Cargo 或启动二进制。操作文档已改为准确说明也读取配置指向的校准与已批准动作文件；两份交付继续停写，等待最终实际验证证据。

## 已完成的独立证据复核

最终新增 `tests/preflight.rs` 五项已完整读回。测试使用隔离临时目录和 guard 配置，若错误落入普通启动会在 identity 父路径处失败；成功检查同时设置另一普通配置路径与缺失 KWS-WAV 环境变量，确认显式 CLI 分支优先。覆盖既有 key 内容和 Unix 0640 权限、目录/文件快照不变，staging 全部 noop 与设备开关关闭；missing、旧 speech.enabled、语法错误、超长输入、空 channel、help/未知/不完整参数、symlink/directory/FIFO 均有相应断言。motion 五项覆盖无校准与确认门槛、合法矩阵不作物理确认、维度/center/PWM/PCA 静态错误、原资产 pin/frame parser 与特殊文件拒绝。测试不依靠真实设备或模型。

首次 `green-embedded.log/json` 为真实 exit 101，80.86 秒、源码无漂移：新 CLI fixture 的 `[identity]` 缺少必需字段，解析报 `missing field device_id`。Root 只补 fixture 中的 device_id / device_name 后重跑；保留原始失败，不把首轮尚未执行的后续目标计入完整通过。

最终 `final-embedded.json/log` 实际为 `cargo test -p alia-embedded --all-targets --offline --locked` exit 0，45.05 秒：lib **241/0/0**，integration **75/0/0**，合计 **316/0/0**；十四个目标结果行完整，新 motion 五项与 CLI 五项逐项通过。独立计算 `final-source-manifest.json` 原字节 SHA-256 为 `dd888cec2f84c06a213cadc236911d088c89015b144025284e8eab72070d3573`，665 个输入文件与清单逐个重算一致，`source_drift=[]`。

`cli-smoke.json` 另记录 Root 对通用示例与 staging 的显式 `--check-config` 两次成功运行，两个输入文件的前后 SHA-256 均相同。已读取两个 JSON 报告：通用示例仍如实列出 native 默认模块，staging 的七槽均 noop；两者 `hardware_verified=false`、`motion.physical_state=not_observed`。仓内已有矩阵解析成功仅记 `calibration.status=checked`，不改变 `calibration_confirmed=false`；复制至新目录缺少矩阵时的 not_checked 行为由隔离测试覆盖。smoke 只执行检查分支，没有运行普通 runtime 或 staging 服务。

现场资料新段落也已读回：保留同日历史盘点与本轮的区别，未将离线通过换算成 R02/R03 或 R05 完成；说明模块实例/feature/模型可启动、Server 清单、设备权限、AEC/时钟与物理保持仍未验证。当前没有独立复核发现阻断。本文已完成并停写，生产源码、测试和其它文档保持冻结；本分工没有执行 Cargo/Git/SSH/Docker、音频采集或硬件动作。

## Root最终接续记录

实现提交 `e409dc944a589104d0e2d2c2977f642ad8aaef28` 的665个blob已与最终输入核对一致。用户在分工停写后回复：R1仍未接入、AEC参考规格缺失，允许Mac模拟；R03机构资料均未定，后续先暂停；部署值待定，Pi可SSH。本轮交接以Root最新检查点为准，前文“待补回复”是当时审查状态。Mac只运行既有合成PCM示例，不启用真实音频、模型或执行器。
