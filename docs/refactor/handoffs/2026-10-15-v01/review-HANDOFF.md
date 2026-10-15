# V01 历史身份与独立审查 HANDOFF

日期：2026-10-15。本轮基线 `c685e5a`，分工 `settings_review`，续接审查 `v01_review`。本分工只读代码、历史日志和当前已存在的二进制并计算哈希，不运行 Cargo、测试二进制、数据库、Git 写入、SSH、模型或硬件。Root 统一执行本轮复现与提交；本页不把历史通过当作当前验证。

## Checklist

- [x] 读取 R04 V3 初次失败、随后复跑、V4 与 10/04 的原始证据。
- [x] 重核相关历史日志/结果哈希，区分 binary 路径、内容哈希与源码 manifest。
- [x] 有界检查现存原 V3 候选、V4 和 10/04 诊断 binary。
- [x] 对照 V3→V4 与 V4→当前相关代码，列出不允许混称同源的变化。
- [x] 给出有界停止条件、最少诊断和未证明的边界。
- [x] 核对本轮实际结果、最终差异与任务资源回收，并保留未采集最终即时数据库快照的边界。

## 初次失败与后续通过

| 证据 | 实际结果 | 能说明与不能说明的内容 |
| --- | --- | --- |
| 09/28 `workspace-v3` | `cargo test --workspace --locked --offline`，exit 101；到第 27 目标共 699/3/3，`duplex_playback-e65e790efe7810e2` 为 6/3/0，套件 35.54 秒 | 三个 Companion 场景都在原 `companion.rs:492` 报外层 25 秒超时；原日志没有具体 phase、ScanProgress 或 retirement reason，不能归因到 scanner、短准入、TTS、CPU、PG 或时钟 |
| 09/28 `paired-one-v3` | package-only 的单例通过，测试 9.49 秒 | 使用另一 binary fingerprint `a8401cddf3efdf70`；不等同 workspace 原失败执行 |
| 09/28 `paired-workspace-binary-v3` | 记录的原 workspace binary 路径随后默认并行 9/0/0；测试 13.65 秒、命令 13.69 秒 | 历史记录支持当时原路径复跑未再现；本轮已经无法重新核实原 V3 binary 内容，不提升为当前同 binary 实测 |
| 09/28 V4 | workspace 1053/0/5，源码 manifest `35efd180…` | 增加失败诊断后通过；没有修复原超时的证据 |
| 10/04 V4 retained 三批 | 分别 9/0/0；命令 19.69 / 15.31 / 15.45 秒 | 27 次执行仍只有 9 个唯一用例，不证明原 V3 根因已解决 |
| 10/04 新诊断 binary | 9/0/0，命令 17.73 秒、测试 14.21 秒 | 三个匿名 admit attempt 为 Fresh；剩余预算 1924 / 1939 / 1955 ms，snapshot 7 / 5 / 3 ms、freeze 3 / 1 / 1 ms、admit 66 / 51 / 50 ms；这些成功样本不能解释初次失败，也不能按日志顺序擅自绑定具体场景 |

本分工重新读取 [R04 原失败说明](../2026-09-28-r04/device-health.md)、[R04 原始索引](../2026-09-28-r04/runtime-evidence-index.json) 与 [10/04 runtime](../2026-10-04/runtime.md)。6 份 R04 相关原始 log/result/count 和 9 份 10/04 baseline/diagnostic/binary-receipt 文件共 **15/15** 匹配各自归档索引的 SHA-256；原始失败没有被后续通过覆盖。逐项路径、字节数、预期/实际哈希和有界检索范围见 [本轮历史审计 JSON](review-history-audit.json)。

## 当前可核验的 binary 身份

| 文件 | 本轮实际字节数 | 本轮 SHA-256 |
| --- | --- | --- |
| `run/validation-20261004/duplex-playback-v4-retained` | 107330192 | `e529ff72ad2e1805040a07291cf152f1625b6c38c9aea4400ba35a801d6fe087` |
| `run/validation-20261004/duplex-playback-review-v1` | 111367904 | `32a8c0f51c9df30ea343e4f1bc5bc821882f93547515aec23b888051e0dc2a59` |
| 初次审计读取时的 `target/debug/deps/duplex_playback-e65e790efe7810e2` | 109467984 | `4384319fac0c4b5e99fdab7bdf800fcfd69f97b540f73e84d54498c3bdbe24ba` |

