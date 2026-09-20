# 额度暂停检查点 — 2026-09-20

工作批次：2026-09-20；实际收口：**2026-09-21 00:00 后（Asia/Shanghai）**，文件名沿用工作批次便于关联证据。

状态：**用户要求收口并暂停，等待额度恢复后明确继续。** 本文件是下次接续入口。不要依据未合并草稿宣称自然全双工的可恢复暂停已经端到端完成。

## 1. 可直接恢复的基线

仓库：`/Users/ke-qiao/Developer/project-alia`。主重构分支：`codex/architecture-refactor`，原始基线 `3530395bbbf75da3b44a47e402c3a1e969cac012`。

| 主分支提交 | 已完成内容 |
| --- | --- |
| `8c3f545` | Appraisal/Relationship 的规范来源与失效重建，完整 Memory 来源闭包及隐私擦除传播 |
| `e6ba75b` | 合入 `213d5ed` 的 Core/Embedded 本地可逆播放暂停基础 |
| `b995098` | 用户确认的两类暂停语义、后续暂停实施契约、Companion 设计及延期台账 |

**最新已验证生产源码为 `e6ba75b`；之后主分支只提交计划和检查点文档。** 四个下一包工作树独立保存，尚未合入主分支。当前主分支网络 WAIT 仍永久取消；新的恢复行为必须等下一包完整接通后才能启用。

主分支已有的 Execution、Memory、ContextLease、缓存、Delivery、签名身体动作、实际播放参考和独立时钟等历史包，见 [完整构建记录](progress.md)。本次暂停不撤销已完成工作，也不从原始基线重建。

## 2. 数字进度与验证边界

本次收口跟踪 **6 个工作包：2 个已合入，4 个在隔离分支保存**。数字按工作包状态和实际测试统计。

| 工作包 | 状态 | 数字/完成证据 |
| --- | --- | --- |
| Appraisal / Relationship / Memory 闭包 | 已合入 `8c3f545` | macOS 完整候选 859 项通过；最终仅测试夹具增量后 Server lib 216 项通过；Linux ARM 最终 Server 637 项通过；5 项真实 PTY/模型另跑通过 |
| 可恢复播放暂停 Stage 1 | 已合入 `e6ba75b` | 与 Appraisal 整合后 macOS 默认 workspace **875 通过、0 失败、5 显式跳过**；其中 Core 25、Embedded lib 135、Server lib 216；两端无默认特性 all-targets 通过 |
| A：Core/Proto/Embedded 控制协议 | 隔离保存，组件已验证 | Core 27、Proto 3；Embedded 默认/无默认 lib 各 145 项通过，两种 all-targets 检查通过；未整合 Server |
| B：原 Response 的持久控制账本 | 隔离 WIP | Core-only Server all-targets 检查通过；9 项真实 PG 测试仅 `--no-run` 编译通过，执行数 0 |
| C：TTS / 保留媒体字节预算 | 隔离 WIP | 首次 all-targets 检查 exit 101：lib 5 / lib-test 22 个编译错误；测试执行数 0；11 个源码文件保存 |
| Root：Server 控制 owner / 语音接线 | 隔离 WIP `e94f66e` | 6 个文件保存，格式/语法检查通过；**未完成装配、未编译、未执行新增测试** |

Stage 1 整合验证耗时 **370.02 秒**；无默认特性检查 **36.18 秒**。587 个构建输入的 SHA-256 清单：`c97283aee880270634f7cedb58ced6ffbdc18d67e425e5aa6533817c7f29a6c1`，运行后无漂移，全部 587 个 Git blob 与 `e6ba75b` 一致。路径：`run/validation-20260920/duplex-stage1-integration/`，含 `source-manifest.json`、`workspace.log`、`workspace-summary.json`、`workspace-result.json`、`no-default-result.json`、`commit-mapping.json`。

5 项跳过是 3 项真实 PTY 和 2 项真实 Easy-Turn/gRPC。它们在前一 Appraisal 候选另跑通过，本次 875 的组合回归没有再次运行，计数不混用。Appraisal 首次直接 CPU 模型调用曾约 **12.6 秒**，后续约 **1.335 秒**；模型功能通过不能等同声学端到端延迟达标。

## 3. 三个 subagent 与主 agent 的持久保存位置

三个 subagent 均已 completed；所有本轮构建/测试 exec 已结束，未留 agent 后台工作。A/C 工作树干净；B/Root 剩余 dirty 仅为已独立备份的临时依赖副本。

