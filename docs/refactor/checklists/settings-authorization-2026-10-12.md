# 2026-10-12 Settings 等待后的身份重验证 checklist

基线 `bdc94dd245251a45b620877be728a7635d3e1296`，分支 `codex/architecture-refactor`。本轮 **10/10 完成**；L09—L12 仍 **4/4**，R01—R06 仍 **2/6**，不是全项目完成百分比。

- [x] 01 读取恢复检查点与四份 HANDOFF，核对提交基线、恢复工作区已有 RED 测试和三个 subagent 分工。
- [x] 02 审计入口身份快照、更新 gate、SQL current、两次 embedding preview 和原政策 actor/CAS。
- [x] 03 未修改生产源时用真实 HTTP／PG 等待见证复现，归档 RED 与副作用证据。
- [x] 04 纯非政策分支在最终准入点做有界 active admin 重验证；保留原错误/API/政策语义。
- [x] 05 回归降权、停用、删除、核验失败／超时及成功路径，拒绝无请求设置副作用。
- [x] 06 三 subagent 独立复查；冻结最终 manifest，边界、格式、diff 检查通过。
- [x] 07 macOS workspace 与 Server 无默认 feature 全目标验证。
- [x] 08 同一源码 Linux ARM64 Server 默认完整回归与无默认 feature 全目标验证。
- [x] 09 关闭本轮数据库和 Linux holder，验证无遗留任务连接／schema，保存证据。
- [x] 10 正常提交，核对 source Git blobs，更新四份 HANDOFF、checklist、暂缓项与恢复检查点。

准入后仍使用既有部分成功与显式重试；不为本轮引入持续撤销、跨模块回滚或新依赖。GET Settings 的相似等待窗口单列后续候选，不扩大本轮写入权限清理范围。

V1 定向真实 HTTP/PG 回归 **8/0/0**，exit 0；独立审查发现测试 panic 清理应先移除已完成 JoinHandle，修正后冻结 V2，再完成全量验收。首次 RED 屏障失败与第二次有效业务 RED 分开保留。纯更新 gate 排队和慢 JSON body 等待没有独立动态见证，最终核验位置由静态顺序与实际政策/preview 等待回归覆盖。

源码 `4553e88138cb91af46f024b343baaa3857c8d769`；最终 V2 manifest `1f88c037cc584c41b88efba5141858930926b1c066f2b0b881e7579e490a90e1`，654/654 源码 Git blobs 一致。

macOS ARM64 workspace **1128/0/5**；Linux ARM64 Server 默认完整回归 **846/0/5**。两端 Server 无默认 feature 全目标检查、架构边界正反自测、变更 Rust 格式、差异和计数器 8 例自测通过。数字为 passed/failed/ignored，嵌套子进程摘要不重复计入。全部最终阶段同源且无漂移，保持默认测试并发和原期限。新增 8 项身份回归均在两端全量中通过，V1 定向 8/0/0 不额外加到全量数字。

RED 首次屏障失败与 V2 有效越权复现分别保留；GREEN V1 定向通过后，仅修复测试 panic 清理再冻结 V2。Linux 首次准备缺少空目录，复制源码和启动容器前失败；补目录后准备成功，未变更生产源码。最终 Cargo 阶段均通过。

全部 Cargo 已结束，本轮 PostgreSQL 和 Linux holder 已停止。关闭前其他 DB 连接 0、测试 schema 0，遗留 schema 清理 0 个；用户 5433 未访问。原日志、源码快照与缓存留在 ignored `run/validation-20261012-settings-authorization/`，可核对副本及哈希见证据索引。空闲 holder 停止码不是 Cargo 结果。

下一软件候选：先审计并复现 `GET /api/v1/settings` 等待后返回含敏感配置的设置前的当前身份有效性；保持原 API 和管理员读取边界，不扩大为全系统持续撤权框架。

[恢复检查点](../resume-checkpoint-settings-authorization-2026-10-12.md)。
