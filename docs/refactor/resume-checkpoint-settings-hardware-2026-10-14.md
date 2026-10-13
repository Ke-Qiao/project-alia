# 当前检查点：2026-10-14 Settings 有界审计 → R02/R03

分支 `codex/architecture-refactor`，测试源码 `cffbca531cec2478c17b5bcacfd191e7f00a56f9`。本轮审计与准备 **8/8**；H01—H05 的实际部署确认仍开放，R01—R06 **2/6**、L09—L12 **4/4**。这些不是项目完成百分比。

先读 [本轮checklist](checklists/settings-hardware-2026-10-14.md)、[四份HANDOFF](handoffs/2026-10-14-settings-hardware/README.md)、[硬件部署清单](../operations/hardware-deployment-readiness.md) 和 [当前验证摘要](handoffs/2026-10-14-settings-hardware/evidence/validation-summary.json)，再核对 branch/status/log。三个subagent已交回，无待恢复测试/数据库作业。

Settings审计完成，生产未改。新增两项Registry取消测试证明取消排队者不占gate、abort/join持锁任务释放gate，但保留已完成desired并需显式保存。现有SQL/最后身份各2秒不代表整个请求有界。LLM跨网络配置读锁、TTS音频额度等待、同步文件调用及真正HTTP取消的恢复仍列为R05前验证，不自动开启新的全面R04重构。

当前macOS ARM64验证：Settings原回归 **5/0/0**，新增取消 **2/0/0**，存储 **7/0/0**，Embedded默认lib **177/0/0**；656输入无漂移并与测试提交一致，格式/边界检查通过。没有本轮Linux、全workspace、release/模型或实机结论。

## 下一工作顺序

1. 接收用户对H01—H05的答复，先回填实际目标与资料路径；明确可信Pi目标后才进行只读盘点，不猜历史地址或绕过host-key。缺信息不阻塞下面不依赖硬件的设计/离线工作。
2. **R02-A：有界双流证据导出与离线重放。** 复用单consumer reference bus、capture timing和原有连续性；以合成PCM验证缺口/close/重开，callback不做文件I/O，实际录音显式启用。详细修改面和验收见review-HANDOFF，本包尚未实施。
3. **R03-A：签名Animation绑定受信资产bytes。** 先固定可信资产清单/摘要来源与版本拒绝，再同步core/server/embedded；同次读取校验与解析，拒绝同名替换/未知摘要/旧未绑定命令。不得把LLM自报hash或设备当前内容当授权，保留全部准入/取消/资源保证；本包尚未实施。
4. H02/H03明确后选定AEC消费、分帧、时钟/漂移方案并实测；H04明确后接入真实MCU boot/ACK/位置反馈和停止策略。无反馈能力就保留unknown。
5. R02/R03达到各自现场出口后进入 **R05**：固定部署身份、干净安装/配对/升级恢复、管理故障恢复和至少24小时混合负载。现场阈值与部署资料同时固定，不用历史容器或模型标签替代。

Web/iOS/Android、通用Embedded、端侧Easy-Turn、同机/同进程产品化、额外Server平台及V01继续按 [延期台账](deferred.md) 保留。未使用reset卡，未执行硬件或部署动作。