全部工作树位于仓库忽略的 `run/worktrees/`，不是临时 `/private/tmp`。恢复时先检查分支和 dirty 状态；不要删除、覆盖或重建工作树。

| 负责人 | 分支 | 工作树目录 | 收口提交 / 状态 |
| --- | --- | --- | --- |
| `/root/body_endpoint` | `codex/duplex-control-protocol` | `run/worktrees/alia-duplex-control-protocol` | `2619a28`，组件验证通过，未整合 Server |
| `/root/body_server` | `codex/duplex-control-ledger` | `run/worktrees/alia-duplex-control-ledger` | `76f5b04`，编译通过，PG 用例未执行 |
| `/root/text_delivery` | `codex/duplex-media-budget` | `run/worktrees/alia-duplex-media-budget` | `779f4f8`，未完成调用端迁移，编译失败 |
| `/root` | `codex/duplex-runtime` | `run/worktrees/alia-duplex-runtime` | `e94f66eb6968706053cceab7c9d371c24f37e116`，未编译 WIP |

四份详细交接还已复制进主分支文档，便于不依赖对话历史续接：[A 协议](handoffs/2026-09-20/protocol.md)、[B 账本](handoffs/2026-09-20/ledger.md)、[C 媒体](handoffs/2026-09-20/media.md)、[Root 运行时](handoffs/2026-09-20/runtime.md)。

各工作树中的原始详细交接文件：

- A：`run/duplex-control-handoff/HANDOFF.md`。
- B：`run/duplex-control-ledger-handoff/HANDOFF.md`。
- C：`run/duplex-media-handoff/HANDOFF.md`。
- Root：`run/duplex-runtime-handoff/HANDOFF.md`、`RESULT.json`、`shared-overlay/`。

B 和 Root 曾复制 A 的共享 Core/Proto/Cargo.lock 用于开发，这些是**临时依赖副本，不能混进其自身阶段提交**。Root 的副本早于 A 最终 `TurnEvidence.output_decision`，恢复应采用 A 最终提交。其原副本已完整备份并记录哈希，工作区状态以交接文件为准。独立分支引用尚未合并的契约时可能无法单独构建，这是本次 WIP 的明确限制。

## 4. 已确认决定，不再重复询问

- 质量、正确架构和可验证性优先；合格旧代码可改进，差的可重建；没有用户和历史数据迁移负担。
- 用户已授权修改代码、使用 subagent、新建分支、正常本地提交。没有要求 push。
- 首轮 macOS Server + 树莓派 5 8 GB，Easy-Turn 固定在 Server，不评估 Pi 承载模型。
- Web / iOS / Android 和 Embedded 通用平台/设备适配按 [延期台账](deferred.md) 留到后续。
- **临时避让与明确暂停分开**：疑似插话经新鲜语义判断确认只是附和后可恢复；明确“等一下／先停一下”必须等明确“继续”。旧 generation 已取消的音频不能复活。
- 暂停只保留同一 Response、Context、source、privacy/relationship epochs、backend、媒体 owner 和游标，不重新 TTS、不申请新 Delivery permit、不生成第二条助理历史。

明确等待多久的问题曾异步询问，但本检查点尚无回答。已说明暂采用可配置工程默认 **60 秒**、硬上限 **120 秒**，并受原输出剩余寿命约束；临时语义判断期限 **3 秒**。这些时限不是用户已确认的产品数值。Confirm 和重试不能延长原 hold 的硬截止时间。

## 5. 下一包接口契约

### A：协议和 Embedded

- playback **v3** / voice **v2** 成对升级，拒绝旧版本，不做双栈或 Cargo feature 兼容开关。
- `OutputBinding { scope, response_id, context_id, server_instance_id }`；设备先 `RegisterOutput`，Start 必须匹配原登记，不能用 Start 自建规范 Response。
- `BoundControlCommand` 固定 typed JSON + 版本域 SHA-256；Pause / ConfirmExplicitWait / Resume 均有原 hold、command、sequence 和 backend。
- `BoundControlReceipt` 分开保存原 immutable outcome 与当前 snapshot；同 owner 的 `observation_sequence` 在控制锁内采样/递增。只读 Observe 不执行、不续期、不创建 owner。
- 设备采集在实际序号入队前，与 Resume 共用控制锁更新屏障；包含 Ready 前排队、generation 变化和后续新语音。Held 仍继续 capture、auth、进度和原绝对 deadline。
- `TurnEvidence.output_decision`：Unchanged / PauseRequested / ResumeRequested / Cancelled。同 epoch 的 Pause/ResumeRequested 只是语义请求，不能凭 Evidence 放开设备门；仅合法 epoch 增长执行 fence。

