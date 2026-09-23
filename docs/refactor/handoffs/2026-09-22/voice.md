# Server VoiceSession v2 — 暂停交接，2026-09-22

用户因额度不足明确要求暂停。已停止新增实现和测试，仅收尾已有编译并保存证据。**本包为部分实现，当前 live fixture 编译失败；没有 voice PG 或 real-model 通过结论。** 共享 integration 树由 Root 统一提交，本 agent 未在这里 Git add/commit/merge/push。

## 固定源码与归属

工作树 `/Users/ke-qiao/Developer/project-alia/run/worktrees/alia-duplex-integration`，分支 `codex/duplex-integration`。快照时 HEAD 为 `82167ddf8724ade6b203de2f9fbbe45459034178`；HEAD 不能代表当前未提交候选。`RESULT.json` 保存每个 owned 文件的 SHA-256，五个 Rust 文件的规范 manifest SHA-256 为：

`e457f46fe6043b1f7235acfaff9d041227da6159f1f69210e93269be8f232900`

本 agent 独占并已冻结的 dirty 文件：

1. `packages/alia-server/src/grpc/voice_session.rs`，含 actor/pure 单元测试。
2. `packages/alia-server/src/bootstrap/runtime.rs`，只改 VoiceSession 构造接线。
3. `packages/alia-server/src/config.rs`，新增持久配置读取形状 `policies.duplex`。
4. `packages/alia-server/tests/config.rs`，新增预算解析边界。
5. `packages/alia-server/tests/voice_stream_live.rs`，真实模型 + canonical PG source/Context/Response 的 smoke 草稿。
6. `docs/refactor/checklists/duplex-voice.md`，明确已过和未跑。

`owned-source.tar` 保存这些文件的完整文本；`owned.patch` 保存 tracked diff（新 checklist 以 tar 为准）。`git-status.txt` 是整个共享树当时的 dirty 清单，其余文件属于 Root/B/C，不得用这份 owned archive 覆盖它们。Root 添加了依赖接口 `AdmittedSource::channel() -> Arc<dyn ChannelAdapter>`；Root 也拥有 Delivery/VoiceOutput、Pipeline、playback transport 与对应 fixtures。原 A 工作树保持干净，已提交 `7bea32152059cce68a2bde89dc79614713d183e8` 与 `7cc9a845a571c2ba8fc8ed1ce7712a391c7dce7d`；A 的通过数不是本包证据。

## 已写入的生产接线（尚未验收）

- VoiceSession 构造签名改为 `new(auth, detector, input, delivery: Arc<DeliveryService>, hold_budget: Duration, shutdown, runtime_tasks)`。Bootstrap 从 `state.delivery` 与 `state.config.policies.duplex.hold_budget_ms` 传入；已更新本 agent 拥有的两个调用点。
- Open 只接受 voice v2、`software_hold=true` 和 `max_hold_ms` 在 `1..=120000`。Ready 回同样的 capability。实际 owner budget 为配置与协商上限的较小值，原 owner 120s 绝对寿命仍由 Delivery 限制。
- 原 `AdmittedSource` 在 session loop 之前只捕获一次；fence 使用其 pin 住的 adapter Arc。凭证刷新只重验，不替换 source/双 epoch。
- 普通接受与有界 drain 的每一个 speech frame 都同步调用当前 `VoiceOutput::near_end_speech(sequence)`；此本地请求不等待 SQL/控制 RPC。
- 模型结果先 drain，正向语义前重验原 source，再 drain，之后由 `TurnController::accept_evidence` 消费 freshness。仅真正新回答与明确 Stop 才 `invalidate`/fence/增加 generation。
- WAIT 变为 `explicit_wait`；Backchannel 仅能被原 gate 作为 Acoustic 恢复请求接纳。完整短语 `继续/继续说/接着说/请继续/继续吧/continue/please continue/go on/resume/resume speaking` 对应 ExplicitContinue；不做 substring 匹配。`继续讲这个问题` 是新请求。停止短语独立进入不可逆取消。没有可恢复 owner 时 Continue 不重新生成回答。
- 真正 Respond 先固定新 request UUID，调用 `delivery.begin_voice_output(owner, request_id, scope, original_channel, cancel.clone(), Some(budget))`，把 `Some(Arc<OutputProducer>)` 随 RecognizedVoiceInput 入队。Session 仅保存 VoiceOutput，因此最后 producer/media owner 消失可结束输出。
- Evidence 的 `output_decision` 只报告 Unchanged/PauseRequested/ResumeRequested/Cancelled，不把它当作 device Applied/Held 回执。
- 配置默认 60000 ms，serde 阶段拒绝 0、超过 120000、负数、浮点或字符串。默认值与边界的实际配置测试已通过。

## 已执行命令和证据

全部命令从该 integration workdir 执行，环境 `CARGO_TARGET_DIR=/Users/ke-qiao/Developer/project-alia/target`，均 `--offline --locked`。本轮没有使用 `DOCS_RS` 替代 native build。日志原件在 `run/duplex-voice-validation/`，已复制到本目录并记录摘要。

