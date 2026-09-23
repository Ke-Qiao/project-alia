# 输出生命周期暂停交接 · 2026-09-22

用户因额度不足明确要求暂停。本代理已停止实现、测试和编译；暂停后仅整理此交接，以及按 Root 明确授权清理其 SIGABRT 测试的两个可证明残留 schema。没有在共享树提交 Git、合并或推送。

## 当前定位与所有权

- 共享工作树：`/Users/ke-qiao/Developer/project-alia/run/worktrees/alia-duplex-integration`；分支 `codex/duplex-integration`，交接读取 HEAD 为 `82167ddf8724ade6b203de2f9fbbe45459034178`。HEAD 之外仍有未提交整合改动，不能把它当作完整受测源码提交。
- 本代理共享源码仅两份测试：`packages/alia-server/tests/delivery_service.rs`（修改）、`packages/alia-server/tests/output_lifecycle.rs`（新增）。交接时两文件 SHA256 均与冻结证据完全一致。
- `service.rs`、`service/output.rs`、`output/gate.rs` 及 Pipeline 生产修复归 Root；A/C 的协议、voice、媒体和其他未提交文件应保留，以 Root 总交接为准。全量 dirty 快照见 [git-status-at-handoff.txt](git-status-at-handoff.txt)。
- 本交接与 `docs/refactor/checklists/duplex-lifecycle.md` 是本代理文档；未运行新的格式/编译/测试命令。

## 固定账本提交

两个提交均已确认是当前 integration HEAD 的祖先：

1. `4846b4a4907ede1539a8e133374f2a9820206a47`：原控制账本验收，真实 PG 13/13；默认与无默认 all-targets 检查当时均 exit 0。
2. `56e9accb892ee59343bd20fc0860d8234cd6feb7`：后加入音频的真实观察期限在同一 admission 事务中仅向小收紧 scope，Resume 后仍保留；单个 60 秒 hold 不压缩无更短音频期限的 120 秒 scope。单项 1/1（0.38 秒），完整账本 13/13（3.82 秒），exit 0。

账本证据在 `/Users/ke-qiao/Developer/project-alia/run/worktrees/alia-duplex-control-ledger/run/validation-output-control-20260922/`，包括 `pg-final.log`、`pg-scope-deadline-full.log`、`scope-deadline-result.json`、`scope-deadline-commit.json`。独立账本树仍保留六项 Core-only 临时覆盖，两个提交均排除了它们：`Cargo.lock`、`crates/alia-core/Cargo.toml`、`src/delivery.rs`、`src/lib.rs`、新增 `src/output_control.rs` 与 `src/playback_control.rs`（后四项相对 `crates/alia-core/`）。不得把这些覆盖随意暂存或当作独立新修复。

## 已完成的生命周期验收

新 `output_lifecycle.rs` 六例经过真实 Context/Response/SQL admission 和生产 DeliveryService，以 fake Channel 屏障控制原设备 ACK：

- 排队成功后同一 poll 内 drop waiter 与最后 producer，仍保留原媒体 owner；终结后的重复调用只返回旧事实，不重新 reserve/dispatch。
- Control ACK 丢失只 Observe 原 command/intent；Control 与 Observe 均不可确认时持久 Unknown 并退休，不重发。
- 后加入 Audio 的 1 秒 SQL 许可到期能结束在途 Resume；不打开本地 gate，不伪造未收到的 ACK。
- Held 不阻断原媒体 owner 的真实 SQL 租约心跳。
- scope fence 超时保留原 future/channel owner 到真实 join；shutdown 不假称完成。
- 最后一次 cleanup 修复还证明：显式 `output.retire()` 后，即使设备 stop barrier 未释放，scope 已持久 Retired、Response.cancel_requested 已为 true；设备停止事实仍未知，原 future 未被丢弃。

旧 `delivery_service.rs` 十二例均增加真实 `begin_voice_output` → `prepare` → `bind` 和保活的原 producer。Fake Channel 新增 typed registration 和独立 scope fence；原 exact `cancel_playback` 身份、ACK 故障及已知前缀断言保持。

