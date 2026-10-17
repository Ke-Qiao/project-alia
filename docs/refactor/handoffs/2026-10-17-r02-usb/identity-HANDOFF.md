# R02 R1 USB 身份核对 HANDOFF

2026-10-17；负责人 `v01_fixture`；本轮基线 `e4da8e1bdd83b19a5d37990a8074bc3fabb71b7c`。本 agent 只读既有资料和 Root 的本轮原件，仅写本文件；未发起 SSH、测试、服务或硬件操作。R03 继续暂停。

## Checklist：3/3

- [x] 核对硬件台账、R1 AEC 前置记录及最新 PN01 检查点，复用已有软件准备。
- [x] 核对本轮 SSH 目标、主机身份、USB／ALSA／节点权限原件及 stdout／stderr SHA-256。
- [x] 分开记录已观察事实与未知项，不将成功连接或枚举视为采集／AEC 通过；本文件冻结。

## 本轮已观察

Root 单次连接既定目标 `ke-qiao@192.168.10.221`，沿用既有 SSH 身份，`StrictHostKeyChecking=yes`、`UpdateHostKeys=no`，未接受新 host key；exit 0、耗时 3.293 秒、无超时，stderr 为空。主机为 `KeQiaopi`，Raspberry Pi 5 Model B Rev 1.0、Ubuntu 24.04.4 LTS、Linux 6.8.0-1065-raspi／aarch64。

| 边界 | 原件事实 | 结论限制 |
| --- | --- | --- |
| USB | `lsusb` 与 sysfs 均仅 5 个 Linux root hub | 本次没有外部 USB／R1 枚举；不能断言实物未插、线缆故障、驱动故障或供电故障中的某一种原因 |
| ALSA | `/proc/asound/cards` 仅 vc4hdmi0／vc4hdmi1；`pcm` 仅两项 playback，无 capture | 无 R1 采集端点可核对，稳定 USB 身份、原生格式、固件／处理声道语义仍未知 |
| 权限 | `ke-qiao` 无 audio 组；现有 `/dev/snd` 节点 `root:audio`、0660，账户 readable／writable 均 no | 当前节点访问条件不足；这不解释 USB／sysfs 没有 R1，也不证明未来 R1 节点权限必然相同 |

Mac 记录开始时标为 `2026-10-17T11:39:29.572061+00:00`，Pi 输出观察时标为 `2026-10-17T11:39:34Z`。二者来自各自主机，未检验时钟同步，不能据相减推导同步偏差或操作延迟。

2026-10-14 历史枚举同样只有 root hubs／HDMI，当时用户明确 R1 未接入；本轮是新的只读快照，不将历史“未接入”擅自当作此次实物状态。后续先核对实物 USB 数据连接，再在同一可信目标读取新出现设备的身份与描述符；当前没有理由重复软件离线回归。

硬件 AEC 优先路线仍保持。即使后续 USB／PCM 枚举成功，也不证明录音成功、持续处理音频语义、AEC 开启或参考信号正确接入。扬声器／功放与参考电气资料仍待确认；本轮未打开 PCM／control、录放音、修改权限或启用执行器。

## 原件

- [连接结果与命令边界](evidence/pi-enumeration-result.json)
- [标准输出](evidence/pi-enumeration.stdout.txt)：SHA-256 `ee79ff29ad8fcc6cff29d5ad2d4fb03fdb991a3b55a5432bd4b1d859eb86e5e4`
- [标准错误](evidence/pi-enumeration.stderr.txt)：空文件，SHA-256 `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`

以上是身份与枚举核对完成 3/3，不是 R02 声学验收完成；没有新增采集许可、权限变更或硬件动作授权。
