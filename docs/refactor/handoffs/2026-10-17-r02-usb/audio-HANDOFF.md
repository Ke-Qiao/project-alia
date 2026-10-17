# R02 — R1 USB只读枚举

2026-10-17；基线`e4da8e1`。本分工4/4完成；只复核仓库检查方式及Root提供的现场原件，没有自行SSH、改代码、运行测试或打开音频设备。R03保持暂停。

## Checklist

- [x] 复用已有sysfs／`/proc/asound`与节点权限检查，限定为只读枚举。
- [x] 核对本次SSH结果、stdout及空stderr，区分设备身份与账户权限。
- [x] 未发现外部USB／USB音频候选后停止，不追加格式探测或软件整备。
- [x] 记录未验边界和唯一下一步，冻结交接。

## 本次实际结果

原件：[运行结果](evidence/pi-enumeration-result.json)、[stdout](evidence/pi-enumeration.stdout.txt)、[stderr](evidence/pi-enumeration.stderr.txt)。Root一次SSH退出0、耗时3.293秒；远端观察时间为2026-10-17 11:39:34 UTC。目标为`ke-qiao@192.168.10.221`，Pi 5 Model B Rev 1.0、Ubuntu 24.04.4 LTS、kernel 6.8.0-1065-raspi、aarch64。

- `lsusb`与sysfs均只列5个Linux root hub，未枚举外部USB设备，**没有R1或其他USB音频候选**。
- `/proc/asound/cards`仅有`vc4hdmi0`和`vc4hdmi1`；`/proc/asound/pcm`各列一个playback，无capture。sysfs路径均属于HDMI。
- 当前用户组不含`audio`。6个已有音频字符节点均为`root:audio`、0660，本账户逐个`test -r/-w`结果均为no；这是本次访问检查，不是通过打开PCM验证。
- 没有USB候选，因此未读取USB `stream*`或尝试协商格式。R1的VID/PID、序列号、固件、声道、原生采样率与样本格式均仍未知。

## 边界与下一步

枚举只能证明观察时内核没有登记R1，不能判断用户是否已经物理插入、线材状态或未枚举原因；不将此结果报告为R1驱动故障。没有更改用户组、ACL、udev或设备权限，也未运行`arecord/aplay`、音频health、Embedded服务或录放音。

下一步仅需用户将R1通过USB接入指定Pi后，再做同一轮只读枚举；出现明确USB音频候选后才读取对应`/proc/asound/cardN/stream*`的内核声明格式及该节点权限。声明格式不等于实际PCM可用，更不等于AEC已启用或声学验收。当前不追加软件框架、设备诊断扫描或录放音；硬件AEC参考与功放接线仍需其原有资料和授权窗口。
