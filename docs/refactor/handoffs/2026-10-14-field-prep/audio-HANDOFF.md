# R02 现场前置音频审查 — HANDOFF

日期：2026-10-14；Owner：`settings_auth_reproduce`；基线 `283f38f`。本轮目标为 Mac Server + Pi 5 / 8 GB Embedded 的现场前置准备。沿用 IoT workflow，先检查已有实现再补缺口，不新增依赖、不泛化音频后端、不重复 R02-A 证据工具。

## Checklist 与当前进度

- [x] A01 读取部署资料、R1 官方资料交接、R02-A native/evidence 交接与当前证据契约。
- [x] A02 追踪实际配置、模块启动、设备选择、格式协商、health 与离线示例。
- [x] A03 向 Root 汇报真实缺口、最小交付建议与集中待确认信息。
- [x] A04 按 Root 分配修正文档／准备材料，不修改未分配文件。
- [x] A05 核对交付材料与当前源码一致，记录实际离线验证及限制。
- [x] A06 分工交付完成并交由 Root 统一合并 checklist、恢复入口及提交；全包状态以 Root 为准。

当前 **6/6（本分工完成）**。Root 授权的三份文档已完成并停写，最终离线证据已独立读回；全包提交及恢复入口以 Root 交接为准，不预写提交 hash。没有修改生产、测试或配置，未运行 Cargo/Git/SSH/Docker/音频或任何设备调用。

## 已完成能力，保持复用

`module/audio_evidence.rs` 已具备显式 `EvidenceSession::start/pump/finish`、独占 reader、有界 JSON export/read 和 replay。native capture 与最终 playback reference 同队列记录，保留独立时钟、gap/loss/marker 和 latest footer。无需新 recorder、默认文件路径、后台任务或第二个 consumer。`examples/audio_evidence_replay.rs` 创建 SyntheticFixture、写新文件并读回，拒绝覆盖；它不是读取任意现场文件的 CLI，也不是打开设备采集的 CLI。实际现场会话尚无自动启动入口，应在取得后续录放音授权后才决定受控调用方式。

旧 `[audio_debug]`、源 WAV recorder 与旧 voice modes 已删除，当前配置采用严格未知字段拒绝。通用 mixer 已按 voice 1.0 / cue 0.2 混合并记录 native 边界；没有生产软件 AEC consumer。R1 的硬件 AEC 是否开启、参考是否接入以及声学质量不能从这些软件能力推导。

## 本轮初始审查查见的缺口与边界

