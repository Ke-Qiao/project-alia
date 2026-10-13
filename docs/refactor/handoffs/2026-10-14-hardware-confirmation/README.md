# 2026-10-14 硬件确认与 R02/R03 接续

本包接续已完成的 Settings 有界审计。用户确认当前 Mac、Pi5/8 GB、R1 USB、PCA9685A/MG996R、停止/断连保持，以及完成机构校准后允许启动归中。它是资料、只读盘点与静态缺口确认包，**不是 R03-0 修复或硬件验收包**。

四份 HANDOFF：

1. [Root](root-HANDOFF.md)：汇总、证据边界、已确认规则与下一轮分工。
2. [Deployment](deployment-HANDOFF.md)：用户输入、第三分工审查与 Root 实际 Pi 盘点。
3. [Audio](audio-HANDOFF.md)：官方 R1 资料、功放参考、条件性选型及接入后枚举。
4. [Motion](motion-HANDOFF.md)：停止/启动、PCA AI/SLEEP、取消与反馈审计。

入口：[checklist](../../checklists/hardware-confirmation-2026-10-14.md)、[恢复检查点](../../resume-checkpoint-hardware-confirmation-2026-10-14.md)、[部署资料](../../../operations/hardware-deployment-readiness.md)。本包准备与审计 6/6，R01—R06 仍 2/6，不换算项目完成百分比。

[证据索引](evidence/evidence-index.json)保存 9 份原始文件的大小和 SHA-256。首次绕过 SSH config 的失败也保留；`pi-inventory.stdout.json` 是该失败的空 stdout，不能作为 JSON 解析。配置身份重试成功，不曾绕过 host key 验证。时间戳保留 UTC，对应上海 2026-10-14。

主 inventory 脚本和结果按原字节保存；脚本中的 `no device open` 标签仅表示没有 PCM 录放音或 I²C/串口操作，ALSA 列举仍可能尝试访问控制节点。补充 stat/access 与 rustup 结果没有独立时间戳，不伪造时间或完整调用脚本。这里的脚本是审计材料，不是部署工具。
