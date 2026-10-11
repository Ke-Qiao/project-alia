# 2026-10-12 Settings 身份重验证交接

本轮范围是纯非政策 `PATCH /api/v1/settings` 等待后的当前管理员准入。继续保留政策事务内 actor/CAS、API、部分成功与显式重试；不新增依赖或平台。

依次读取四份交接：

1. [Root](root-HANDOFF.md)：集成、源码身份、验证和后续入口。
2. [生产流程](flow-HANDOFF.md)：唯一生产修复与权限边界。
3. [回归](regression-HANDOFF.md)：RED、等待见证及 8 项真实 HTTP/PG 场景。
4. [独立审查](review-HANDOFF.md)：授权位置、事务边界及证据限制。

关联 [技术说明](../../settings-authorization-2026-10-12.md)、[checklist](../../checklists/settings-authorization-2026-10-12.md)、[证据索引](evidence-index.json)。证据包含旧失败、最终源码 manifest、原始日志、命令、计数及资源关闭。归档运行辅助脚本保留原执行路径作为审计材料，不保证从文档目录直接重跑。

本轮 **10/10**。

源码 `4553e88138cb91af46f024b343baaa3857c8d769`；最终 V2 manifest `1f88c037cc584c41b88efba5141858930926b1c066f2b0b881e7579e490a90e1`，654/654 源码 Git blobs 一致。

macOS ARM64 workspace **1128/0/5**；Linux ARM64 Server 默认完整回归 **846/0/5**。两端 Server 无默认 feature 全目标检查、架构边界正反自测、变更 Rust 格式、差异和计数器 8 例自测通过。数字为 passed/failed/ignored，嵌套子进程摘要不重复计入。全部最终阶段同源且无漂移，保持默认测试并发和原期限。新增 8 项身份回归均在两端全量中通过，V1 定向 8/0/0 不额外加到全量数字。

RED 首次屏障失败与 V2 有效越权复现分别保留；GREEN V1 定向通过后，仅修复测试 panic 清理再冻结 V2。Linux 首次准备缺少空目录，复制源码和启动容器前失败；补目录后准备成功，未变更生产源码。最终 Cargo 阶段均通过。

全部 Cargo 已结束，本轮 PostgreSQL 和 Linux holder 已停止。关闭前其他 DB 连接 0、测试 schema 0，遗留 schema 清理 0 个；用户 5433 未访问。原日志、源码快照与缓存留在 ignored `run/validation-20261012-settings-authorization/`，可核对副本及哈希见证据索引。空闲 holder 停止码不是 Cargo 结果。

未独立动态验证纯 update gate 排队或慢 JSON body 窗口；最终核验位置由静态顺序与真实 policy/preview 等待验证。未运行真实 PTY/Easy-Turn、Linux 全 workspace、release/部署 smoke、外部付费服务、Pi/声学/执行器或长稳。最终核验是一次准入，准入后的降权不追溯撤销已开始的更新。

原始 Cargo 日志的结尾空行按原字节保留，以保持证据哈希。提交前差异检查对这些 `.log` 仅关闭 `blank-at-eof` 规则，其余源码与文档使用正常空白检查；没有重写日志来满足格式检查。
