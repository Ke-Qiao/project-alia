# Paired playback handoff — 2026-09-22

用户明确因额度不足要求暂停。此处保存真实中间状态；**不继续实现、不启动 PG、不把未执行测试计为通过**。本 agent 已结束最后一个 no-run 命令，无活动 exec、容器或本包创建的 schema。没有提交共享 integration 中的新修改。

工作树：`/Users/ke-qiao/Developer/project-alia/run/worktrees/alia-duplex-integration`。分支 `codex/duplex-integration`。记录时 HEAD 为 `82167ddf8724ade6b203de2f9fbbe45459034178`，Root 仍管理该共享树；最终主检查点若有新 SHA，以 Root 记录为准。当前工作树的所有 dirty 路径保存在本目录 `git-status.txt`，本 agent 的六个文件 SHA-256 保存在 `source-manifest.json`。机器可读命令、退出码、限制和资源状态见 `RESULT.json`。

## 先区分已完成的 C 媒体预算包

上一子包已经正常提交 `5333eff80149d996edc765da9798781deaceba6d`，分支 `codex/duplex-media-budget`，在旧 WIP `779f4f8` 之上，Root 已合入 integration。原 C 工作树干净，未 merge/push。它的 59 项通过结果是：TTS 21、真实 PG sentence 2、Embedded playback 9、Pipeline 17、Embedded normalize 3、error notice 4、sentence expression 3；default/no-default all-targets 均 exit 0。清单为 `docs/refactor/checklists/duplex-media.md`，日志及固定源码 manifest 在 `run/duplex-media-validation/`。**这些证据不覆盖本次新增的配对五项测试。**

## 新配对包已经写入的内容

`packages/alia-server/tests/duplex_playback.rs` 已新增五项测试。它复用 `delivery_support` 的真实 PostgreSQL 随机 schema、Context 和规范 Response；Server 使用真实 `DeliveryService` / `OutputProducer`，通道走 `EmbeddedChannel::init` 加载测试私有签名密钥、恢复 ledger 并建立真实 Tonic client。对端使用生产 `PlaybackService`、`PlaybackSession`、`PlaybackControl`、`AudioSubmission`。

唯一 native 替身是测试内部有界 PCM 队列，沿用端侧原 gRPC 测试的 backend 模式：Condvar 精确阻塞满队列的 submit，Semaphore 暴露入队阻塞/End 屏障，模拟 callback 必须通过原 `try_begin_output` 门并 `finish` 实际提交数量。测试不制造 Presented，不声称物理声卡、扬声器静音或 AEC 行为。

五项测试的目标和关键观测如下，**均未执行**：

1. `held_before_first_start_survives_handshake_idle_and_resumes_one_original_output`：Output 先规范绑定，然后 ExplicitWait；原媒体保留额度，停留 5.5 秒后仍没有 Push，fresh ExplicitContinue 后只创建一次 Push，原 4 samples 提交。
2. `held_full_native_queue_keeps_original_pcm_and_control_lane_available`：队列在 1600 samples 时以 Condvar 阻塞下一次 enqueue；独立 Hold 到达，原 `try_begin_output` 返回 Held；恢复后逐样本对比 6400 samples 的原顺序，只有一个 submission。
3. `held_after_end_preserves_unsubmitted_tail_beyond_media_idle`：End 已接纳但 tail 尚未提交，Hold 5.5 秒跨过媒体 idle，原 tail 仍保留；恢复后提交同一尾部。
4. `control_ack_loss_is_observed_from_original_device_command`：薄代理先让真实服务执行 Control，再返回 unavailable 丢弃 ACK；actor 通过真实 Observe 恢复同一 command 的回执，未获许可前保持 Held。
5. `registration_reply_after_fence_never_repopulates_obsolete_channel_binding`：薄代理在真实 RegisterOutput 完成后延迟回复，原 scope Fence 先执行；晚 ACK 必须拒绝，旧 binding 不得从本地 cache 复活。

