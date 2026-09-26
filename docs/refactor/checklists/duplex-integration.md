# 可恢复暂停纵向整合 checklist

更新：2026-09-26。**DU01—DU11 完成（11/11）。** 已通过的 V2 候选于 `948b3c4` 合入主重构分支，合并后逐项核对 605 个构建输入，哈希全部一致；仅解决三处文档冲突，没有源码冲突。历史验证为 macOS workspace 967/0/5（另补 5/5 真实 PTY/模型）、Linux ARM64 定向矩阵 325/0/0；未重复运行不变输入。合并证据见 [DU11 result](../handoffs/2026-09-23/du11-merge-result.json)。下一阶段为 [R01 Companion checklist](companion.md)。

本表是当前任务入口，旧 [2026-09-20 检查点](../resume-checkpoint-2026-09-20.md) 及四份 HANDOFF 作为历史证据保留。只有满足完成条件并写入证据后才能勾选；编译、测试运行、平台验证和真机验收分别记录。

## 分工与修改边界

| 负责人 | 工作树 / 分支 | 独占内容 | 子 checklist |
| --- | --- | --- | --- |
| A `/root/body_endpoint` | `alia-duplex-control-protocol` / `codex/duplex-control-protocol` | Core/Proto/Embedded；Core `turn.rs` 除外 | `duplex-protocol.md` |
| B `/root/body_server` | `alia-duplex-control-ledger` / `codex/duplex-control-ledger` | 控制 Repository、迁移 000013、真实 PG 用例 | `duplex-ledger.md` |
| C `/root/text_delivery` | `alia-duplex-media-budget` / `codex/duplex-media-budget` | TTS/HTTP 预算、AudioBuffer 调用端 | `duplex-media.md` |
| Root | `alia-duplex-integration` / `codex/duplex-integration` | 原 output owner、DeliveryService 装配、Pipeline/传输整合及完整验收 | 本表 |

A/B/C 原独立子包已提交并合入。后续三组在整合树内按文件独占协作：A 为 Server Voice/Config/Bootstrap 与 `duplex-voice.md`；B 为 `tests/output_lifecycle.rs`、既有 DeliveryService 夹具；C 为 `tests/duplex_playback.rs`、配对测试依赖及公开 backend trait 必要构造入口。Root 不在这些文件并行编辑。

根目录均为 `/Users/ke-qiao/Developer/project-alia/run/worktrees/`。A/B/C 在各自树修改、验证并提交；Root 逐次合入固定提交。B 的旧 Core 和旧 Root 的 Core/Proto overlay 继续独立保留，不复制为最新版、不混入无关提交。

## DU01—DU11 实施与通过条件

- [x] **DU01 恢复核对（Root）**：读取检查点及四份 HANDOFF；核对主分支 `e156531` 干净，A `2619a28` / B `76f5b04` / C `779f4f8` / Root `e94f66e` 均存在；A/C 干净，B/Root 仅剩已记录 overlay。2026-09-22 `git status`、`git worktree list` 及四树状态读取已完成。
- [x] **DU02 隔离组合（Root）**：已创建 `codex/duplex-integration`；顺序组合 `2619a28`、`76f5b04`、`779f4f8`、`e94f66e`，再合入 A `7cc9a84`（含 `7bea321`）、B `4846b4a` / `56e9acc`、C `5333eff`。交叉修改先保存 Root 补丁再合并并恢复，旧 overlay 独立保留。整合 Server lib 已编译通过；完整验收仍见 DU10。
- [x] **DU03 控制 owner 生命周期（Root）**：独立有界控制入口、原 producer/media owner、JoinSet、启动/关闭、取消/到期/队列容量；新增 actor 真正装配进 DeliveryService，相关生命周期测试通过。
- [x] **DU04 原规范身份（Root）**：request ID 在 admission 固定，贯穿 VoiceInput/VoiceTurn；原 Response/Context prepare 后登记；first TTS/Start 前绑定；排队 Drop 和原生尾部有真实所有权测试。
- [x] **DU05 两类暂停语义（Root）**：voice v2 能力检查；持续采集/auth 不等 SQL/RPC；fresh evidence、临时附和恢复、明确继续、真正新轮次/停止的 generation 转移；Evidence 不能自行打开设备门。
- [x] **DU06 生产背压（Root + C）**：同 Response 持续持有预算；Held 不启动新 TTS；保留在途有限请求和 stream，不重生成/重派发；无等待期间长期 PG 事务。
- [x] **DU07 控制竞争与未知事实（Root）**：Bind 超时/取消，短语义期限变化，未知 ACK/Observe，迟到事实，新语音与 Resume，backend/实例变化均有针对性回归；不把 Unknown 伪造为 Rejected。
- [x] **DU08 子包验收（A/B/C）**：A Ready/原 Binding 竞争；B 后加入 Delivery deadline、snapshot 上界、真实关系来源及实际 PG；C 所有调用端编译、真实 HTTP 与额度生命周期；对应子 checklist 及固定提交齐全。
- [x] **DU09 组合故障测试（Root）**：真实两端 gRPC 的队列满、Start 前、End 后尾部、超 media idle、鉴权撤销/刷新、丢 ACK、旧恢复与捕获屏障；无新增 Response/history/TTS/媒体 owner。
- [x] **DU10 完整验证（Root）**：默认 workspace、两端无默认 all-targets、适用 Linux ARM、真实 PTY/模型；源码 manifest 无漂移，失败与 ignored 单列；旧 WAIT 模型用例已符合完整新协议。
- [x] **DU11 完成提交（Root）**：通过以上出口后才合入主重构分支；更新协议/操作文档、progress、计划和机器可读状态；清理仅任务自有临时执行资源。

