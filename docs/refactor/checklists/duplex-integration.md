# 可恢复暂停纵向整合 checklist

更新：2026-09-22。执行中；用户已恢复额度并明确要求本轮维护 checklist。

本表是当前任务入口，旧 [2026-09-20 检查点](../resume-checkpoint-2026-09-20.md) 及四份 HANDOFF 作为历史证据保留。只有满足完成条件并写入证据后才能勾选；编译、测试运行、平台验证和真机验收分别记录。

## 分工与修改边界

| 负责人 | 工作树 / 分支 | 独占内容 | 子 checklist |
| --- | --- | --- | --- |
| A `/root/body_endpoint` | `alia-duplex-control-protocol` / `codex/duplex-control-protocol` | Core/Proto/Embedded；Core `turn.rs` 除外 | `duplex-protocol.md` |
| B `/root/body_server` | `alia-duplex-control-ledger` / `codex/duplex-control-ledger` | 控制 Repository、迁移 000013、真实 PG 用例 | `duplex-ledger.md` |
| C `/root/text_delivery` | `alia-duplex-media-budget` / `codex/duplex-media-budget` | TTS/HTTP 预算、AudioBuffer 调用端 | `duplex-media.md` |
| Root | `alia-duplex-integration` / `codex/duplex-integration` | 原 output owner、DeliveryService 装配、Voice/Pipeline/传输整合及完整验收 | 本表 |

根目录均为 `/Users/ke-qiao/Developer/project-alia/run/worktrees/`。A/B/C 在各自树修改、验证并提交；Root 逐次合入固定提交。B 的旧 Core 和旧 Root 的 Core/Proto overlay 继续独立保留，不复制为最新版、不混入无关提交。

## DU01—DU11 实施与通过条件

- [x] **DU01 恢复核对（Root）**：读取检查点及四份 HANDOFF；核对主分支 `e156531` 干净，A `2619a28` / B `76f5b04` / C `779f4f8` / Root `e94f66e` 均存在；A/C 干净，B/Root 仅剩已记录 overlay。2026-09-22 `git status`、`git worktree list` 及四树状态读取已完成。
- [ ] **DU02 隔离组合（Root）**：创建隔离整合树；逐包合并，处理 C/Root 交叉修改；依赖源头唯一，工作树中的构建输入可追踪。完成时记录所有输入 SHA。
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

## 后续任务接续

- [ ] 暂停包完成后：Companion shadow/回放与受约束主动表达。
- [ ] AEC、时钟映射/漂移、Mac/Pi 双讲与暂停听感。
- [ ] D3 动作资产、执行器反馈/限位、真实位置与呈现证据。
- [ ] 首轮 CLI/REST 管理/诊断、选定组件及其他 Server 平台收敛。
- [ ] Mac + Pi 真机全新安装、故障恢复及至少 24 小时混合负载。

Web/iOS/Android 与通用 Embedded 适配继续按 [延期台账](../deferred.md) 留待后续。再次暂停时，把当前分支 SHA、dirty 文件归属、活动 exec/容器、未过测试和下一条 checklist 一起记录，避免仅留下笼统百分比。