两条 5.5 秒等待用于跨越真实五秒期限，不用于概率制造竞态。其他竞态使用 semaphore/Condvar；状态轮询有三秒上限，单 case body 32 秒，外层 PG fixture 45 秒。正常、panic、超时均先释放原 native 队列并 join 两端 owner，再清理 PG；初始化出错也有 listener guard。这里描述的是已写的 cleanup 代码，尚不能以运行结果保证其所有路径。

## 四个公开 backend API

原 `AudioPlaybackBackend` 是公开 trait，但合法实现控制门所需方法原来只有 crate 可见，外部 integration 无法注入仍使用同一生产控制门的 backend。Root 已明确授权只公开现有四个方法并补契约注释：

- `PlaybackControl::new(scope, generation, backend)`：构造一个 native generation 的原 gate，必须保留和复用 Arc。
- `PlaybackControl::apply(command)`：Session 已做准入与 owner 核验后，backend 对保留的原 gate 执行命令。
- `PlaybackControl::retire(reason)`：丢弃排队媒体前永久关闭原 owner。
- `AudioSubmission::with_control(identity, generation, source_rate, backend, control)`：绑定原 gate，现有 scope/generation/backend 一致性检查保留。

没有新增 `test-support` feature、绕过控制的 submission 或平行状态机。文件仅为 `module/playback_session/control.rs` 与 `submission.rs`，其余生产行为未由本 agent 修改。

Server `Cargo.toml` 的 dev-dependency 增加 `alia-embedded = { path = "../alia-embedded", default-features = false }`。Cargo.lock 仅在 alia-server dependency 列表增加已有 workspace 包 `alia-embedded` 一行，无新外部依赖。

## 实际验证与日志

平台为 macOS / Darwin arm64，Rust 1.94.1。所有命令在本 integration 工作树运行，没有联网拉依赖。

| 命令 | 实际结果 | 本目录日志 |
| --- | --- | --- |
| `cargo check -p alia-server --test duplex_playback --offline` | exit 101：新测试缺少 `ConnectionTrait`、一个捕获 mutable Vec 的 async closure，另有 unused imports；已修正 | `duplex-paired-check-initial.log` |
| `cargo check -p alia-server --test duplex_playback --locked --offline` | exit 0，无本目标编译错误 | `duplex-paired-check.log` |
| `cargo test -p alia-server --test duplex_playback --no-run --locked --offline` | exit 0，47.34 秒完成实际链接；**没有执行测试** | `duplex-paired-link.log` |

no-run 产物：`target/debug/deps/duplex_playback-debfc4d3753dc8c4`。该路径是构建产物，不替代源码或测试证据。最后 exec `73327` 已完成 exit 0；此前编译 exec `10934` 已完成 exit 101，后续修正 check exit 0。没有仍在等待的 cargo 或数据库命令。

共享 integration 的其它生产修改由 Root/A/B 同期推进，因此这里的通过结果仅是当次实际编译/链接，**不是最终整合树的全量验收**。本子包没有 PG/gRPC 运行、Linux、Windows 或物理音频硬件证据。

## 未解决的 replacement cleanup 契约

Root 另外提出旧 session 被新 session 替换后的 cleanup 可能过严。本 agent 仅读取代码，没有新增测试或修改这条生产路径。

可达代码时序：端侧 `PlaybackSession::activate_local(new_session)` 先标记旧 capture retired、调用 `retire(old)` 再装入新 active。随后旧 Server output actor 调用 `stop_current_playback(oldscope)`，生成 non-close `fence_voice(old_gen + 1)`。端侧 `fence` 发现 session 不同，返回 applied=false 和新 active_scope；Server `validate_fence_receipt` 对 non-close 强制 same-session + minimum epoch，因此返回 Err。Root actor 可能把本已退休的旧 owner 记成 cleanup failure。

