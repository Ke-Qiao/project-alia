# 2026-10-12 Settings GET 身份重验证 checklist

基线 `b15fa7d87e8447d4f6819332121823c271b41573`，分支 `codex/architecture-refactor`。本轮 **10/10 完成**；L09—L12 **4/4**，R01—R06 仍 **2/6**，不是项目完成百分比。

- [x] 01 读取上轮检查点和四份 HANDOFF，确认干净基线与三个 subagent 分工。
- [x] 02 审计 GET 全部等待、响应/ETag/prefix、secret schema 与管理员读取边界。
- [x] 03 未修改生产代码时，以真实 HTTP/PG 和假密钥复现降权后的敏感响应。
- [x] 04 最终设置快照完成后、响应构造前核验当前管理员；复用 PATCH 原查询且保持其行为。
- [x] 05 回归降权/停用/删除/核验失败或超时与成功 API，拒绝不返回设置或 ETag。
- [x] 06 三人审阅、冻结最终输入、边界/格式/差异检查通过。
- [x] 07 macOS workspace 和 Server 无默认 feature 全目标验证。
- [x] 08 同源 Linux ARM64 Server 全量和无默认 feature 全目标验证。
- [x] 09 核对本轮无遗留测试连接/schema，关闭 PostgreSQL 和 Linux holder，归档证据。
- [x] 10 正常提交并核对 Git blobs，更新四份 HANDOFF、台账和恢复检查点。

本轮保护敏感响应交付；原 SQL current 自动暂停到期和 Registry 政策投影保留，不宣称被拒绝的 GET 绝无状态变化。不新增响应遮罩、持续撤权、跨网络发送期间用户行锁或其他权限框架。

源码 `d080710052ae308b962b826d0f096fb10067469e`；最终 V3 manifest `48547827ad52952af80c9275cc83dd35ef9da5a25cb58b7fb32e8cab57fc1c7c`，655/655 源码 Git blobs 一致。

macOS ARM64 workspace **1134/0/5**；Linux ARM64 Server 默认完整回归 **852/0/5**。两端 Server 无默认 feature 全目标检查、架构边界正反自测、变更 Rust 格式、差异及计数器 8 例自测通过。数字为 passed/failed/ignored，嵌套子进程摘要不重复计入。全部最终阶段同源且无漂移，原期限与默认测试并发保留。新增 GET 6 项及既有 PATCH 8 项均在两端全量中逐项通过；定向 V3 的 6/0/0 不额外计入总数。

RED 在未改生产时有效复现降权后的 200/ETag/假密钥泄露。GREEN V1 为 5/1，V2 保留原断言的诊断单例为 0/1；166 个键中 3 个浮点 value 与直接内存值比较不同，但期望按 HTTP 同样 JSON 编码解码后完整严格相等。V3 只修正测试比较层次，不用容差或删除字段，生产自 V1 起未变。Linux V1 仅准备环境，未执行 Cargo；Linux 最终阶段明确使用 V3。失败、诊断、原 RED 测试全文和各 manifest 均保留。

全部 Cargo 已结束，本轮 PostgreSQL 及 Linux V1/V3 holder 均已停止。关闭前其他 DB 连接 0、测试 schema 0，遗留 schema 清理 0 个；用户 5433 未访问。原日志、源码快照与缓存留在 ignored `run/validation-20261012-settings-read-authorization/`，可核对副本与哈希见证据索引。空闲 holder 停止码不是 Cargo 结果。

下一软件候选：先审计 Settings GET/PATCH 共用 update gate 的排队、各阶段预算和取消所有权。current/final-auth 已有 2 秒，gate/preview/apply 等待尚未故障实证；区分准入前等待与 SQL/desired/module 已发生后的部分成功，不直接套整请求 timeout 或全局回滚，不把客户端断开等同 handler 已取消。

[恢复检查点](../resume-checkpoint-settings-read-authorization-2026-10-12.md)。
