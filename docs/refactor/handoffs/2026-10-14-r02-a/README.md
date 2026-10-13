# R02-A 双流有界证据与合成 PCM 离线重放交接

日期：2026-10-14（Asia/Shanghai）。分支 `codex/architecture-refactor`；起点 `f372ad3b4a7fc87de2526840fcada461080c20dc`；实现提交 **`e557ceb962610004287f38e66b50abca3e44b661`**。本软件包 **10/10**，R01—R06仍 **2/6**。

- [Root HANDOFF：范围、组合验证、资源与恢复](root-HANDOFF.md)
- [Native HANDOFF：双流、单消费者和时钟](native-HANDOFF.md)
- [Evidence HANDOFF：会话、格式、导出与重放](evidence-HANDOFF.md)
- [Review HANDOFF：独立审查和失败边界](review-HANDOFF.md)

[Checklist](../../checklists/r02-a-2026-10-14.md) · [最新恢复检查点](../../resume-checkpoint-r02-a-2026-10-14.md) · [契约与使用说明](../../audio-reference-clock-design.md) · [验证摘要](evidence/validation-summary.json) · [归档哈希索引](evidence/evidence-index.json)

先取得4项预期 RED，再用冻结后的659个输入执行默认 features/默认并行 Embedded all-targets：macOS ARM64 **284通过/0失败/0忽略**（lib218、integration66）。实现提交全部659个输入与测试清单一致。首轮沙箱42项 loopback bind EPERM 保留原始日志；同源同命令允许本地回环后全过。

合成示例实际执行导出→读回→重放：7条记录、16个样本、9个reset，并报告尾部丢失及旧流最终状态未知。再次导出被拒绝，原文件字节不变且无临时文件残留。源码没有默认文件录音或自动证据会话，没有新依赖/AEC；旧 source-WAV recorder 与配置已删除。运行采集时仍存在固定大小的内存 PCM 队列。

Embedded fmt通过；全仓fmt只报未改的历史 Server import 顺序。Linux/OrbStack停止，本轮没有启动平台、真实录放音或硬件，不能据软件结果声称AEC、声学对齐、物理取消或整机验收完成。

## 证据使用

evidence/ 保留 RED/GREEN manifest、RED patch、每个命令的原始 log/JSON、环境、合成文件、拒绝覆盖核对和提交核验。索引逐文件记录字节数与 SHA-256；日志末尾空行和 patch 上下文空格是原字节，格式检查仅对这些确切路径豁免对应空白规则，不改写证据。

freeze-source.py、validate.py 是当时运行脚本的原样存档，路径假设仍为仓库 run/validation-20261014-r02-a/，不能直接从 docs/evidence/ 当作启动器执行。时间字段使用 UTC，因此记录中的2026-10-13晚间对应本地2026-10-14。来源标签是声明而非真实性认证；合成文件不含真实录音。