## 必须保持的行为

- [x] 同一 hold 的原 Response、Context、source、双 epoch、backend、播放游标和媒体 owner 始终不变。
- [x] Fresh SQL permit 才能首次发送；同 ID 查询、未知提交、重启不授权重发媒体或控制。
- [x] 原命令 outcome 与当前设备 snapshot 分开；旧回执不能让已退休输出复活。
- [x] 临时附和只能恢复 AcousticCandidate；ExplicitWait 需要明确 Continue。
- [x] 工程默认 hold 60 秒、硬上限 120 秒、临时语义期限 3 秒；受原输出剩余寿命限制；Confirm/重试不延长硬期限。
- [x] 单 encoded payload 1 MiB、同 Response 保留 8 MiB、进程全局 256 MiB；应用复制前取得额度，实际保留期间不提前释放。
- [x] 取消与记录已经发生的晚事实不要求旧 Context 仍有效；Resume 必须通过原完整权限/根版本检查。
- [x] 软件门、模型标签、后端接纳与物理 Presented/AEC 各有独立证据；不能混称完成。

## 验证日志

本轮最终证据目录：`run/validation-20260923/`；前轮历史目录为 `run/validation-20260922/`；各 agent 也在自己的 `run/` 保留命令、exit、源码清单。任务 PG 端口 32768，B 初始独占测试窗口；不操作用户 5433 或其他业务容器。完整 schema 测试需协调并发，不能以串行所有行为掩盖真实竞争。

| 时间 / 固定源码 | 命令或证据 | 结果 | 适用范围 |
| --- | --- | --- | --- |
| 恢复基线 `e6ba75b` | 旧 `duplex-stage1-integration` 证据 | workspace 875 pass / 0 fail / 5 ignored | 历史基线，尚非本轮新源码 |
| 2026-09-22 | 四份 HANDOFF、Git 状态读取 | DU01 完成 | 仅恢复核对，无新源码测试结论 |
| A `7cc9a84` | `duplex-protocol.md` 与 A `run/duplex-ready-validation` | Ready/Binding 4 red→8 green；Embedded lib default/no-default 各 153 pass；两种 all-targets exit 0 | 原 Binding ABA 与 capture owner；真实 Sherpa，无真机声明 |
| B `56e9acc` | `duplex-ledger.md` 与 B `run/validation-output-control-20260922` | 真实 PG 13/13；后加入 Audio 期限跨 Resume 保留 | SQL 来源、permit、迟到事实与固定期限 |
| C `5333eff` | `duplex-media.md` 与 `run/duplex-media-validation` | 59 定向 pass；default/no-default all-targets exit 0 | 编码预算、真实 HTTP、原 Context/Response→TTS 顺序 |
| 整合工作树（未冻结） | `run/validation-20260922/integration/check-after-subpackages.log` | Server lib check exit 0，17.33s | 仅该次编译；后续编辑仍需完整复验 |
| 整合工作树（未冻结） | `run/output-lifecycle-pg.log` | DeliveryService 12 + Output lifecycle 6 = 18 pass / 0 fail / 0 ignored | 控制丢 ACK、原队列所有权、期限、Held heartbeat；cleanup 新断言待复验 |

