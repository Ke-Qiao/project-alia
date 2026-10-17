# R02 R1 USB 只读枚举独立审查

2026-10-17；基线 `e4da8e1bdd83b19a5d37990a8074bc3fabb71b7c`。**4/4，审查 GO；本次未检出可识别的 R1，尚未通过 R1 接入验收。** 本 agent 只读本地原件并写本文件，未执行 SSH、测试、服务或硬件操作。按 `iot-workflow` 区分观测与实机能力；R03 保持暂停。

## Checklist

- [x] 核对 SSH 目标、严格主机信任及单次有界执行结果。
- [x] 核对远端命令仅身份／USB／ALSA 元数据与权限查询，没有录放音、服务启动、配置或权限修改。
- [x] 核对 stdout／stderr SHA-256 与结果 JSON 一致，并校准结论边界。
- [x] 明确仅待接入后重枚举，不重复软件整备或扩大硬件动作。

## 证据与结论

原件为本目录 `evidence/pi-enumeration-result.json`、`pi-enumeration.stdout.txt` 与 `pi-enumeration.stderr.txt`。单次 SSH 使用 `BatchMode=yes`、`StrictHostKeyChecking=yes` 等限制，返回 **exit 0，3.293 秒，未超时**；stderr 为空。stdout／stderr 摘要分别为 `ee79ff29ad8fcc6cff29d5ad2d4fb03fdb991a3b55a5432bd4b1d859eb86e5e4`／`e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`，已独立重算一致。Mac 执行时标和 Pi 观测时标各自记录，不据此推断时钟同步或远端启动耗时。

目标返回 `KeQiaopi`、Raspberry Pi 5 Model B Rev 1.0、Ubuntu 24.04.4 LTS、Linux 6.8.0-1065-raspi／aarch64，SSH 用户为 `ke-qiao`。`lsusb` 与 sysfs 只列出 5 个 Linux USB root hub；ALSA 只列出两个 HDMI playback，无 capture。因此准确结论是：**本次系统枚举没有显示外部 USB 设备或可识别的 R1 音频输入**。这不证明设备物理未插入，也不定位线材、供电或设备故障。

当前 `/dev/snd` 现有节点的 shell 可读／可写检查均为 no，用户组未含 audio；本轮未改变权限或组。该观测仅适用于当前节点，不能预判尚未枚举的 R1 节点权限，也不应据此自动执行 chmod、usermod、udev 或驱动安装。

**下一步仅需用户接入／确认 R1 USB 后，在相同范围重枚举。** 暂不录放音，不连接未经确认的 AEC 功放参考，不启动执行器；无须为本次“未检出”重复 Cargo、模型、插件隔离或整套发布验证。R1 的 PCM 格式、采集权限、录音效果、AEC 与自然全双工均尚未由本次观测验证。
