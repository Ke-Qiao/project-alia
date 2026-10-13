# Root — 硬件确认与审计收口

日期：2026-10-14。分支 `codex/architecture-refactor`，本包起点 `31314b03e3629217bfdea3615a415029e4dd7c20`，上次已测试源码 `cffbca531cec2478c17b5bcacfd191e7f00a56f9`。本包只修改文档与归档盘点证据，没有重新运行 Cargo 或把历史测试当本轮测试。

## 交付和证据

用户硬件选择、校准后启动归中与停止/断连保持规则已回填 [部署资料](../../../operations/hardware-deployment-readiness.md)。三个 subagent 分别完成官方音频资料、Motion 静态审计和部署整理/交叉审阅；各自交接已停写。Root 完成严格既有 host key 的 Pi 只读 SSH 盘点，失败与成功均归档；没有待恢复的测试、数据库或远端任务。

- 准备/审计 [checklist](../../checklists/hardware-confirmation-2026-10-14.md) 6/6；音频资料 5/5、静态动作审计 8/8。计数描述不同子集，不相加。
- 9 份原始文件的字节/哈希见 [证据索引](evidence/evidence-index.json)。上轮 source manifest 的 656 项逐项一致，无源码漂移；Git 变更仅在 docs。
- 新实现/新 Cargo 测试/实际音频或动作验收均为 0。R01—R06 仍 2/6，L09—L12 4/4；不推算项目完成率。

原始 inventory 的 `no device open` 标签不是系统调用级证明：ALSA 列举可尝试访问 control 节点；准确范围是未打开 PCM 录放音、未 I²C/串口操作。没有授予设备权限、安装依赖、部署服务或调整政策。

## 下一轮可直接开始

先完成 **R03-0**，使用三个 subagent：

1. Motion runtime/writer 与配置：停止后固定保持，清理能重新启用 stop-reset 的旧路径；启动只在明确完成校准后允许归中。把 serde/default/示例/调用者/测试一起收敛，不把 `previous=center` 当真实姿态。保持当前 hardware_disabled 默认。
2. PCA 驱动：先用生产路径寄存器模型复现 AI 缺失与 SLEEP 后取消/失败，再修复初始化与有界收尾。保持单 native owner、逐通道取消、短写不重放；恢复失败报告未知，不伪造已保持。
3. 回归/独立审查：覆盖进入 write 后 stop、丢弃 waiter 后再 join、重复 stop，以及 lease Close/到期、binding revoke 与 observer drop 的区别；协调 Linux 有关路径测试，不新增依赖或真实动作。

Root 负责接口取舍、组合验证、checklist/HANDOFF/恢复检查点和正常提交。**R03-0 尚未实现**，当前不能启用未校准 hardware 来代替测试。具体缺口、源符号、最少复现见 [Motion HANDOFF](motion-HANDOFF.md)。

**R02-A** 的双流有界证据格式、合成 PCM 离线重放可独立并行，不必等待 R1 接入；沿用已有单 consumer reference、缺口/时钟与关闭所有权，callback 不写文件。实际录音须有明确采集窗口与保留范围。官方 R1 独立功放输出参考资料及扬声器条件性方案见 [Audio HANDOFF](audio-HANDOFF.md)，不添加未经验证的第二套 AEC。

**R03-A** 受信动作资产 bytes 绑定在 R03-0 后推进；不以模型自报 hash 或设备当前内容作信任根。原 R02/R03 出口满足后进入 R05；开环硬件不免除原位置/保持的物理证据要求。Settings 剩余等待/取消可用性风险继续放在 R05 前故障验证，先复现再补缺口，不再次自动扩成全面 R04。

## 未完成与资源状态

R1/扬声器/功放未形成实际声学链；服务账户音频权限未处理；启动校准和停止保持未实现，PCA AI/SLEEP 缺口未修；资产内容认证、物理反馈/保持、模型部署和24小时负载未完成。没有修改真实配置、绑定或硬件寄存器，没有使用 reset 卡。前端、额外平台、通用 Embedded 和未来同机/端上模型等继续按 [延期台账](../../deferred.md) 保存。
