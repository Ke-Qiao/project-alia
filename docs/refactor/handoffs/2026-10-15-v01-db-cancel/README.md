# V01 初始数据库构造取消交接

2026-10-15；本轮 **10/10 完成**。分支 `codex/architecture-refactor`，源码 `8bcda7af892ba36aea98b9de785afa68e3935940`。历史 Companion V01 根因仍开放，R03 保持暂停，R01—R06 仍为 2/6。

- [Root](root-HANDOFF.md)：范围、修复、验证与后续边界。
- [Ownership](ownership-HANDOFF.md)：独立 owner、原数据库 future 与调用方清理契约。
- [Cancellation](cancellation-HANDOFF.md)：四项取消回归和 V1 阶段观测失败的修订。
- [Review](review-HANDOFF.md)：独立源码与原始证据复核。
- [恢复检查点](../../resume-checkpoint-v01-db-cancel-2026-10-15.md)与[checklist](../../checklists/v01-db-cancel-2026-10-15.md)。

| 批次 | 结果 | 解释 |
| --- | --- | --- |
| 旧实现 RED | 0 通过 / 2 失败 / 0 ignored / 27 filtered | 直接取消 constructor 后 schema 遗留，已建工作 pool 仍开放；先记录再兜底 |
| 首次定向 GREEN | 7 / 0 / 0 / 24 filtered | 旧 3 项初始化失败与新 4 项取消；不代表后续 V2 源码全量 |
| V1 workspace | 1265 / 1 / 5 / 0 filtered | 新 buffered-ready 测试在到达观测阶段前计时，失败原件完整保留 |
| V2 workspace | **1266 / 0 / 5 / 0 filtered** | 仅修已确认的测试阶段门与缺证据清理；最终计数只采用本次 |

V2 duplex **31/0/0**，目标 46.53 秒；workspace 285.11 秒，67 个 Running 与 5 个 Doc-tests。5 个 ignored 是既有 PTY 3 项、真实 Easy-Turn 2 项，本轮未启用它们。原 25/32/45 秒、RPC 3 秒、诊断 1 秒、shutdown 10 秒、CASES=3、默认 libtest 并发均不变。

[证据索引](evidence-index.json)保存 31 份原件的长度和 SHA-256。最终[结果](evidence/validation-summary.json)、[提交输入核对](evidence/commit-verification.json)、[资源快照](evidence/final-v2-resources.json)和[回收记录](evidence/cleanup.json)可独立复核。V2 675 项输入与提交逐 blob 一致，清单 SHA-256 `51b11511010fedfa47cf8f01b814eb17731ded45b5404fb7ad01ff2e4ce9c553`。

命令结束约 0.175 秒后同一 PostgreSQL 为 0 schema / 0 其他 client；随后只删除本轮 exact ID/label 容器及卷，共享服务保留。原始日志的 EOF 空行不做美化；二进制保存在 ignored run 目录，身份已入索引。归档脚本沿用原 run 目录布局，不能直接从 evidence 路径当作通用工具运行。

本轮只改变测试夹具和测试，不改生产、默认 shadow、政策、依赖或平台范围。Drop 仅请求取消，await 原 completion 才证明清理完成；runtime 退出、强杀、永久数据库失联等不在普通取消证明内。
