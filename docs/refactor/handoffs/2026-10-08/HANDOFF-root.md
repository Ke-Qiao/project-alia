# Root handoff — 2026-10-08

## 范围与状态

用户授权推进干净重构，重点为 Memory 的 provider 残留和 Pipeline 过重。基线 `adb28c1`；工作从干净的 `codex/architecture-refactor` 继续。分工和数字进度见 [本轮 checklist](../../checklists/layering-2026-10-08.md)，完整职责说明见 [技术说明](../../layering-2026-10-08.md)。

Root 负责所有外部调用者、唯一 Cargo / PostgreSQL 验证窗口、最终审查与正常提交。三个 subagent 分别负责 Memory、Interaction、Bootstrap / Platform；其 HANDOFF 的“实现完成”与实际运行验收分别记录。

## 已完成的代码工作

- Memory 类型、策略、服务和采集完整迁入 `modules/memory`；删除旧 provider trait/no-op/名称/路径与单实现工厂，原 25 项具体操作保持。
- 删除 Channel 下的 Pipeline；Interaction 明确拥有轮次用例，接收实际服务集合，缓存完成出口和模型执行阶段去重。
- Bootstrap 组装服务图，Platform 拥有生命周期和后台任务；Easy-Turn 具体构造归 Bootstrap。
- API、工具、Insights、设置、测试均采用新契约与路径。历史数据库 migration、既有政策和外部协议没有修改。
- 新增标准库架构检查器与负例自测；当前架构文档区分已完成范围与剩余固定核心。

## 审查与失败证据

移除 trait 后已逐项核对 Memory 的 25 个操作；真实 SQL / guard / transaction 不变，两项原默认查询方法原样成为具体方法。生命周期迁移保留所有权、注册和关闭顺序。

独立审查发现新的缓存后取消出口遗漏终端中断记录。新增确定性回归在修复前准确失败：`cache cancellation lost the terminal interruption audit`，实际 0 条、预期 1 条。修复在共享退出点保留用户来源、原中断阶段和终端审计；精确与语义缓存都覆盖，不继续路由或生成输出。未通过延长超时、串行化或忽略测试绕过失败。

编译预检中的旧导入和测试导入遗漏已修复，最终预检 `cargo check -p alia-server --all-targets --locked --offline` 通过、无 warning。完整验证以最终冻结源码为准。

## 源码身份（V3 为已提交实现，V4 补测试夹具）

- 源码提交：`08c54be3cca92afecccf36d8c85324909a0ce925`；630/630 Git blob 与冻结输入逐项匹配。
- V3 冻结输入：630。
- V3 manifest：`source-v3-manifest.json`。
- SHA-256：`7900e3c771864992562374a4bb8de47d54b1eba014fdf3c9a641afc0ab3dd7dc`。
- v1 定向测试：49 passed / 0 failed / 0 ignored；输入 SHA `ffc896d1e31dd24cbb8cdcb5505247c951daa95f18d96412966743912b572e1e`。
- v2 修复前取消回归：0 passed / 1 failed；输入 SHA `a4ff199a858e6075df0f3c0f7f83f3db61ef5fa8faf29d9dc1b42beac831960e`。

最终运行计数与提交对应见本文件末尾 Root 最终验收；V3 历史与 V4 最终结果分开保留。

## 续接边界

下一轮从技术说明的 **L09 固定核心归属** 开始检查，而非重新实现 Memory/Pipeline。随后 L10 缩窄初始化依赖、L11 跨域事务所有权、L12 出站契约归属分别按实际调用链推进。不得声称本轮完成整个架构提案。

R01—R06 仍为 2/6；前端、真实 AEC、执行器反馈、V01 根因、Pi 整机和其它平台的待办仍在原台账。本轮没有新增依赖、Cargo feature、设备绑定、凭证、外部模型、物理 I/O 或用户数据操作。

## Linux 失败调查与 V4

V3 全 Server 首次 **770/1/5**，唯一失败为 `appraisal_ack_deadline_reports_unknown_and_clear_fences_the_original_identity`；同源单项复核通过。原始 SQL 日志已保留，显示 Appraisal INSERT 的 statement timeout。三个相同 2 秒期限并无固定返回先后，不能靠复跑通过解释原失败。V4 只修正该用例的真实排队夹具，保留全部原行为断言并 join 第一个写入；默认政策及生产代码没有改动。V4 manifest `ccfaa7981fc7062dfeb43e568e52af038915064a2187352e0891980afc6fcbd3`，630 输入，629 与 V3 相同。

## Root 最终验收

本轮 **8/8**。最终源码 `978af9c942da42aa3cd98182cde3dafa6a6e33be`，V4 `ccfaa7981fc7062dfeb43e568e52af038915064a2187352e0891980afc6fcbd3`，630/630 输入匹配。macOS workspace **1053/0/5**、真实 PTY **3/0**、Server 无默认全部目标通过；Linux ARM64 Server 全包 **771/0/5**、Server 无默认全部目标和默认 release/native link 通过。架构守卫、自测、格式与差异检查通过。

实际命令与逐目标计数见 [最终验证表](../../layering-2026-10-08.md#最终验证与证据)、[证据索引](evidence-index.json)。两项真实模型未重跑；不声称 Linux 全工作区或硬件验收。V3 Linux 原失败与定向复核保持原样，V4 仅修测试夹具。三个 subagent 已交回；两个 Linux holder 和独立 PG 已停止，用户 5433 未访问。没有待接回的 Cargo/PG 作业。下一步从 [L09（0/4）](../../checklists/layering-next.md) 开始。