## 后续任务接续

细分出口与准确进度见 [R01—R06 后续 checklist](next-round.md)，当前 0/6 完成，不能将 DU 完成视为整个重构完成。

- [ ] 暂停包完成后：Companion shadow/回放与受约束主动表达。
- [ ] AEC、时钟映射/漂移、Mac/Pi 双讲与暂停听感。
- [ ] D3 动作资产、执行器反馈/限位、真实位置与呈现证据。
- [ ] 首轮 CLI/REST 管理/诊断、选定组件及其他 Server 平台收敛。
- [ ] Mac + Pi 真机全新安装、故障恢复及至少 24 小时混合负载。

Web/iOS/Android 与通用 Embedded 适配继续按 [延期台账](../deferred.md) 留待后续。再次暂停时，把当前分支 SHA、dirty 文件归属、活动 exec/容器、未过测试和下一条 checklist 一起记录，避免仅留下笼统百分比。

## 已处理的整合风险（最终矩阵单列）

- [x] 独立控制队列与 output JoinSet 已实际接线，首次 Server lib 编译通过。
- [x] Audio 入队前保留原 MediaOwner；已完成请求的幂等查询仍返回原事实。
- [x] SQL command/hold 期限与整个 output 期限分别收紧，恢复不丢失后加入 Audio 的原硬期限。
- [x] scope fence 超时仍保留同一个 native future；shutdown 不能提前报成功。永久退休先与该 fence 并行持久化，最后一项 pending 阶段断言正在复验。
- [x] Start 真正准备好后才启动上传 handshake 时钟；等待双端 gRPC 回归。
- [x] RegisterOutput 的本地入场身份在 Fence 时删除；旧 ACK 不能回填 cache，等待配对故障测试。
- [x] 原 LLM/TTS 在途 Future/结果保留、Held 禁止下一次 TTS 的真实 PG 流水线回归；最终 21 项 Pipeline 已在 V1 完整 workspace 再次通过。
- [x] 对已绑定 voice scope 的生成错误，只取消原输出；语音错误提示如需另一个 Response，必须由新的 generation admission 明确授权。本轮不在原 scope 自动生成替代 Response/TTS。未绑定前的固定 source notice 仍走规范 Context/Response。

## 2026-09-22 暂停收口（当时状态，保留原记录）

- [x] 用户明确提出额度暂停；三个 subagent 停止新增实现与测试，分别写最终 HANDOFF。
- [x] Root lib-test no-run 编译成功（39.77s）；不代表所有最新文件已经运行通过。
- [x] B cleanup 并行修复后的 fence 用例再次真实 PG 通过；仍计入原 18 unique，不重复计数。
- [ ] **首要恢复项：Pipeline 栈溢出。** `pipeline-first.log` 启动 20 项，仅 3 项明确完成，随后 `text_delivery::a_large_provider_chunk_splits_only_at_utf8_boundaries_under_one_delivery` 栈溢出、SIGABRT、exit 101；没有完整结果表，未开始修复。先定位 Future/调用栈问题，再按默认设置运行回归。
- [ ] A 新 Voice actor 与 real-model canonical fixture 完成实际验证；见 voice HANDOFF。
- [ ] C 5 条配对 gRPC 测试已编译/链接，实际运行 0 条；旧 scope cleanup/session replacement 风险尚待最小复现。
- [ ] 全部冻结输入的完整 workspace / 平台 / PTY / 模型验证，以及主分支正式合入，尚未执行。

完整保存提交、manifest、资源清理和四份交接见最新检查点。下一次从该检查点开始，不重做 DU01/DU02，也不覆盖旧树保留的 overlay。

## 2026-09-23 恢复分工

- Root：归档暂停快照后复现/修复Pipeline栈溢出；负责整合与最终冻结验证。
- `/root/voice_resume`：接续A Voice actor/live fixture与v2语义验证。
- `/root/lifecycle_review`：独立审查Future栈尺寸及输出生命周期，配合回归。
- `/root/playback_resume`：运行5条真实paired gRPC及替换session清理回归。

### 本轮增量 checklist 与证据