1. **通用配置不是无设备预检配置。** `config/embedded.example.toml` 的 `[modules]` 留空后使用 native audio 默认值，`[audio]` 为 `auto` + `default`。`EmbeddedModuleRegistry::start` 在语音 wake 之前已经启动 playback，并调用 `audio_capture.start_capture`；不能以“还没说唤醒词”当作麦克风尚未打开。示例同时启用 vision、BLE 等其他能力，不宜拿通用 `cargo run -p alia-embedded` 当本轮现场准备检查。
2. **音频 health 不是只读设备名枚举。** Linux ALSA `is_available` 会 `AlsaPcm::new` 打开 capture/playback PCM。现场未授权打开设备时，不运行 Embedded 来获取音频 health，不以 `arecord --dump-hw-params`、`aplay`、ALSA test stream 代替枚举。Server 的默认 `/health?probe=false` 语义与这个 Embedded native 检查不能混称。
3. **设备选择必须固定实际身份。** CPAL `find_device` 当前是 `contains` 后取第一个匹配，`auto/default` 也可能在 CPAL 失败后转 ALSA；它们不证明选中 R1。Pi 现场应准备明确 `capture_backend="alsa"` / `playback_backend="alsa"` 与已核验的 `CARD` / `DEV` 名称，不能填未经核对的 `0,0`、通用 `default` 或模糊 `R1` 名称。此轮不泛化重构 CPAL 匹配；若将来真正使用 CPAL 多设备选择，再单独解决精确唯一选择。
4. **配置格式与原生格式不同。** `capture_sample_rate=16000` / mono 是送往 speech 的格式；CPAL 可能使用可用其他 rate/channels，ALSA 使用 `set_*_near` 后转换。现场证据须分别记录配置请求、实际协商结果、USB 描述和固件通道语义，不能按配置值填“原生 16 kHz mono”。R1 经 USB 接 Pi 不证明 USB playback endpoint，也不证明采播共时钟；保持 `alsa_playback_lock_to_capture_rate=false`，除非有另行确认的共享时钟设计。
5. **有失效操作说明。** `docs/operations/local-development.md` 仍要求启用 `[speech].enabled`，但当前 `SpeechConfig` 只有 `channel_id/pre_roll_ms/upload_queue_frames` 且 deny_unknown_fields，该说明会使配置拒绝。该文与 `packages/alia-embedded/docs/configuration.md` 仍列 `run/audio-debug/` 为常见默认目录，已不存在此自动录音目录，应删除或改为显式调用者选择的证据路径，不能恢复旧开关。
6. **实际现场资料尚未落到配置身份。** 已知 R1 尚未接入、将以 USB 接 Pi；上轮 USB 仅 root hubs、现有 `/dev/snd` 权限不足并非 R1 驱动失败。无 R1 VID/PID/serial/USB path、实际 ALSA card/device、固件和参考输入电气数据，不能给出可直接启用的真实设备配置。后续只读读取 `/proc/asound` 与 sysfs/USB 身份可以补充描述能力；这仍不同于打开 PCM 协商或做录放音。

## 审查后确定的最小交付

- 修正上述失效文档，并在通用启动入口明确标识它会打开已配置设备。
- 复用当前模块选择，准备独立目录、全设备禁用的 staging 配置或覆盖清单；音频使用 `modules.audio_capture="noop"`、`audio_playback="noop"`，wake/VAD 也明确禁用以避免模型依赖。完整设备禁用范围由 Root 与部署 owner 统一，不仅关闭音频。
- 给出 R1/Pi 现场音频记录表和 **未激活** 的后续 ALSA 字段模板，实际 `CARD/DEV` 留待枚举证据填入，默认不跑、不绑定、不录音。区分只读身份、权限、请求格式、实际 native 格式、固件处理输出、功放参考路由，以及后续有授权的声学试验。
- 使用已有 synthetic example 作为离线格式演练，明确不证明硬件；不重写 session/export/replay，不新增音频依赖。运行由 Root 在统一验证窗口决定。

## 已完成的限定文档编辑

Root 分配后，仅修改 `docs/operations/local-development.md`、`packages/alia-embedded/docs/audio-and-voice.md`、`packages/alia-embedded/docs/configuration.md`：

- 删除启用不存在 `speech.enabled` 的建议，并说明当前真实 voice/module 配置；删除 `run/audio-debug/` 自动目录建议。
- 明确一般零参数启动会在 wake 前打开 native 音频，并可能初始化其他设备/identity/state/listener；staging 运行仍是有状态服务，不能称纯只读检查。
- 对齐 Root 新入口 `cargo run -p alia-embedded -- --check-config /absolute/path/embedded.toml`：应用检查不打开设备、不加载推理模型、不创建身份/状态、不监听，missing/unknown/incomplete 不回落普通启动；Cargo 本身仍可能编译构建目录。实际检查范围和结果待 Root 最终实证，不将成功检查称硬件 ready。
- 链接 reviewer 负责的 `config/embedded.staging.example.toml` 和 Root 负责的集中 `hardware-deployment-readiness.md`；不另建重复现场表。
- 说明 Pi 明确 ALSA backend 与按 USB 身份核实的 CARD/DEV，不能靠默认/数字序号/CPAL 模糊首匹配证明所选设备；区分请求格式、协商 native 格式和 firmware 语义，保持 USB R1 不假定共享时钟或 USB 播放参考。
- 明确 R02-A example 是合成文件生成/读取，不是现场录音或任意既有文件的 CLI；真实会话由显式 library caller 拥有，保留/删除规则集中记录。

