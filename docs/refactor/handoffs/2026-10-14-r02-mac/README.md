# R02 Mac 离线模拟交接

日期2026-10-14，基线`ea3f0e4`，分支`codex/architecture-refactor`，实现提交`160704578cd79d23898e6c8225d18c8511895d39`。本包 **10/10**；R01—R06仍 **2/6**，R03后续继续暂停。

- [Root](root-HANDOFF.md)：范围、CLI重构、复现、统一验证与提交。
- [音频契约](audio-HANDOFF.md)：closed loss一致性与单consumer审计，6/6。
- [模拟链路](simulation-HANDOFF.md)：生产音频helper组合回归与清理，8/8。
- [独立审查](review-HANDOFF.md)：输入、摘要、取消与所有权边界，7/7。

进度见[checklist](../../checklists/r02-mac-2026-10-14.md)，恢复从[检查点](../../resume-checkpoint-r02-mac-2026-10-14.md)开始。默认Embedded327/0/0；无默认252/0/0为重复feature验证，不能相加为独立测试；实际CLI九场景全部符合预期。没有真实录放音、执行器、模型、SSH、服务或新依赖。

## 证据

[验证摘要](evidence/validation-summary.json)、[最终输入清单](evidence/final-source-manifest.json)、[提交逐blob核对](evidence/commit-verification.json)、[文件哈希索引](evidence/evidence-index.json)均已保存。666项提交输入完全匹配，manifest SHA-256为`ef33fcd714968f0236f42a065783c9967f23f7b0ac22deee6f120b376efef7f8`。

[旧CLI实际复现](evidence/baseline-cli-reproduction.json)保留基线`--help`写文件的行为；[closed统计RED](evidence/red-closed-footer.log)为真实断言失败，另存RED清单与补丁，不用最终代码替代失败身份。[CLI演练](evidence/cli-smoke.json)涵盖新生成、只读重放历史与新文件、不覆盖、错误参数和矛盾closed拒绝，合成样本仍为7records/16samples/9resets；四项生产helper组合测试独立覆盖实际软件链路。

原始`.log`、`.patch`和JSON按字节归档，不为格式检查修改。归档Python脚本保留原`run/validation-20261014-r02-mac`工作位置约定，不能直接在本档案目录运行。测试资源均已结束，仅保留忽略的本地构建缓存与验证原件；未改共享服务环境。

变更Rust格式检查通过；全仓fmt仍仅报告未改动的Server旧import排序，见[静态结果](evidence/static.json)。本轮不声称Linux/ALSA、真实声学、模型或长稳通过。