本轮有界检索并哈希了 10 个 macOS 命名候选：当前 `target/debug/deps/duplex_playback-*` 的 7 个可执行文件、旧 integration worktree 的 1 个、10/04 保留的 2 个；各文件哈希期间 size/mtime 均未变，没有匹配历史 V3 SHA `b30c9aa9ac17f9fc8f5ab3b106f7945dad8c76f2cdc983e323170479d6151059` 的文件。该结论限于上述明确范围，不称对整台电脑或备份做了穷尽搜索。

原 V3 SHA 本轮只从 10/04 `runtime.md` 的历史引述取得；09/28 `paired-workspace-binary-v3-result.json` 保存了命令路径和源码 manifest，没有独立 binary SHA。不能声称本轮重新验证了原 failing V3 binary，也不能通过再次编译旧源码恢复原冷负载、工具链和依赖链接身份。V4 retained 的全部构建输入与 artifact 之间仍按历史说明限定，不能用“文件名未变”推定它是 V3。

## 源码身份与相关变化

R04 V3 manifest 为 `b3ff3f99531932bd169d9c73b92cc16d7d6b341e655d37e68b972df3e913897a`，V4 为 `35efd1807b5c2820b3024fc6142bf769ace5973cc13275e0b5649f8bec011a8b`。两份都是 633 项输入，逐项对照仅 `packages/alia-server/tests/duplex_playback/companion.rs` 的内容哈希不同：`b0ceecdb…` → `b26fe000…`，符合只增加固定 stage 与最多 1 秒失败摘要的历史记录。本轮还核验 `run/validation-20260928-r04/linux-arm64-v4/source` 的 **633/633** 文件全部匹配 V4 manifest，可将这个源快照用于代码对照；这不是对 Mac binary 的可复现构建证明。

V4 至本轮基线已经有后续正式重构，因此当前测试通过不能称为 V4 同源复跑。相关差异包括：

- 10/04 runner 增加 snapshot / freeze / admit 的固定阶段、同进程 monotonic 耗时和原 UTC deadline 剩余预算；fixture 增加限定 namespace 日志及数据库 state/wait_event_type 分组。
- Conversation/Memory 从 provider 归属迁移；Companion 的来源读取归于自身模块，公共 authority 通过 `lock_appraisal_authority` 使用，邀请 Context 与事务边界按新归属重排。
- runner 使用 `ResponseCommitter` 提交固定邀请历史；Delivery 的事务 prepare 改为显式使用传入事务。保留原事务原子性不能直接推导耗时恒等。
- Voice ingress 从旧 `ServerAudioIngressService` 变为 `VoiceIngressAuth`，fixture 显式发送 `x-alia-channel-id`；出站 transport、audio budget 与 Builder 依赖归属已改变。
- 同目标中的其它播放测试后来改为等待 durable resume receipt 和 producer 本地 gate 都完成；这一夹具改动不能当作三项 Companion 原始超时的修复。

逐字节对照中，V4 与当前的 `grpc/voice_session/initiative.rs`、Companion `decision.rs`、`service.rs` 及 `m20260926_000015_companion_live.sql` 保持相同。相同局部文件不等同全程序、二进制或运行环境相同。

## 有界复现与停止条件

本轮有界计划由 Root 固定为：原基线完整 workspace 构建后，同一实际 binary 默认并行运行两批完整 `duplex_playback`；若均过，只补确认的测试诊断盲区，最后一批来自诊断版完整 workspace 验证。两种源码 manifest 和 binary 必须分别记录，最后一批不称为原基线同源复跑。任一批复现就停止重复空跑，保存失败现场后按明确阶段追查；有界通过就收口“本轮未复现，V01 继续开放”。不因当前通过继续增加轮数求确定性，不延长 25 秒、不减少并发、不 ignore，不通过刷新机会或延长原 admit_before 掩盖失败。

本分工已读取 `run/validation-20261015-v01/baseline-{binary,summary,paired-01,paired-02,resources}.json`，并重哈希保留的 baseline binary 与源码 manifest：binary 为 `f1c4756de285b49cf183acba7d4db6069ad7d012dd33e98c09e6f8ebc183539e`（109011920 字节），manifest 为 `2258c3ecfac2bc19caf2ea2bf3b76f8c640b1d8da18833e99da0a4e8d39e2535`。两批分别 **9/0/0，14.40 / 13.28 秒**，`source_drift=[]`、`binary_unchanged=true`；18 次执行仍是 9 个唯一用例。资源核对记录 fixture schema=0、其他 client connections=0。匿名并发阶段不能对应到具体场景。

