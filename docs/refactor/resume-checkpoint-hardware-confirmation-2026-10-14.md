# 当前检查点：2026-10-14 硬件确认 → R03-0 / R02-A

分支 `codex/architecture-refactor`。本包起点 `31314b0`，源码仍为上轮已测试的 `cffbca5`；本包仅 docs/证据。准备与审计 **6/6**，R01—R06 **2/6**，L09—L12 **4/4**。不把静态审计、资料或历史回归计为本轮修复/实机通过。

先读 [checklist](checklists/hardware-confirmation-2026-10-14.md)、[四份 HANDOFF](handoffs/2026-10-14-hardware-confirmation/README.md)、[部署资料](../operations/hardware-deployment-readiness.md) 与 [证据索引](handoffs/2026-10-14-hardware-confirmation/evidence/evidence-index.json)，再核对 branch/status/log。三个 subagent 已交回，没有待恢复测试或远端任务。

## 已确认，不重复询问

- 当前开发 Mac 运行 Server；Pi5/8 GB 为 Embedded，地址 `ke-qiao@192.168.10.221`，Ubuntu24.04.4 LTS，5 V/5 A DC。模型服务后续确认，散热未提供。
- Root 严格既有 host key 的只读 SSH 已成功；aarch64/kernel6.8.0-1065-raspi/7.8 GiB，与用户资料一致。当前账户没有现有音频节点权限。Rust 已安装但不在非交互 PATH，不是未安装；protoc 可用性待固定。
- R1 尚未接入，后续 USB 接 Pi；扬声器/功放待选。官方资料指向独立的功放输出 AEC 参考，USB 采集不证明参考已接入。电气兼容性、固件/PCM 与实际声学效果待测。
- PCA9685A + MG996R；停止/断连保持；**完成机构校准后允许启动归中，校准前不自动归中**。目前没有校准资料；5 V/5 A 仅为用户说明的 Pi 供电，不据此推定舵机供电可用。幻尔方案未来再定具体型号。

## 第一条可执行任务

继续使用三个 subagent，先 **R03-0**：配置/启动/停止契约、PCA 寄存器初始化、回归/独立审查三个分工，Root 组合验证及提交。先用 mock/寄存器模型复现，再修复以下已定位缺口：

1. 当前停止默认 `reset_on_stop=true` 会新增归中运动；清理旧配置可重新启用此行为的路径，保持精确取消和 native join。
2. 当前启动无条件 reset，`soft_start_enabled=false` 也会写中心；落实校准门槛，不以软件 previous=center 伪装真实初始位置或物理限速证据。
3. PCA 连续写四个 PWM 寄存器，但初始化未开 AI；以实际生产 initialize+PWM 寄存器模型验证。
4. SLEEP 成功后取消或 prescale 失败会跳过恢复；原 owner 完成必要且有界的收尾，保留原错/恢复失败与未知结果，不能盲重放目标。

这些 **尚未修复**。不启用 native 硬件来替代回归，不添加依赖/平台。测试区分 stop、binding revoke、lease Close/到期、observer drop；单观察者掉线不获得全局撤权，授权期限不当物理停止上界。详见 [Motion](handoffs/2026-10-14-hardware-confirmation/motion-HANDOFF.md)。

随后推进 **R03-A 可信资产内容身份**；**R02-A 有界双流证据与合成 PCM 离线重放可并行**，不等待 R1 到位。保持单 reference consumer、callback 不做文件 I/O；实际音频采集时再固定采集/保留范围。现有硬件 AEC 先核对参考链，不直接叠加另一 AEC。

R02/R03 现场出口满足后进入 R05。PCA 开环不免除物理位置/保持证据，可由外部测量或后续反馈硬件提供；不伪造 MCU ACK。模型、供电/散热、音频链、校准和长稳仍待落实。Settings 剩余慢锁/handler 取消风险留在 R05 前故障验证，不自动重启全面 R04。

## 本包验证边界

9 份原始证据保留首次 SSH 认证失败和配置身份成功；656 源码输入与上次 manifest 一致。仅本地文档链接/差异/证据哈希校验，没有新 Cargo 或 Linux 构建、录放音、I²C/串口操作、部署安装或负载验收；历史测试不重算。未使用 reset 卡。前端、额外平台、通用 Embedded、端侧模型和同机组合继续按 [延期台账](deferred.md) 保留。
