# Root — Settings GET 身份重验证

最终检查项 **8/8 完成**；以下保留各实施时点，最终结果以文末 Root 验收记录为准。
基线 `b15fa7d87e8447d4f6819332121823c271b41573`，分支 `codex/architecture-refactor`。工作区从干净提交开始，本轮只清理 GET 在等待后交付敏感设置的管理员准入缺口。

继续使用三个 subagent：`settings_auth_fix` 负责唯一生产文件；`settings_auth_reproduce` 负责真实 HTTP/PG 回归；`settings_auth_review` 独立只读审查。Root 负责冻结、统一运行、文档、证据、资源关闭和提交。

## Checklist

- [x] 核对上轮四份交接、GET 全部 await、SettingEntry secret 元数据与 API 调用者。
- [x] 未改生产时，以真实政策锁等待复现降权后 200/ETag/假密钥泄露。
- [x] GET 最后快照 await 后重验证；PATCH 私有查询提取但位置和行为不变。
- [x] 保留 V1/V2 失败与脱敏诊断；仅修正测试按实际 JSON 表示严格比较。
- [x] V3 定向 6/0/0，静态审查、格式和边界通过，冻结 655 输入。
- [x] 同源 macOS workspace/Server no-default、Linux ARM64 Server/no-default 验收。
- [x] 资源关闭、正常源码提交与 Git blobs 核对。
- [x] 证据、checklist、四份交接、台账和恢复检查点提交。

## 证据解释

RED manifest `e9dbd269854da9e4562a48d763e1875380862dd83c52130c539983a435842161`；19.53 秒、exit101、无漂移。等待见证建立后降权，原 GET 返回 200、ETag 和随机假密钥；原测试全文另存，日志仅含布尔。

修复 V1 定向 5/1，所有拒绝通过，但正常管理员全图 JSON 比较失败。V2 保留原断言并补脱敏诊断：166 个键、3 个浮点 value 差异、schema 相同，期望经实际 JSON 编码解码后全图严格相等。V3 只修正期望的比较层次并移除诊断，没有容差或省略字段，生产不变；定向 **6/0/0**、13.23 秒、无漂移，超时用例约 2005 ms。

最终 V3 manifest `48547827ad52952af80c9275cc83dd35ef9da5a25cb58b7fb32e8cab57fc1c7c`。Linux V1 仅创建快照/容器和记录环境，未执行 Cargo；最终 Linux 使用 V3 独立只读快照，不能混用版本或计数。

## 边界与后续

拒绝只保证不交付设置/ETag；原 SQL current 自动暂停到期及 Registry 政策投影保持。最终核验是一次响应准入，不持续锁住用户直到网络发送结束。成功的 secret 字段、JSON、prefix 和 ETag 均保持，不新增服务端遮罩。PATCH、政策/embedding SQL actor/CAS、默认 shadow、执行批准、精确取消、依赖与平台不变。

纯 gate 排队和 Registry 私有锁未独立动态注入；最后核验位置由静态全 await 审查及真实政策等待验证。下一候选为 GET/PATCH 共用 update gate 的排队、各阶段预算与取消所有权，尚未故障实证；不能直接套整请求 timeout、全局回滚或把客户端断开等同 handler 取消。

## Root 最终验收补记

源码 `d080710052ae308b962b826d0f096fb10067469e`；最终 V3 manifest `48547827ad52952af80c9275cc83dd35ef9da5a25cb58b7fb32e8cab57fc1c7c`，655/655 源码 Git blobs 一致。

macOS ARM64 workspace **1134/0/5**；Linux ARM64 Server 默认完整回归 **852/0/5**。两端 Server 无默认 feature 全目标检查、架构边界正反自测、变更 Rust 格式、差异及计数器 8 例自测通过。数字为 passed/failed/ignored，嵌套子进程摘要不重复计入。全部最终阶段同源且无漂移，原期限与默认测试并发保留。新增 GET 6 项及既有 PATCH 8 项均在两端全量中逐项通过；定向 V3 的 6/0/0 不额外计入总数。

RED 在未改生产时有效复现降权后的 200/ETag/假密钥泄露。GREEN V1 为 5/1，V2 保留原断言的诊断单例为 0/1；166 个键中 3 个浮点 value 与直接内存值比较不同，但期望按 HTTP 同样 JSON 编码解码后完整严格相等。V3 只修正测试比较层次，不用容差或删除字段，生产自 V1 起未变。Linux V1 仅准备环境，未执行 Cargo；Linux 最终阶段明确使用 V3。失败、诊断、原 RED 测试全文和各 manifest 均保留。

全部 Cargo 已结束，本轮 PostgreSQL 及 Linux V1/V3 holder 均已停止。关闭前其他 DB 连接 0、测试 schema 0，遗留 schema 清理 0 个；用户 5433 未访问。原日志、源码快照与缓存留在 ignored `run/validation-20261012-settings-read-authorization/`，可核对副本与哈希见证据索引。空闲 holder 停止码不是 Cargo 结果。

本轮总 checklist **10/10**，L09—L12 **4/4**，R01—R06 仍 **2/6**，不是项目完成百分比。

未独立动态注入纯更新 gate 排队或 Registry 私有锁等待；最后核验位置由完整 await 静态审查和真实政策等待验证。未运行真实 PTY/Easy-Turn、Linux 全 workspace、release/部署 smoke、外部付费服务、Pi/声学/执行器或长稳。核验是一次响应准入，不承诺后续持续撤权或撤回已发送响应。