### B：持久账本

`DeliveryRepository` 上的 `register_control`、`admit_control`、`query_control`、`record_control`、`unknown_control`、`retire_control`、`recover_expired_controls`。公开类型位于 `repository::control`。

只有首次事务明确提交成功返回的非 Clone `ControlPermit` 授权一次 Control RPC；Existing、query、数据库 commit ACK 未知均不能重新发送。Resume 走完整原 Context 根锁序；取消、退休和迟到事实不重新依赖旧权限。新实例不能以“不是自己”推定另一实例死亡，更不能接管旧 PCM。

两张表，每 scope 最多 32 个 command；每 command first/latest receipt，加 scope latest，总计最多 **65 份回执**。未知命令的迟到原 outcome 可固定，但旧 snapshot 不回退当前状态；普通 Observe 不无限追加审计日志。新增迁移 `000013`，不重写已提交迁移。

### C：媒体预算

`ResponseAudioBudget::for_response(Uuid) -> Result<Self>` 复用原 Response 预算，`for_standalone_request` 明确区分独立 API 请求。`EncodedAudio` 是 TTS 的必需返回类型，只能从 reservation 构造；`AudioBuffer` 是通用通道数据。共享字节及 RAII 额度使 clone 不复制 Vec、不重复占额，最后 owner drop 释放；这些行为在 C 检查点尚未测试。

目标上限：单 encoded payload **1 MiB**，同一原 Response **8 MiB**，进程全局 **256 MiB**；错误 HTTP body **4 KiB**。在 HTTP body 复制/累积之前取得预算，贯穿 pitch/header、TTS stream 和队列；不存在 `bytes().await` 后才检查的假有界路径。编码预算不等于所有 PCM/native 分配或整个进程 RSS 已有界。

规范 `prepare_response` 要早于首次 TTS；四条 TTS 路径复用原预算。Root 控制 owner 还需持有预算直到原 producer 和媒体全部退出，不能在句间释放后重新创建。

## 6. 恢复后的具体 Todo，按顺序执行

### 第一优先级：完成本次暂停纵向包

- [ ] **DU01** 读本文件、三个 agent HANDOFF 和 root HANDOFF，执行 `git status` / `git worktree list`，核对保存 SHA 和临时 overlay；不要恢复过时副本覆盖最新 A。
- [ ] **DU02** 在隔离整合树依次组合 A（共享契约）、B（SQL）、C（媒体）与 Root WIP。C/Root 在 Server `channels/adapter.rs`、`embedded/playback.rs` 等处有不同职责的交叉修改，逐 hunk 合并；不要整文件覆盖。
- [ ] **DU03** 完成 DeliveryService 的 output owner 装配：原队列、独立控制入口、JoinSet、启动/关闭、容量、原 media owner 计数及到期收尾。Root 新 `output.rs` 目前没有在 service.rs 声明，引用的 outputs/output_sender 等字段尚不存在。
- [ ] **DU04** 在 VoiceInput admission 时固定 request ID，贯穿 RecognizedVoiceInput、VoiceTurn、原生产 owner；规范 Response prepare 后登记设备/SQL，first Start 前核验。排队输入 Drop、错误路径和原生尾部都必须正确关闭或保留原 owner。
- [ ] **DU05** Server voice v2：持续采集和 auth 不等待 SQL/Control；先消费 fresh evidence，再决定 Wait/Continue/新响应是否改变 generation。临时 Backchannel 只恢复临时 hold；明确 Wait 只能明确 Continue。通过独立控制回执判断设备事实。
- [ ] **DU06** TTS/LLM/音频背压与字节预算接通；Held 不启动新 TTS，已开始的有限请求和原 stream 保留，不 Drop 后重调 provider。不要在暂停等待中持有 PG 事务或阻塞控制队列。
- [ ] **DU07** 审核 Root 草稿的已知缺口：Bind 的超时/取消；Confirm 改短语义期限时的定时器重算；未知 ACK 的 Observe/永久退休；迟到事实；新语音与 in-flight Resume；生命周期、backend 更换、同实例恢复和双实例隔离。补真实失败测试再修正。
- [ ] **DU08** 先审 A 的 Ready 激活与原 Binding 读锁竞争；B 的后加入 Delivery 观察期限、snapshot watermark 上界、真实关系根 fixture；C 的流式错误传播和未迁移调用端。再编译默认与无默认特性；执行 A/B/C 各自未完成测试。重点真实 PG：clear/policy/root 变更对 Resume、重复意图、未知 commit、到期、取消、双实例和 late facts。
- [ ] **DU09** 两端真实 gRPC loopback：媒体队列满、Pause-before-Start、End 后尾部、Held 超过 media idle、鉴权刷新/撤销、丢 ACK、旧 Resume、输入排队前屏障；确认没有新 Response/history/TTS 或第二份媒体 owner。
- [ ] **DU10** 修改真实 Easy-Turn/gRPC 用例的旧 WAIT 取消预期，验证成对新协议的两类暂停；完整 workspace、适用 Linux ARM 定向复核和源码 manifest；记录 failures/ignored，不混合不同候选计数。
- [ ] **DU11** 全部出口通过后形成可评审完整提交，才合入主重构分支；更新 progress/voice protocol/operations/plan/台账。