本 agent 在 staging 与 Root CLI 落地后重新完成标准库只读相对链接检查，三份文档均无缺失目标。静态核对 staging 七个模块 noop、设备/发现关闭、loopback 临时端口与包文档一致；`main` 在创建 Tokio runtime 前处理 check/help/error，新 `preflight` 不构造 identity/state/node/model/device，报告明确 module/model/permissions/Server catalog/physical 均未检查。文档未把配置通过称硬件 ready，也将 Cargo 构建目录写入与应用只读区分。

Root 的首次新 CLI 回归因 fixture 缺少 identity 必填字段失败；`green-embedded.log` 保留 `missing field device_id` 和 preflight 4 passed / 1 failed。Root 修正 fixture 后完整重跑，未据此修改生产或弱化只读语义，也没有覆盖首轮失败证据。

## 最终离线验证读回

本 agent 只读核对 `run/validation-20261014-field-prep/` 中下列记录，没有另行运行命令。最终 665 输入 manifest SHA-256：`dd888cec2f84c06a213cadc236911d088c89015b144025284e8eab72070d3573`；两轮最终测试均 `source_drift=[]`。

- `final-embedded.log/json`：默认 features `--all-targets --offline --locked`，退出 0，45.05 秒；lib 241 + integration 75 = **316 passed / 0 failed / 0 ignored / 0 filtered**，包含 5 项新 CLI 测试和原音频 owner/cleanup 回归。
- `final-no-default-cli.log/json`：`--no-default-features --test preflight --offline --locked`，退出 0，17.77 秒，**5 passed / 0 failed / 0 ignored / 0 filtered**。该结果证明配置检查不依赖默认推理 feature；没有运行音频后端。
- `cli-smoke.json`：实际已构建二进制对通用和 staging 两份示例运行 `--check-config`，均退出 0，输入 SHA-256 前后相同；生成 `example-report.json` 与 `staging-report.json`。两份报告均 `mode=offline_configuration_check`、`hardware_verified=false`；前者真实报告 native 模块配置，后者报告七个 noop，均未启动普通 runtime 或 staging 服务。

三份文档链接及入口语义与最终源码一致；源码、配置、测试与包文档继续冻结。本轮没有 Linux 应用/ALSA、R1、真实录放音、AEC、部署绑定或执行器现场证据，不把上述离线结果转记为 R02/R03 硬件验收完成。

## 仅需 Root 集中确认的现场信息

- R1 当前是否已经接入（上次明确未接入）、实物板版本／固件／附带线束；不再询问已确认的 Pi 主机和 USB 连接选择。
- 扬声器与功放的拟用型号，以及 R1 厂商对功放输出参考端电气范围、BTL/Class-D 兼容性与接线的确认。现有官方交接已明确 UAC capture 与外部功放参考为不同路径；本轮不重搜相似六麦产品或猜测 line-in 接法。
- Pi 服务运行账户／预期 `/dev/snd` 访问方式，后续真实音频采集的受控保存目录、保留时长、删除责任及授权时点。软件预算 512 records / 16 MiB 不能替代用户的真实音频保留规则。

设备未接入、输出链未选型或电气资料不足不阻断文档与无设备配置准备；依赖这些信息的真实 PCM 打开、录放音、参考接线与 AEC 验收继续保留未完成。本 agent 没有执行这些动作。

## Root最终接续记录

实现提交 `e409dc944a589104d0e2d2c2977f642ad8aaef28` 的665个blob已与最终输入核对一致。用户在分工停写后回复：R1仍未接入、AEC参考规格缺失，允许Mac模拟；R03机构资料均未定，后续先暂停；部署值待定，Pi可SSH。本轮交接以Root最新检查点为准，前文“待补回复”是当时审查状态。Mac只运行既有合成PCM示例，不启用真实音频、模型或执行器。
