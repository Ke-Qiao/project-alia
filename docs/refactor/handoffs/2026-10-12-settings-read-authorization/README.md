# 2026-10-12 Settings GET 身份重验证交接

本轮保持 Settings API、政策和管理员读取权限，修复等待后返回敏感配置前的当前身份核验。

四份交接依次为：

1. [Root](root-HANDOFF.md)：集成、冻结、验证、资源和提交。
2. [生产流程](flow-HANDOFF.md)：GET 最后准入及 PATCH 查询复用。
3. [复现与回归](regression-HANDOFF.md)：真实等待、敏感响应检测、JSON 契约及失败清理。
4. [独立审查](review-HANDOFF.md)：授权时点、API、测试与证据范围。

关联 [技术说明](../../settings-read-authorization-2026-10-12.md)、[checklist](../../checklists/settings-read-authorization-2026-10-12.md)、[证据索引](evidence-index.json)。原始失败与最终结果分别保留；归档运行脚本保留原路径作为审计材料，不保证在文档目录直接重跑。

原始 Cargo 日志按原字节保留，包括结尾空行；提交前对日志只豁免 `blank-at-eof`，其余源码和文档正常检查，避免为格式重写证据。

本轮 **10/10**。

源码 `d080710052ae308b962b826d0f096fb10067469e`；最终 V3 manifest `48547827ad52952af80c9275cc83dd35ef9da5a25cb58b7fb32e8cab57fc1c7c`，655/655 源码 Git blobs 一致。

macOS ARM64 workspace **1134/0/5**；Linux ARM64 Server 默认完整回归 **852/0/5**。两端 Server 无默认 feature 全目标检查、架构边界正反自测、变更 Rust 格式、差异及计数器 8 例自测通过。数字为 passed/failed/ignored，嵌套子进程摘要不重复计入。全部最终阶段同源且无漂移，原期限与默认测试并发保留。新增 GET 6 项及既有 PATCH 8 项均在两端全量中逐项通过；定向 V3 的 6/0/0 不额外计入总数。

RED 在未改生产时有效复现降权后的 200/ETag/假密钥泄露。GREEN V1 为 5/1，V2 保留原断言的诊断单例为 0/1；166 个键中 3 个浮点 value 与直接内存值比较不同，但期望按 HTTP 同样 JSON 编码解码后完整严格相等。V3 只修正测试比较层次，不用容差或删除字段，生产自 V1 起未变。Linux V1 仅准备环境，未执行 Cargo；Linux 最终阶段明确使用 V3。失败、诊断、原 RED 测试全文和各 manifest 均保留。

全部 Cargo 已结束，本轮 PostgreSQL 及 Linux V1/V3 holder 均已停止。关闭前其他 DB 连接 0、测试 schema 0，遗留 schema 清理 0 个；用户 5433 未访问。原日志、源码快照与缓存留在 ignored `run/validation-20261012-settings-read-authorization/`，可核对副本与哈希见证据索引。空闲 holder 停止码不是 Cargo 结果。

未独立动态注入纯更新 gate 排队或 Registry 私有锁等待；最后核验位置由完整 await 静态审查和真实政策等待验证。未运行真实 PTY/Easy-Turn、Linux 全 workspace、release/部署 smoke、外部付费服务、Pi/声学/执行器或长稳。核验是一次响应准入，不承诺后续持续撤权或撤回已发送响应。