这是本轮新构建后的内容身份，不能与上表初次只读审计时的可变 target 路径内容合并。最终报告回归与 workspace 证据已复核，详见文末；不增加复现批次。

现有最少阶段证据已经覆盖：固定 test stage；shutdown 前的 ScanProgress；Appraisal/Response 数量；pending/admissions 按 state 与 retirement_reason 分组；任务数据库 state/wait_event_type；runner snapshot/freeze/admit 的 monotonic wait、original admission remaining 与 outcome。失败摘要仍最多 1 秒，不打印来源原文、用户/设备/来源身份、private grant、原始 SQL 错误。诊断查询的耗时是 pool+transport+SQL 合计，不可声称单独测到 pool 等待；Server UTC 剩余预算不是数据库或设备间时延。

真实 PostgreSQL 的 `commit_checks_original_short_admission_deadline_without_refreshing_output_deadline` 已能证明过期拒绝契约，可作为有界对照；人为持锁到过期只证明该拒绝机制，不能凭相同“最终没输出”回推历史 V01 根因。原成功断言仍要求一次 TTS、一个原 Response/Admission、原媒体与精确取消清理，不把退休改成成功。

Root 应另行保存本轮运行结果与 cleanup/join、schema/连接回收证据。没有当前失败现场时，不新增生产重构、无限观测框架或推测性时间政策变更。R03 继续暂停；默认 shadow、执行批准、硬件与依赖边界保持。

## 本轮诊断补丁独立复审

本分工在 fixture agent 冻结后重核 `companion.rs` SHA 为 `5565c1262683334b86209eb038d144c5dcc9e20a21dbc3e0aed797d5744a88b4`。逐项重哈希 baseline manifest 的 669 个输入，只有该测试文件由 `56629b6a…` 变化；生产、依赖、迁移、公共 fixture 并发与其它测试输入未变。新增测试代码动态结果仍由 Root 最终 workspace 提供。

- filter 只增加 `companion::service=warn`，核对该模块现有 warning 均为固定文字及静态 stage/class，不打印 owner、source、grant 或原始 SQL 错误；继续以 `off` 为默认，不继承广域 `RUST_LOG`。
- `failure_state` 仍只有一个最多 1 秒的 SQL future，接收现有 db 与实际 `ScanProgress`。后者以 `shadow_scan` 标记，避免把 shadow 进度误当 live 扫描成功；`impl Debug` 只用于测试内部复用，没有扩张生产 exports，两个调用点都传入现有匿名进度值。
- 新增 source claims 按数据库约束的 `origin_mode`（仅 shadow/enabled）分组、rejected sources 总数；其余只保留状态与计数。错误一律返回固定 `unavailable`，超时返回固定 diagnostic deadline；没有凭私有身份把并发日志串联成伪精确追踪。
- 内层 25 秒 future 的 timeout 和 catch_unwind panic 均在 `stop.cancel` 之前获取并打印摘要；成功路径不查摘要，原 panic 仍在 cleanup 后重抛。原 heartbeat cancel/join、TTS/resume 屏障释放、Runtime shutdown、voice/paired close 和清理后的撤销断言保持顺序。初始化失败、外层 45 秒截止和 cleanup 自身失败不因此获得完整动态见证。
- 新增真实 PG 用例写入一条 enabled claim 和一条 rejected source，在 shadow 计数仍为零时直接读取同一 helper；检查固定计数和 owner/source/appraisal/generator 私有值不出现在报告里。随后仅在自己的随机 schema 内改列名，确认查询错误被映射为固定 `unavailable`；由既有 `with_db` 清理整份 schema。没有启用 scanner、输出、模型、硬件或外部服务。

这是一项已确认的诊断盲区补齐，**不是 V01 超时根因修复**。原 9 项业务场景与新增 1 项诊断用例分别计数；最终诊断版 binary 与原基线 binary 不混作同源证据。

## 最终证据独立核对（续接）

续接仅进行文件读取、静态差异检查与 Python 标准库哈希/日志计数，没有重新执行 Cargo、测试二进制、PG 或设备。重新读取历史审计列出的 15 份原始文件，**15/15** 仍匹配归档 SHA；最终 manifest 的 **669/669** 输入均匹配当前文件。baseline→final 的路径集合相同，唯一内容变化仍是 `packages/alia-server/tests/duplex_playback/companion.rs`（最终 SHA `5565c1262683334b86209eb038d144c5dcc9e20a21dbc3e0aed797d5744a88b4`），生产代码、依赖、迁移与公共 fixture 未变。

