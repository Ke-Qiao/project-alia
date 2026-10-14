# Settings 慢模型、额度与共享锁收尾

日期：2026-10-15；分支 `codex/architecture-refactor`；基线 `e89d34c`。仅 Server 软件；默认 shadow、政策、管理员边界、部分成功及显式重试保持。R03 继续暂停，无新依赖、硬件或外部模型。

- [x] 01 读取检查点、既有实现与取消证据，三个 subagent 按模型、TTS、关闭审查分工。
- [x] 02 固定 GET/PATCH/privacy、provider、额度、SQL/desired/module/file 的调用与所有权范围。
- [x] 03 真实复现三个 LLM 配置锁与 SoVITS 额度期限缺口，保留冻结源码及失败日志。
- [x] 04 LLM 每请求一次完整配置快照；保持流式解析、工具与取消所有权。
- [x] 05 SoVITS 每次权重请求的原期限包含额度准入；保留本地修改、远端 warning 与显式重试。
- [x] 06 核对原 HTTP pipeline 关闭基线已通过，保留实际 Drop、SQL/desired/模块效果及旧文件证据，不因静态猜测重写 Runtime。
- [x] 07 补真实断开/共享锁关闭回归，执行同源定向、Server 整合与无默认特性检查。
- [x] 08 三方交叉审查生产差异、测试边界、API/政策与依赖未变。
- [x] 09 保存四份 HANDOFF、证据、进度和恢复检查点，清理本轮隔离数据库。
- [x] 10 正常提交并核对提交内容与冻结验证输入一致。

本轮 **10/10**；完成数只表示本软件包，不表示项目百分比。R01—R06 仍 **2/6**（R01/R04），R02 现场门槛和 R05 未关闭。

源码提交 `4e99134`；Server 全目标 **868/0/5**，provider 复核 **51/0/0**，无默认 all-targets 检查、9 个改动 Rust 格式及 diff 检查通过。整包后只有一处 cfg 属性格式调整，身份转换与669个提交输入验证见 [证据索引](../handoffs/2026-10-15-settings-waits/evidence-index.json)。task PostgreSQL 已删除，共享运行时保留。
