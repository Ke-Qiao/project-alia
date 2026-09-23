# 恢复检查点 — 2026-09-22

**暂停原因：用户明确要求因额度不足停止新增工作、与三个 subagent 收口并保存详细进度。** 这不是功能完成或正式验收。旧2026-09-20检查点保留为历史；后续优先从本文件和 [数字状态](resume-state-2026-09-22.json) 接续。

归档说明：本文件记录2026-09-22暂停状态；源码快照于2026-09-23恢复后、任何新编辑之前完成提交。用户已于2026-09-23恢复执行，当前活动进度见总checklist。

## 1. 先看这几个数字

| 指标 | 本次暂停状态 | 解释 |
| --- | --- | --- |
| 暂停纵向整合主 checklist | **完成 2 / 共 11；剩余 9** | DU01 恢复核对、DU02 隔离组合已完成；DU03—DU11 仍待完整出口验证 |
| 原独立组件包 | **3 / 3 已提交、已组件验证并合入整合树** | A 协议/绑定竞争；B 控制账本；C 编码预算/调用端。不能换算成整条暂停链路完成 |
| 新运行时 PG 专项 | **18 unique pass / 0 fail / 0 ignored** | 旧 DeliveryService 12 + 新 lifecycle 6。cleanup修复后的单项复跑仍只计1次 |
| 最新 Pipeline 套件 | **启动 20；3 项明确完成；随后进程 abort** | 栈溢出，cargo exit101，无完整结果表；其余终态未知，不能写成17失败或全部未执行 |
| 新 paired gRPC | **5 条已编译/链接；0 / 5 实际执行** | 真实两端fixture已准备，尚未取得运行证据 |
| 新 Voice 包 | **生产lib和配置等局部检查通过；actor/live 尚未实跑** | 准确源码与命令边界见 A HANDOFF；不要把 no-run 当通过 |
| 暂停后首轮后续方向 | **6 项仍开放** | R01—R06：Companion、AEC/时钟、D3真机、CLI/REST、Mac/Pi长期负载、其他Server平台 |

这里没有给“整个重构百分比”：当前数字是有明确完成条件的工作项计数，不能用已实现代码行数推算交付率。

## 2. Git 与恢复位置

- 仓库：`/Users/ke-qiao/Developer/project-alia`。
- 主分支：`codex/architecture-refactor`。本次仅收口文档，已验收生产代码仍是 `e6ba75b34136f7ae3c23774594d7ccae9bc67f89`；其历史默认workspace为875 pass、0 fail、5 ignored。
- **当前待续代码**：`run/worktrees/alia-duplex-integration`，分支 `codex/duplex-integration`。本轮WIP源码保存提交：**ae75a4ee92b1ea2e8c11a1d20c488ba62125d81f**。它包含共享树内Root/A/B/C所有未验收新代码，不要重新从旧WIP拼一次，也不要直接合入主分支。
- 保存前已合并到 `82167dd`：旧包 `2619a28` / `76f5b04` / `779f4f8` / `e94f66e`；新 A `7cc9a84`（含 `7bea321`）、B `4846b4a` / `56e9acc`、C `5333eff`。
- 旧 A/C 树干净；旧 B 和旧 Root/runtime 树仍保留之前的 Core/Proto overlay，具体文件见旧HANDOFF；**不复制过时overlay、不restore、不删除旧树**。
- 构建输入 manifest 与最终代码 SHA/文件数见 `resume-state-2026-09-22.json`。本轮完整workspace未执行，manifest只是可恢复性证据，不是通过证明。
- 没有push、没有PR或部署。

## 3. 四份必须阅读的 HANDOFF

| 负责人 | 最新交接 | 当前代码归属 / 完成度 |
| --- | --- | --- |
| A `/root/body_endpoint` | [Voice + 协议交接](handoffs/2026-09-22/voice.md) | 旧协议包7cc9a84已验收；新Server Voice/Config/Bootstrap/real-model fixture仍WIP |
| B `/root/body_server` | [账本 + 生命周期交接](handoffs/2026-09-22/lifecycle.md) | 账本56e9acc已通过13项PG；新lifecycle与旧DeliveryService18项绿，等待最终整合验收 |
| C `/root/text_delivery` | [媒体 + 配对播放交接](handoffs/2026-09-22/playback.md) | 媒体5333eff已通过59项；5条真实paired gRPC只编译、未运行 |
| Root | [整合运行时交接](handoffs/2026-09-22/runtime.md) | actor/owner/gate/transport已接入；最新Pipeline栈溢出是首要问题 |

最新总表：[duplex-integration.md](checklists/duplex-integration.md)。子表：protocol、ledger、media、voice、lifecycle、playback 均在同目录。各agent原始日志和RESULT仍保留在整合树或其旧树的run目录，归档不会删除原证据。

## 4. 本轮实现与真实边界

Server已有独立有界output actor，入队前保留原media owner，原Response/Context登记先于TTS/Start；控制和媒体队列独立。未知Control提交/ACK只Observe，不重新发送。短语义期限、每hold期限和整个output期限分别约束；B修复了后加入Audio的硬期限在Resume后丢失的问题。

设备绑定Ready/Evidence使用原binding身份与同锁边界，解绑/重绑/相同配置重新加载不会让旧会话复活。服务端Start准备好后才启动RPC握手计时，迟到RegisterOutput ACK不能回填被Fence淘汰的原入场身份。设备停止超时保留原Future/owner，SQL取消/退休与native停止并行，shutdown不伪报成功。

Voice v2、严格Continue/Stop短语、临时Backchannel恢复、明确Wait同epoch、request ID和原producer入队已经有实现草稿。Pipeline已有TTS/LLM gate和新回归，但目前整合产生栈溢出；两条链路尚不能作为自然全双工完成交付。

