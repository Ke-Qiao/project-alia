# 当前恢复检查点：R02 Mac 离线模拟

日期2026-10-14；分支`codex/architecture-refactor`；本轮基线`ea3f0e4`，实现提交`160704578cd79d23898e6c8225d18c8511895d39`。本包 **10/10完成并正常提交**；R01—R06仍 **2/6**，R03后续保持用户要求的暂停。此检查点/证据的文档提交紧随实现提交，不将文档变更算成新测试源码。

先读[checklist](checklists/r02-mac-2026-10-14.md)、[四份HANDOFF](handoffs/2026-10-14-r02-mac/README.md)、[音频操作说明](../../packages/alia-embedded/docs/audio-and-voice.md)及[硬件资料](../operations/hardware-deployment-readiness.md)，核对branch/status/log。已完成R03-0/R02-A/R03-A、现场前置和本包不重复实施；三个subagent已完成分工并停写，下一轮仍沿用三名分工与Root统一验证/提交。

## 本轮已交付

- 严格`generate <new-file>` / `replay <existing-file>`示例CLI；旧隐式单参数拒绝，help无写入，普通文件有界只读导入，生成不覆盖。
- 从输入推导每流记录/PCM/reset/关闭标记与匹配最新快照；支持空、单向和仅footer。来源声明不认证，不推定物理完成、旧流终态或采播对齐。
- Closed loss不得吞掉已观察成功记录/PCM的checked下界；部分观察仍合法，活动快照保持独立原子观察语义。
- 四项组合模拟连接生产capture helper、post-mix callback、单EvidenceSession、JSON与replay，覆盖精确cue取消、明确暂停/继续、观察取消/drop、溢出/丢关闭标记/重开及资源清理。五项example回归覆盖CLI操作边界。

## 实际验证与证据

默认Embedded **327通过/0失败/0忽略**；无默认features **252/0/0**为重复验证；实际CLI **9/9**符合预期，包括历史fixture只读重放与不覆盖。旧CLI `--help`写文件及closed矛盾摘要均已真实复现并保存RED，不使用历史测试数替代。

最终666项输入的manifest SHA-256为`ef33fcd714968f0236f42a065783c9967f23f7b0ac22deee6f120b376efef7f8`，与实现提交逐blob匹配；每轮source_drift为空。见[验证摘要](handoffs/2026-10-14-r02-mac/evidence/validation-summary.json)、[提交核对](handoffs/2026-10-14-r02-mac/evidence/commit-verification.json)和[证据索引](handoffs/2026-10-14-r02-mac/evidence/evidence-index.json)。变更Rust格式通过；全仓fmt仅有未改动Server旧import排序，保持单列。

全部验证进程已结束、CLI临时目录已清理；只保留本地ignored构建缓存和原始验证文件。没有新增依赖、SSH、服务、真实录放音/执行器或模型；本轮Linux、Server、staging正常启停和长稳未运行。

## 后续执行顺序与暂停项

1. R02继续前先有界审计已有独立时钟、gap/reset与实际AEC consumer的帧需求，明确待测漂移/对齐契约再决定具体补缺口；不重复基础fixture，也不先引入AEC依赖或未选定consumer的重采样框架。
2. 实际AEC/双讲/尾音仍需R1接入、实物/固件、扬声器/功放及厂商参考规格，确认录放音窗口和PCM保留规则后再操作；资料不足不猜测电气或硬件参数。现有软件测试不关闭这些现场项。
3. **R03保持暂停，须用户恢复后才推进**；板卡/通道/供电/OE/机械限位/校准待定。已完成软件保留，停止保持、校准后启动归中和双端本地受信清单决策不变。
4. 部署账户/目录/散热/模型端点与版本待定；Pi既有SSH许可保留但本轮未使用。R05等待R02/R03现场出口。V01、Settings剩余故障、前端/其他平台/端侧及同机部署按[延期台账](deferred.md)继续保留。

默认shadow、当前政策、执行批准和硬件门槛保持；mixer不等于AEC，成功提交PCM不等于扬声器已播放。
