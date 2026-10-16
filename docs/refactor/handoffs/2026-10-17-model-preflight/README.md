# 2026-10-17 软件发布资料与真实模型链前置交接

本轮 **10/10：资料收口，用户答复及待定项已登记**；不是模型验收通过率。起点`6a8b113`，已验应用源码`6c62c84`；只有文档与Server示例两行注释修改，未运行测试／服务／模型或设备。

按以下顺序恢复：

1. [Root HANDOFF](root-HANDOFF.md)：交付、证据和MP10确认状态。
2. [配置 HANDOFF](config-HANDOFF.md)：角色、provider、覆盖层、政策与副作用。
3. [验收 HANDOFF](acceptance-HANDOFF.md)：已有入口、样本、原期限／并发和后续执行边界。
4. [独立审查 HANDOFF](review-HANDOFF.md)：来源、平台边界及文案交叉核对。

[checklist](../../checklists/model-preflight-2026-10-17.md) · [恢复检查点](../../resume-checkpoint-model-preflight-2026-10-17.md) · [软件发布记录](../../../operations/software-release.md) · [只读证据索引](evidence-index.json)

本轮只读原件保留于`run/validation-20261017-model-preflight/`，归档副本位于`evidence/`，索引提供字节数／SHA-256。上轮[148份证据](../2026-10-17-release-lifecycle/evidence-index.json)不重复复制、不更改失败分母；当前Linux release副本仍在其原ignored目录。本轮起点678输入全同，最终677项全同加1项TOML语义相同的注释差异，不生成新的构建通过声明。

默认shadow／probe=false、当前政策、原期限和并发不变；R03暂停，历史V01open，R01—R06仍2/6。BotCF拟测试基址、Developer模型／Juno／项目Whisper与开发验证方式已登记；具体模型／素材和运行身份仍未验，不自动启动或探测任何端点。