### 后续首轮主线，仍未完成

- [ ] **R01** Companion：按 [设计](companion-design.md) 分开 Observation、Hypothesis、Opportunity、Initiative；先有界 shadow/回放，再接真实 Context/Execution/Delivery 和反馈，不制造用户来源。
- [ ] **R02** AEC、独立采集/播放时钟映射、漂移测量、真实双讲和自然插话质量。现有参考流与时钟不等于 AEC 已完成。
- [ ] **R03** D3 短动作资产、执行器限位/反馈与真实位置/呈现证据；signed 软件提交不等于硬件完成。
- [ ] **R04** 首轮 CLI/REST 管理和诊断闭环、选定外部组件/平台接口收敛；按原计划逐项验收。
- [ ] **R05** Mac + Pi 实机全新安装、故障恢复、输出听感、物理停止延迟、资源/热量和至少 24 小时混合负载。
- [ ] **R06** 其他 Server 系统/架构验证，保留模型位置与部署角色的分离；将来端侧模型、同机多进程和组合进程按独立门槛继续。

待实机信息：Pi OS/连接方式、麦克风/声卡/扬声器与 AEC、执行器与反馈/限位、最终 LLM/TTS/Embedding 配置。之前已询问，不反复询问；代码和独立测试可继续，硬件结论不能提前验收。前端与通用端侧适配继续按台账延期。

## 7. 测试资源和证据

- 本任务隔离 PG：容器 `alia-refactor-test-db`，宿主端口 `32768`，数据库 `alia_refactor_delivery_final`；测试连接 `postgresql://alia_test:alia_test@127.0.0.1:32768/alia_refactor_delivery_final`。随机 schema 隔离；Linux host 用 `host.docker.internal`。
- **不要操作用户的 `alia-postgres:5433` 或其他 HA/Astrbot 容器。** 保留既有历史 schema，不把未识别 schema 当本轮垃圾删除。PG 并行完整 schema 夹具曾耗尽共享锁；Context 全套夹具并行上限已设 4，实际事务竞争仍保留。
- Sherpa 真正归档在 `run/worktrees/alia-audio-clock-a33ec67/target/sherpa-onnx-prebuilt/`，文件 `sherpa-onnx-v1.13.1-osx-arm64-static-lib.tar.bz2`，SHA-256 `ca7baf61f331ea6b848e3630e566647302c26960c7303932bc33125074871eeb`。默认构建使用 `SHERPA_ONNX_ARCHIVE_DIR` 指向该目录，不用 `DOCS_RS` 跳过原生链接。
- Easy-Turn 本机 CPU 服务此前位于 `127.0.0.1:8091`；模型、样本和 Cargo target 保留。下次先读状态，不假定服务仍活着。官方 PCM：`run/easy-turn-backchannel.pcm`、`run/easy-turn-stream-pcm/`。
- Appraisal macOS / Linux 完整证据分别在 `run/validation-20260920/appraisal-integration/` 与 `appraisal-linux/`，保留初次失败及各候选变化。Linux 固定 Rust 镜像 SHA 见 [平台记录](platform-validation.md)。
- `run/` 被 Git 忽略：源码已通过各隔离分支提交保存；完整原始日志/归档/overlay 仍依赖本机磁盘，请勿清理该目录。

## 8. 下一次对话入口

用户额度恢复后可直接说：

> 额度已恢复。请从 docs/refactor/resume-checkpoint-2026-09-20.md 继续，先读取四份 HANDOFF，保留已验证主分支，从 DU01 开始完成暂停纵向整合。继续使用三个 subagent，按实际验证结果正常提交。

恢复先处理这些保存的分支，不重复规划已确认目标、不重新询问已授权提交，不先开展前端或 Pi 模型性能评估。