**目前只有代码路径证据，没有实际配对复现，也没有确认任何替代 proof 足够。** 下次先完成上述五项，再用同 fixture 做最小第六条：准备旧规范 Output，设备本地 Ready 替换 session，尝试旧 cleanup，确认新 scope/native owner 不受影响并检查真实错误及账本状态。建议研究仅供 cleanup 使用、有类型且绑定原 scope/backend/registration 的退休证明；必须区分已关闭准入与已收尾在途 native write。不得把新 active_scope 自动当成旧 native 已静音/已结算，也不得放宽用于新会话 admission 的 fence 校验。

## 文件归属与暂停资源

本 agent 唯一拥有的未提交源码/文档：

- `packages/alia-server/tests/duplex_playback.rs`
- `packages/alia-server/Cargo.toml`（仅上述 dev-dependency）
- `Cargo.lock`（仅上述依赖边）
- `packages/alia-embedded/src/module/playback_session/control.rs`（三个公开方法及注释）
- `packages/alia-embedded/src/module/playback_session/submission.rs`（一个公开方法及注释）
- `docs/refactor/checklists/duplex-playback.md`

`run/duplex-playback-handoff-20260922/` 为此 agent 新增的交接证据，原始三个日志也留在 `run/duplex-paired-*.log`。这些 run 路径被 .gitignore 忽略，Root 后续若需提交归档须显式 add；**本 agent 按暂停指令没有 add/commit 共享树**。

其余 dirty 文件都由 Root 协调、C 未修改。当前快照分组：

- Root 整合/通道/管线：`docs/refactor/checklists/duplex-integration.md`；`src/channels/embedded/{channel.rs,playback.rs,playback/tests.rs}`；`src/channels/pipeline/{delivery.rs,error_notice.rs,integration_tests.rs,streaming.rs,turn.rs,voice.rs}`；`src/grpc/audio_ingress.rs`；`src/modules/delivery/{mod.rs,service.rs,service/output.rs}`；`src/modules/interaction/voice_input.rs`；`tests/delivery_service.rs`。
- A 的 voice 子包，由 Root 管理归档：`src/bootstrap/runtime.rs`、`src/config.rs`、`src/grpc/voice_session.rs`、`tests/config.rs`、`tests/voice_stream_live.rs`、`docs/refactor/checklists/duplex-voice.md`。
- B 的 owner 生命周期子包：`packages/alia-server/tests/output_lifecycle.rs`。

上述 `src/`、`tests/` 简写均位于 `packages/alia-server/`。Root 可能在生成本记录后继续写暂停摘要，完整精确路径以 `git-status.txt` 和最终 Root 检查点为准；分组用于避免恢复时覆盖他人修改。

本包从未启动 PG 测试，没有创建 schema、测试私有 SQLite 或临时签名密钥运行目录（这些只会在运行 fixture 时建立）。没有创建 Docker 容器。任务 PG 32768 属于 Root 共享资源，不由本 agent 停止；未访问用户 5433。此前旧 C 的 PG 随机 schema 已在成功 fixture cleanup 后归还窗口，不与本未跑包混算。

## 下次最短续做入口

1. 读 Root 最新暂停总检查点和本文件，保留共享 integration 全部修改，确认最终 HEAD/归档 SHA。
2. 确認 Root/A/B 没有正在用任务 PG，再设置授权 `ALIA_TEST_DATABASE_URL`；不要沿用任意旧 schema。
3. 运行 `cargo test -p alia-server --test duplex_playback --locked --offline -- --test-threads=1`。五项预期 runtime 约 15 秒，加编译与 schema 建立；目前这一预计不是运行证据。
4. 若红，保留真实日志，定位是哪端生产契约或 fixture 问题，与 Root 协调生产文件所有权；不得把失败断言放宽成“没有崩溃”或伪造 Presented。
5. 五项真正通过后再复现 replacement cleanup，定义原 owner 的窄退休 proof；新会话准入校验仍严格。
6. 更新 paired checklist、日志/源码摘要，再由 Root 做完整 DU01 校验和正常提交。当前只暂停，不声称新配对包完成。