已绑定voice scope发生生成错误时只取消原输出，不在同scope自动生成另一个Response/TTS错误提示。未绑定前的固定source notice仍走规范首次Context/Response。将来如需失败后的新语音提示，应由新generation admission明确授权。

所有软件gate、后端Submitted、模型标签均不等于物理静音、Presented、真实AEC或听感合格。Pi真机未验收。

## 5. 恢复后 Todo 与顺序

- [ ] **P01 / DU06、DU10：先复现栈溢出。** 日志 `run/validation-20260922/integration/pipeline-first.log`。失败测试 `channels::pipeline::integration_tests::text_delivery::a_large_provider_chunk_splits_only_at_utf8_boundaries_under_one_delivery`。检查新增 `cancellable_stage` 包装、异步Future尺寸与构造/调用栈。原因未定位；不得只扩大 `RUST_MIN_STACK` 然后宣称通过。先单项定位，再默认参数重跑整个20项Pipeline。
- [ ] **P02 / DU03、DU04、DU06：验证实际所有权与生产背压。** 新sentence hold测试应证明原在途TTS只执行一次、Held不发下一次TTS、不输出首块；恢复维持原Response/Context、两种epoch、backend与budget。旧9项transport和3项Gate单元也需执行。
- [ ] **P03 / DU05：完成A Voice actor与真实模型链路。** 必须走真实DeliveryService.start/recovery与规范Context/Response，不加测试恢复旁路；确认两个暂停模式、phrase不误判、capture freshness、auth刷新/撤销与queue drop。先修live fixture调用不存在的 `OutputBinding::validate()` 以及表名 `responses`（实际为 `delivery_responses`）；详细新增测试/当前编译边界见A交接。
- [ ] **P04 / DU09：运行C 5项真实两端gRPC。** 首块前Hold超过5s、native队列满、End后tail、丢Control ACK→Observe、迟到RegisterOutput ACK。保留生产软件后端边界，不能称真声卡。
- [ ] **P05 / DU07、DU09：旧scope cleanup与替换session。** C只读发现现有non-close fence严格校验新admission的same session/min epoch，旧owner cleanup在新session已激活后可报Err。先真实复现；需要原owner退休的证明，不能放宽新会话入场条件或误杀新scope。
- [ ] **P06 / DU03、DU07、DU08：收齐生命周期出口。** 在最终源码复验18项PG专项，覆盖Bind取消/超时、permit到期、Unknown/late facts、新speech与Resume、backend/实例变更、scope停止原Future和预算保留；组件结果与组合结果分别记录。
- [ ] **P07 / DU10：冻结源码后完整验收。** macOS默认workspace、两端no-default alltargets、适用Linux ARM验证、3项真实PTY及2项实际模型目标；每条记录exit/日志/hash，失败与ignored单列，不混加多轮测试数。
- [ ] **P08 / DU11：验收后正式合并。** 更新voice/控制协议、部署与操作文档、progress/checklist，再正常提交合入主重构分支；未授权push。

独立后续仍保持6项：R01 Companion shadow/回放与受约束主动表达；R02 AEC/时钟映射/漂移/双讲；R03 D3动作资产/反馈/限位/实际位置证据；R04 CLI/REST管理诊断/组件收敛；R05 Mac+Pi全新安装/故障/听感/物理停止/24小时负载；R06其他Server系统架构及未来同机部署/模型下沉。用户搁置的Web/iOS/Android与通用Embedded适配继续见[延期台账](deferred.md)。

## 6. 资源与重启约束

- 任务PG：`alia-refactor-test-db`，端口32768，URL `postgresql://alia_test:alia_test@127.0.0.1:32768/alia_refactor_delivery_final`。保留容器；PG套件用独立schema，重型完整schema测试协调窗口，保留用例内部真实并发。用户5433、HA/Astrbot及历史schema不动。
- Root abort留下的新schema由B按暂停前准确baseline做差集清理；最终名单/证据见B HANDOFF与RESULT；不能按通配符删除未知历史schema。
- Easy-Turn `http://127.0.0.1:8091/health` 本次实读ready/model_loaded，CPU float32。原模型revision `7d1f8b06776b575ac22e2fd4aa7913da2f4b8b98`，源码 `fe175eaa8442c9a0834acf29f8640ee74c2aceca`，Qwen `7ae557604adf67be50417f59c2c2f167def9a775`。保留服务/模型缓存；下次重新读health，不假定一直健康。
- PCM：`/Users/ke-qiao/Developer/project-alia/run/easy-turn-stream-pcm` 与 `run/easy-turn-backchannel.pcm`。
- Cargo target：`/Users/ke-qiao/Developer/project-alia/target`。真实Sherpa归档位于旧audio-clock树target/sherpa-onnx-prebuilt，SHA256 `ca7baf61f331ea6b848e3630e566647302c26960c7303932bc33125074871eeb`；不要用DOCS_RS等旁路伪装链接。
- 访问本机PG/loopback需要正确sandbox权限；普通sandbox curl exit7不代表服务停止。无需重新向用户询问已有授权的本地构建/测试。
- 最终活动agent/exec数及资源清理以机器状态为准；所有工作树和日志保留。

## 7. 下次可直接发送

> 额度已恢复。请从 `docs/refactor/resume-checkpoint-2026-09-22.md` 继续，读取四份最新 HANDOFF，使用三个 subagent，维护现有 checklist。保留已验证主分支，从整合WIP快照先定位 Pipeline 栈溢出，再完成 DU03—DU11 的暂停纵向整合与真实验证。
