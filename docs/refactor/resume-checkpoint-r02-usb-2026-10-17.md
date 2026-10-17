# 恢复检查点：R02 R1 USB 枚举

2026-10-17；分支 `codex/architecture-refactor`，基线 `e4da8e1`。**只读核对 4/4；Pi 可达，R1 当前未被系统枚举。** [Checklist](checklists/r02-usb-2026-10-17.md)、[四份 HANDOFF](handoffs/2026-10-17-r02-usb/README.md)。

一次 SSH 使用既有配置身份和严格 known_hosts，exit 0，耗时 3.293 秒。目标 `ke-qiao@192.168.10.221`，主机 `KeQiaopi`，Raspberry Pi 5 Model B Rev 1.0、Ubuntu 24.04.4 LTS、aarch64、kernel 6.8.0-1065-raspi。Pi 记录时间 `2026-10-17T11:39:34Z`；本地和 Pi 时间分别记录，未验证时钟同步。

USB 清单和 sysfs 均只有 5 个 root hub；ALSA 只有 2 个 HDMI playback，没有 USB 音频卡或 capture 端点。当前用户不在 audio 组，现有 `/dev/snd` 字符节点均为 root:audio／0660，当前账户 read/write 均为 no。未改权限、驱动或服务；这不是 R1 节点权限的测量，也不证明实物是否已插入。

## 下一步

1. 用户将 R1 通过 USB 接入 Pi 后，再做一次同范围枚举；若用户确认已经接入，则针对 USB 识别做必要定位，不重复软件整备。
2. 出现候选后核对设备身份和对应 `/proc/asound/cardN/stream*` 声明的格式／声道／采样率及实际节点权限，不打开 PCM。不要把 USB 描述符或同机连接当作 AEC 生效证据。
3. 真正采集／播放另行确定范围：固定设备、无插件／motion 禁用配置、必要的账户权限、停止清理和录音窗口。播放／AEC 参考接线仍待扬声器／功放及厂商电气资料；这些不阻挡 USB 枚举。

本轮只更新记录，没有应用源码、依赖、构建／测试或远端配置变更；没有启动服务、录放音或执行器，SSH 会话已退出。默认 shadow／政策／执行批准保持，R03 暂停。PN01 已完成，PN02—PN04 与完整模型链不作为首个无插件音频接入前置；R01—R06 仍 2/6。