| 验证 | 结果 | 证据 |
| --- | --- | --- |
| 初次新目标 no-run | exit 101，8 项当时尚未整合的生产类型错误；未运行测试 | `../output-lifecycle-compile.log` |
| 两目标 no-run | exit 0，54.24 秒 | `../output-lifecycle-compile-final.log` |
| 真实 PG delivery_service | 12 passed / 0 failed / 0 ignored，11.35 秒 | `../output-lifecycle-pg.log` |
| 真实 PG output_lifecycle | 6 passed / 0 failed / 0 ignored，5.62 秒 | 同上 |
| cleanup 最后调整后的 no-run | exit 0，54.14 秒 | `../output-lifecycle-fence-compile.log` |
| cleanup 受影响单项重跑 | 1 passed，2.58 秒，exit 0 | `../output-lifecycle-fence-pg.log` |

唯一 lifecycle 测试数为 **18**，单项重跑不另累计。18 项全绿发生在最后 cleanup-only 调整之前；调整后重跑的是受影响的一项。完整组合 workspace 尚未验证完成，不能宣称全包通过。精确命令、日志哈希与两测试文件 SHA256 见 [RESULT.json](RESULT.json)；原证据 [output-lifecycle-result.json](../output-lifecycle-result.json) 不覆盖。

## 下一轮优先问题：Pipeline stack overflow

Root 请求的 20 项 Pipeline integration suite 在打印 3 个 named `ok` 后，由
`channels::pipeline::integration_tests::text_delivery::a_large_provider_chunk_splits_only_at_utf8_boundaries_under_one_delivery`
触发 `stack overflow`，进程 SIGABRT（signal 6）；Root session `71344` 已 exit 101。没有完整 suite 汇总，其余用例不能计作通过或逐项失败。

原日志 `run/validation-20260922/integration/pipeline-first.log` 保持不动，逐字节副本为 [pipeline-first-preserved.log](pipeline-first-preserved.log)。**根因尚未诊断**；本代理遵守暂停要求，未启动复现或排查。恢复工作后先处理此问题，再执行完整整合验收。

## 资源核对和精确清理

本代理 18 项测试及最终单项完成时，schema 与 `run/output-lifecycle-schemas-after.txt` 基线完全一致，其他客户端连接为 0，PG 窗口已释放。之后 Root 明确确认只有其 Pipeline suite 真正使用此库，A/C 未开始 PG；中止进程已经退出。

按该基线重新只读核对，恰有两个新增 `pipeline_delivery_*`，没有缺失 schema、没有其他客户端连接。只对这两个可确定归属的 schema 执行 DROP，各 exit 0：

- `pipeline_delivery_387dd79d7b04433ba541aedb0740ccd7`
- `pipeline_delivery_a4098bb5d5444dd5a0e965912abcdef7`

清理后 namespace 与原基线逐字节相同，其他客户端连接仍为 0。原三个历史 schema 全部保留：

- `pipeline_delivery_12873bb2547946af85f998a91746a5a4`
- `pipeline_delivery_5d85b6229c81441fb5e38a8000f7037f`
- `pipeline_delivery_c7794da066664a34901bca6d85453375`

差集、连接采样、每项 SQL/exit/stdout/stderr 均保存在本目录，汇总见 [root-abort-cleanup.json](root-abort-cleanup.json)。仅访问 `alia-refactor-test-db` 的 `alia_refactor_delivery_final`（任务 32768），未触碰用户 5433、其他容器或其他 schema。共享测试容器保留；本代理没有活动 exec、测试或自建容器。

## 恢复后检查清单

- [x] 两个固定账本提交已合入，13/13 真实 PG 证据保留。
- [x] 两份 shared 测试冻结，18 unique 全绿及最后单项复核已记录。
- [x] Pipeline SIGABRT 原日志保留，故障与未验证边界明确。
- [x] 仅清理两项已确认 Root abort 残留；保留历史基线、无活动客户端。
- [ ] 用户恢复工作后优先定位和修复上述 stack overflow。
- [ ] 接续 A/C/Root 各自交接中的 actor/live/paired transport 验收，未运行部分仍保持未完成。
- [ ] 冻结完整组合源码并完成 workspace、特性组合和必要真实集成测试。
- [ ] 审阅 shared dirty 归属后由 Root 正常提交；不得把临时 Core overlay 混入。
