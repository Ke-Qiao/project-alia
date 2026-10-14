# 当前恢复检查点：R02 时钟、缺口与 AEC consumer 契约

日期2026-10-14；分支`codex/architecture-refactor`；本轮基线`38fb689`，实现提交`28920fb92e1785f0e8880309f1b4c7b1193905fa`。本包 **10/10完成并正常提交**；文档/证据提交紧随实现提交。R01—R06仍 **2/6**，R03后续继续按用户要求暂停。

先读[checklist](checklists/r02-clock-2026-10-14.md)、[四份HANDOFF](handoffs/2026-10-14-r02-clock/README.md)、[时钟/consumer契约](audio-reference-clock-design.md)和[硬件资料](../operations/hardware-deployment-readiness.md)，核对branch/status/log。三个subagent已完成clock6/6、reference8/8、review7/7并停写；后续继续三名分工与Root统一验证/提交。已完成的CLI、R02-A/Mac组合模拟及R03软件不重做。

## 本轮交付与契约

- 采播独立native时钟/游标、converted域、后端估计与主机观察时间分别定义；名义速率不等于实际漂移或对齐证明。
- sample或callback锚点严格回退时分段，整批Unknown且保留真实PCM/游标。采集/播放共用私有分块发布，每native batch只observe一次，分块nominal offset不污染下一真实锚点。
- 历史证据仍可读取，replay新增ClockDiscontinuity；同stream相邻实际PCM的quality/可用性/时间变化明确提示，marker、queued delay、host time和其他流不误当同一时钟。
- 当前没有AEC processor/feature/自动consumer。1024样本参考块、speech上传块、VAD窗口与rubato内部块各自有用途；漂移映射公式与consumer帧需求只是接入条件，未实现或输出猜测估计。claim reader不会自动改造converted speech路径。

## 实际验证与身份

真实RED三项为0通过/3失败，11.08s，清单666项，无漂移。首次默认完整回归212通过/43失败全为沙箱禁止现有fixture的`127.0.0.1:0`绑定；原日志保留。允许本机临时回环端口后同源默认 **335通过/0失败/0忽略**，68.75s；无默认 **260/0/0**，39.55s，是重复配置验证。新增8项和原四项组合模拟均通过；实际CLI两项纯文件replay通过，输入不变。

最终666输入manifest SHA-256为`fb4a641051ef77125d788d7013487dcc7fda488e43c74044a0690c5767144fe7`，与实现提交逐blob匹配。见[验证摘要](handoffs/2026-10-14-r02-clock/evidence/validation-summary.json)、[提交核对](handoffs/2026-10-14-r02-clock/evidence/commit-verification.json)及[证据索引](handoffs/2026-10-14-r02-clock/evidence/evidence-index.json)。变更Rust格式通过，全仓fmt仍仅有未改动Server历史import排序。

全部验证进程已结束，只保留ignored构建缓存、原日志和合成fixture。无新增依赖、部署服务、外部数据库、真实录放音/执行器、模型或SSH；本轮Server、Linux/Pi/ALSA runtime、staging启停及长稳未运行。默认shadow、政策/执行批准、单consumer、精确取消与owner清理保持。

## 下一步与暂停项

1. 本轮软件审计及已复现缺口已关闭，不重复另建帧框架。继续R02前先确认R1硬件AEC的播放参考路由、固件/格式、扬声器/功放，以及硬件处理后是否还需要host软件AEC；当前R1仍未接入。
2. 据已确认路径固定实际consumer的采样率/声道/帧长、调用次序、尾帧、延迟/等待上界及reset规则，然后才安排可验证的组帧/对齐接入；不将现有rubato固定比率转换视为漂移控制，也不猜ppm/延迟阈值。
3. 真实双讲/尾音/延迟验收仍需明确录放音窗口和PCM保留规则；当前软件证据不证明采播物理对齐或AEC效果。资料未齐时只继续明确范围的离线证据工作。
4. **R03继续暂停，须用户恢复后推进**；已完成保持、校准后归中和双端受信清单决策保留。部署账户/目录/散热/模型端点版本待定，Pi既有SSH许可保留但本轮未用。R05等待R02/R03现场出口，V01、Settings剩余故障、前端/额外平台/端侧及同机部署沿[延期台账](deferred.md)继续保留。