- [x] 默认栈重现原 SIGABRT；保存原/修改后机器码 prologue。`cancellable_stage` 在构造 async 状态前固定同一个 boxed future，不修改线程栈大小、不重建生产操作。
- [x] 默认栈原单例 1/1（3.54s），既有 Pipeline 整套 20/20（7.78s）。日志为 `run/validation-20260923/pipeline-stack-{red,green,suite}.log`；实际运行 binary SHA-256 `75aef77c31cf1e97f9469e7f89163f0832960296eaf5a6a9ed08ac8f4eaabc9d`。该次直接运行已编译目标，不覆盖随后新增的 retirement 协议与 error guard 用例。
- [x] queued Bind、Register ACK 等待中取消、语义期限到期三项真实 PG 先红；固定候选身份与设备 registration 分离后，生命周期共 11/11（6.49s）。另覆盖外部取消入口及不属于当前请求的 Response 保持原状态。证据 `run/validation-20260923/lifecycle-{bind-red,final}.log`。新 retirement fixture 接口仍须复验。
- [x] Voice actor 首轮 20/23，失败 3 项是虚拟 Tokio 时间推进了真实 PG 恢复 SQL 的期限；改为私有短真实检查间隔后 23/23（3.26s）。生产权限复核间隔仍 30 秒；短周期测试不声称执行了五分钟墙钟验收。
- [x] 配对 gRPC 首轮真实运行 1 pass / 4 fail；新增 replacement 第六项在原媒体 setup 失败，尚未到 replacement 断言。收紧 cleanup 后不吞 shutdown 错误。
- [x] 后续精确诊断发现首个 reservation 使用 `fetch_update` 返回的旧序号 0；已改为返回新序号 1。原失败日志 `run/duplex-paired-media-early-result.log`，后续 paired 六项全套及 V1 workspace 已通过。
- [x] 新 `RetireOutput` 按原 scope/binding/backend 保留退休事实；旧 session 的 native 写入收尾不能由“已有新 session”代替证明。Core/Proto/Embedded/Server 与配对、native/body 错误回归已通过；旧 scope 不能由 Ready 重新激活。
- [x] prepared Response 尚未收到 Register ACK 时发生错误，仍只取消原 Context/Response，不生成另一 source notice；真实 PG 红绿回归及 21 项 Pipeline 已通过。
- [x] 官方 Easy-Turn 音频 + 真实 Server voice/SQL 控制账本定向复验通过；首次 8 秒模型超时保留，V2 两个 ignored 目标另行补跑。
- [ ] 所有新源码冻结后执行完整 workspace、无默认 feature、Linux 与适用模型/PTY 验证，再完成 DU11。

每个日志只证明实际受测的源码/二进制；初始失败与修复后结果分别保留。主 checklist 不因编译通过或局部套件通过而提前勾选。

## 2026-09-23 V2 最终矩阵

- [x] V1 完整默认 workspace，964/3/5，exit 101；两 Embedded 重用退休 scope、一个 Runtime 缺原 producer 的旧夹具失败已定位并保留。
- [x] 仅两测试文件修正，生产输入不变；新 manifest 与独立只读 snapshot 已建立，不覆盖 V1。
- [x] 新身份夹具 8/8、Runtime 43/43；默认限时及故障断言保留。
- [x] macOS 两端 no-default all-targets exit 0（57.30 秒）；27 个变更 Rust 文件 rustfmt 检查 exit 0。
- [x] V2 完整默认 workspace：967 passed / 0 failed / 5 ignored，203.85 秒；Ignored 另行 5/5 通过，不重复累计。
- [x] V2 三个真实 PTY 与两个真实模型用例，3 + 2 passed、0 failed；原期限保留，无额外预热，不宣称冷启动。
- [x] V2 Linux ARM64 检查、Embedded 全库、受影响 Server PG 矩阵与 Runtime 全套，共 325 unique passed / 0 failed。
- [x] 归档同一输入的汇总证据、正常提交并合回主重构分支。

## 2026-09-23 额度暂停最终状态（历史，已于 9 月 26 日恢复）

用户明确要求停止，三个 subagent 均 completed，活动 exec 为 0。DU **10/11**、剩余 **1**；R01—R06 **0/6**、剩余 **6**。源码 `5810687` 与栈修复 `9c4f5b0` 已正常提交，完整 V2 验证已归档，正式合入留待恢复。下次首项是 DU11，不重新开始 DU01，也不重复已通过且输入未变的矩阵。
