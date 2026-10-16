# 当前版本 Linux 软件发布验收交接

2026-10-17；本轮 **10/10 完成**；Linux 构建、测试与 release 链接矩阵通过，三个 subagent 均完成独立复核。分支 `codex/architecture-refactor`，基线 `da3eca2`，应用源码仍为 `4e2968f`。无生产、测试或依赖修改；默认 shadow、政策与原测试期限/并发保持，R03 暂停。

- [Root](root-HANDOFF.md)：结果、源码与产物身份、资源回收、下一软件出口。
- [Environment](environment-HANDOFF.md)：镜像、私有缓存、native 输入和真实链接。
- [Tests](tests-HANDOFF.md)：完整计数、Mac/Linux 平台差异、ignored 和期限边界。
- [Review](review-HANDOFF.md)：独立核对与验收限制。
- [Checklist](../../checklists/linux-release-2026-10-17.md)、[恢复检查点](../../resume-checkpoint-linux-release-2026-10-17.md)。

默认完整 workspace 一次 **1285/0/5/0 filtered**，两端分别 no-default all-targets 通过，两端 default release 构建与真实链接通过。Embedded 170 个定义 Sherpa C API 符号、安全 help 通过；没有启动普通 Server/Embedded 服务或硬件。

677 输入与基线 Git、最终宿主及上一轮 Mac 逐项相同。50 份字节一致原件（721154 字节）见 [证据索引](evidence-index.json)和[汇总](evidence/evidence/validation-summary.json)。独立二进制留在 ignored run/artifacts，哈希见 [产物清单](evidence/evidence/retained-artifacts.json)。测试后约 11.165 秒 PG 0/0，关闭前再核对 0/0；两个任务容器、匿名卷和 internal network 已回收，共享服务保留。

这是当前 Linux ARM64 软件构建/测试/链接出口，不代表实际部署、Pi 录放音、AEC、执行器、真实模型、其他平台或长期负载通过。历史 V01 根因仍 open；六轮仍 2/6。下一步候选为已有全 noop staging 的实际进程生命周期与恢复演练，详见恢复检查点。

作者文档及非原日志 diff 检查通过；完整 diff 检查仅报告原始 `workspace-default.log:1830` 的 EOF 空行，保留日志原字节，不为消除格式提示改写原件。
