# 可恢复暂停纵向整合 checklist

更新：2026-09-23。**用户已恢复执行；2026-09-22暂停源码已保存为 `ae75a4e`，未经完整验收。** 最新接续入口为 [2026-09-22 检查点](../resume-checkpoint-2026-09-22.md)。主 DU checklist 完成 2/11、待完成 9/11；3 个原组件包已验证合入，运行时 18 个不同测试通过，最新 Pipeline 套件因栈溢出中止。

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
- [ ] **DU03 控制 owner 生命周期（Root）**：独立有界控制入口、原 producer/media owner、JoinSet、启动/关闭、取消/到期/队列容量；新增 actor 真正装配进 DeliveryService，相关生命周期测试通过。
- [ ] **DU04 原规范身份（Root）**：request ID 在 admission 固定，贯穿 VoiceInput/VoiceTurn；原 Response/Context prepare 后登记；first TTS/Start 前绑定；排队 Drop 和原生尾部有真实所有权测试。
- [ ] **DU05 两类暂停语义（Root）**：voice v2 能力检查；持续采集/auth 不等 SQL/RPC；fresh evidence、临时附和恢复、明确继续、真正新轮次/停止的 generation 转移；Evidence 不能自行打开设备门。
- [ ] **DU06 生产背压（Root + C）**：同 Response 持续持有预算；Held 不启动新 TTS；保留在途有限请求和 stream，不重生成/重派发；无等待期间长期 PG 事务。
- [ ] **DU07 控制竞争与未知事实（Root）**：Bind 超时/取消，短语义期限变化，未知 ACK/Observe，迟到事实，新语音与 Resume，backend/实例变化均有针对性回归；不把 Unknown 伪造为 Rejected。
- [ ] **DU08 子包验收（A/B/C）**：A Ready/原 Binding 竞争；B 后加入 Delivery deadline、snapshot 上界、真实关系来源及实际 PG；C 所有调用端编译、真实 HTTP 与额度生命周期；对应子 checklist 及固定提交齐全。
- [ ] **DU09 组合故障测试（Root）**：真实两端 gRPC 的队列满、Start 前、End 后尾部、超 media idle、鉴权撤销/刷新、丢 ACK、旧恢复与捕获屏障；无新增 Response/history/TTS/媒体 owner。
- [ ] **DU10 完整验证（Root）**：默认 workspace、两端无默认 all-targets、适用 Linux ARM、真实 PTY/模型；源码 manifest 无漂移，失败与 ignored 单列；旧 WAIT 模型用例已符合完整新协议。
- [ ] **DU11 完成提交（Root）**：通过以上出口后才合入主重构分支；更新协议/操作文档、progress、计划和机器可读状态；清理仅任务自有临时执行资源。

## 必须保持的行为

- [ ] 同一 hold 的原 Response、Context、source、双 epoch、backend、播放游标和媒体 owner 始终不变。
- [ ] Fresh SQL permit 才能首次发送；同 ID 查询、未知提交、重启不授权重发媒体或控制。
- [ ] 原命令 outcome 与当前设备 snapshot 分开；旧回执不能让已退休输出复活。
- [ ] 临时附和只能恢复 AcousticCandidate；ExplicitWait 需要明确 Continue。
- [ ] 工程默认 hold 60 秒、硬上限 120 秒、临时语义期限 3 秒；受原输出剩余寿命限制；Confirm/重试不延长硬期限。
- [ ] 单 encoded payload 1 MiB、同 Response 保留 8 MiB、进程全局 256 MiB；应用复制前取得额度，实际保留期间不提前释放。
- [ ] 取消与记录已经发生的晚事实不要求旧 Context 仍有效；Resume 必须通过原完整权限/根版本检查。
- [ ] 软件门、模型标签、后端接纳与物理 Presented/AEC 各有独立证据；不能混称完成。

## 验证日志

本轮证据目录：`run/validation-20260922/`；各 agent 也在自己的 `run/` 保留命令、exit、源码清单。任务 PG 端口 32768，B 初始独占测试窗口；不操作用户 5433 或其他业务容器。完整 schema 测试需协调并发，不能以串行所有行为掩盖真实竞争。

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

- [ ] 暂停包完成后：Companion shadow/回放与受约束主动表达。
- [ ] AEC、时钟映射/漂移、Mac/Pi 双讲与暂停听感。
- [ ] D3 动作资产、执行器反馈/限位、真实位置与呈现证据。
- [ ] 首轮 CLI/REST 管理/诊断、选定组件及其他 Server 平台收敛。
- [ ] Mac + Pi 真机全新安装、故障恢复及至少 24 小时混合负载。

Web/iOS/Android 与通用 Embedded 适配继续按 [延期台账](../deferred.md) 留待后续。再次暂停时，把当前分支 SHA、dirty 文件归属、活动 exec/容器、未过测试和下一条 checklist 一起记录，避免仅留下笼统百分比。

## 本轮已处理的整合风险（等待对应最终验收）

- [x] 独立控制队列与 output JoinSet 已实际接线，首次 Server lib 编译通过。
- [x] Audio 入队前保留原 MediaOwner；已完成请求的幂等查询仍返回原事实。
- [x] SQL command/hold 期限与整个 output 期限分别收紧，恢复不丢失后加入 Audio 的原硬期限。
- [x] scope fence 超时仍保留同一个 native future；shutdown 不能提前报成功。永久退休先与该 fence 并行持久化，最后一项 pending 阶段断言正在复验。
- [x] Start 真正准备好后才启动上传 handshake 时钟；等待双端 gRPC 回归。
- [x] RegisterOutput 的本地入场身份在 Fence 时删除；旧 ACK 不能回填 cache，等待配对故障测试。
- [ ] 原 LLM/TTS 在途 Future/结果保留、Held 禁止下一次 TTS 的真实 PG 流水线回归。
- [ ] 对已绑定 voice scope 的生成错误，只取消原输出；语音错误提示如需另一个 Response，必须由新的 generation admission 明确授权。本轮不在原 scope 自动生成替代 Response/TTS。未绑定前的固定 source notice 仍走规范 Context/Response。

## 2026-09-22 暂停收口

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

本轮尚无新通过结论，沿用前述准确计数，满足条件后才继续勾选。