| 本轮身份/执行 | 独立核对结果 |
| --- | --- |
| baseline manifest | `2258c3ecfac2bc19caf2ea2bf3b76f8c640b1d8da18833e99da0a4e8d39e2535`，669 输入 |
| baseline retained binary | `f1c4756de285b49cf183acba7d4db6069ad7d012dd33e98c09e6f8ebc183539e`，与两批执行记录一致 |
| final manifest | `27d16d5b28bc4b0dcc60a92b2522d72a330fb56e2da9a1ace2483835ad769093`，669 输入 |
| final retained binary | `c26bf833583ad6b9dd8b737d2ddef8414d86ed498b49137d163658d51458fa33`，109059760 字节，workspace 结束后留存 |
| final workspace | `cargo test --workspace --offline --locked --no-fail-fast`，exit 0，436.72 秒，`source_drift=[]` |
| final 原始日志计数 | 72 个 Cargo target（67 Running + 5 Doc-tests），**1245 passed / 0 failed / 5 ignored / 0 filtered**；按各 target 最终摘要计数，不重复加入子进程重入或基线复跑 |
| final duplex 目标 | **10/0/0**，13.77 秒；9 个原业务场景 + 1 个新增失败报告用例 |

5 个 ignored 均为已有项目：3 个真实 PTY 用例、1 个真实 Easy-Turn adapter 用例、1 个真实 Easy-Turn voice-stream 用例；本轮没有新增或扩大 ignore。原 `commit_checks_original_short_admission_deadline_without_refreshing_output_deadline` 也在 workspace 日志中通过，仍仅说明过期拒绝契约。

复核 `validate.py` 与命令记录：执行环境清除 `RUST_TEST_THREADS`，所有三批均未传 `--test-threads`；现有 fixture `CASES=3` 未变。`CARGO_BUILD_JOBS=2` 仅限定构建进程，不改变测试线程。业务 25 秒、外层 fixture 45 秒、失败摘要最多 1 秒均未延长；没有刷新原 admission deadline。三批已经达到本轮上界，不继续用空跑累积“确定性”。

本轮未再现旧超时；保留 **V01 open**。成功结果不能恢复缺失的原 V3 binary、冷负载或运行环境，也不能把这项测试诊断补齐写成超时根因修复。此次成功路径实际执行了原 cleanup 与取消断言；新增报告测试覆盖匿名计数与 SQL 错误脱敏，但没有故意注入整条 Companion 超时/断言失败，因此不声称失败全路径已经得到动态证明。原初始化失败、外层 45 秒截止和 cleanup 自身失败仍在本轮诊断覆盖之外。

已读取 Root 的 `final-resources.json` 和 `cleanup.json`，并与 `database-create.json` 对照完整任务容器 ID。恢复时共享 OrbStack 已停止且 Docker socket 不存在；Root 恢复共享 runtime 后，任务容器仍为 stopped，没有重启 PG 或测试。当前相关测试进程列表为空。**最终测试结束当时的 schema/连接快照没有留存**，`final_fixture_schemas` 与 `final_other_client_connections` 均为 `null`；不得用基线的 0/0 或恢复后的停止状态补写为最终即时 0/0。

资源清理记录确认完整任务 ID 与 `alia.validation=v01-20261015` label 一致，随后只对该任务容器执行 stop、确认 stopped、`rm -v` 并复查不存在，全部命令 exit 0。共享 runtime 恢复时既有 `astrbot` 自动运行，Root 留其运行，没有修改配置或单独停止它。因此可以确认任务容器已回收、当前没有相关测试进程；不能据此补证缺失的测试结束即时 schema/连接观测。该证据缺口已明确保留，无需重启数据库或增加测试批次来伪造即时快照。

最终归档追加复核：[evidence-index.json](evidence-index.json) 列出的 **26/26** 文件与 `run/validation-20261015-v01` 原件逐字节一致，字节数与 SHA-256 全部匹配；源码提交收据 [commit-verification.json](evidence/commit-verification.json) 记录源码提交 `f81762395de0ad75efa9c1e7afe60f46b620cac1` 的 669 项输入吻合、无新增或缺失输入。该收据由 Root 的 Git 对照产生，本分工未重复执行 Git 提交校验。两份 retained 可执行文件留在 ignored `run/`，Git 仅归档身份收据，不能把收据视为二进制副本。