| 命令 | exit | 实际结论 |
| --- | --- | --- |
| `cargo check -p alia-server --lib --offline --locked` | 0 | 当时生产库通过，3.90s；不是最后共享候选的整体证明。`production-first.log` |
| `cargo test -p alia-server --test config --offline --locked` | 0 | 4/4、0 ignored；编译 14.30s，测试 0.01s。`config.log` |
| `cargo test -p alia-server --lib grpc::voice_session::tests --no-run --offline --locked` | 101 | 当时其它 owned 区域 `channels/embedded/playback/tests.rs` 有 11 处 v3 接口错误。Root 随后迁移了该文件，但本 agent 没有重跑。`actor-compile.log` |
| `cargo test -p alia-server --test voice_stream_live --no-run --offline --locked` | 101 | 当前 owned live fixture 有 E0599：第 234 行 `OutputBinding::validate()` 不存在。按用户暂停要求保留失败现场，没有继续修。`live-compile.log` |

**未执行**：voice actor PostgreSQL suite、live Easy-Turn/PG、最后 all-targets、no-default、共享 workspace。新的 Rust 文件已执行 rustfmt，但没有最后独立格式检查或全库检验，不能将 checklist 的验收项预勾选。

## 最紧要 TODO 和建议恢复顺序

1. 核对 `RESULT.json` 的 owned hash 与 Root 之后的阶段提交；保留共享树其它 agent 修改。先读本文件和 `duplex-voice.md`，不要回到旧 worktree 重做 A。
2. 修 `tests/voice_stream_live.rs:234` 不存在的 `binding.validate()`：使用 Core 当前实际提供的 registration/command 合法性验证，保留 original binding/backend 精确匹配，不能删掉身份约束来求绿。
3. 修同文件第 545 行 `fixture.count("responses")`：真实表是 `delivery_responses`。这是已知尚未执行到的 SQL 错误。
4. 重编译默认 voice actor 和 live targets。Root 的 playback fixture 迁移应解除原 11 个编译问题；任何新错误都按当前真实代码解决，不用 feature 绕过。
5. 与 Root/B 协调 PG 独占窗口，执行 `cargo test -p alia-server --lib grpc::voice_session::tests --offline --locked -- --nocapture`。预计 22 个测试，包括原测试迁移、capability/phrase、explicit wait/continue/stop、新采集使旧 continue 失效、双 epoch/绑定撤销、最后 producer 释放、队列饱和。尚未跑，数量只供定位。
6. Actor fixture 用随机 `voice_actor_*` schema，正常 migrations + Delivery `start`/recovery，没有跳过恢复的 test 入口。虚拟时钟测试在 PG 初始化后 pause、清理前 resume。复核失败/超时的清理路径，避免测试 panic 后遗留任务或随机 schema；当前成功路径显式 shutdown/cleanup，尚未由运行验证。
7. 下一独立窗口执行 live ignored case。该 fixture 计划通过真实模型 Complete 的 raw transcript 落库 → 同 request/source materialize → prepare canonical Response → 原 producer bind，然后通过真实 SQL control ledger 检查 Backchannel/WAIT 保留同一 Response。设备输出是明确的 Core 软件状态机 fixture，auth 为测试 credential；不宣称生产设备签名、物理音频、完整 Pipeline/LLM/TTS 或真实播放。
8. 正确性关注：确认 capture 在 inference/revalidation 期间推进会阻止旧 Resume；ACK 未知由原 Delivery owner 保持门，不由 voice Evidence 开门；WAIT 保持相同 epoch/cancel token，Stop/新问题必须增加 epoch；session 不可因额外保存 producer 延长输出。确认 source/owner 变化会取消原输出，绝不重新获取新 source stamp。
9. 更新 checklist 为实际通过证据，记录真实模型/PG 时间与失败，再交 Root 固定整体候选正常提交、全 workspace 与平台检查。共享树不得由本 agent 单独 Git 提交。

## 资源与下一次命令环境

- 自有 exec `74041`、`30367` 已 exit 0；`31920`、`4541` 已 exit 101。全部完成，无自有活动 exec。
- 未启动任何 PG 命令、未创建 voice/delivery schema，未启动容器。窗口先因编译阻塞释放，之后按用户要求未重新占用。
- PG 仅允许任务库：`postgresql://alia_test:alia_test@127.0.0.1:32768/alia_refactor_delivery_final`。不得碰 `alia-postgres:5433`。
- Root 已核对原模型服务 `http://127.0.0.1:8091` ready；由 Root 管理，本 agent 没有启动/停止它。模型 revision 仍要求 `7d1f8b06776b575ac22e2fd4aa7913da2f4b8b98`。
- `ALIA_EASY_TURN_PCM_DIR=/Users/ke-qiao/Developer/project-alia/run/easy-turn-stream-pcm`，三个官方 PCM 名为 `complete_real_1.pcm` / `backchannel_real_1.pcm` / `wait_real_1.pcm`，测试内仍有 SHA-256 固定校验。
- live 命令：设置上述 `ALIA_TEST_DATABASE_URL`、`ALIA_EASY_TURN_PCM_DIR` 与 `ALIA_EASY_TURN_URL=http://127.0.0.1:8091` 后，运行 `cargo test -p alia-server --test voice_stream_live --offline --locked -- --ignored --nocapture`。必须使用允许本机 loopback/PG 的 `require_escalated`；普通 sandbox 网络失败不能解释成服务不可用。

交接完成后进入 idle，等待用户恢复额度与 Root 新指令。
