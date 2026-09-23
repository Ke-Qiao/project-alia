# Server VoiceSession v2 检查表

2026-09-23，工作树 `run/worktrees/alia-duplex-integration`。本文件记录本 agent 的 voice/config 子包；完整暂停纵向集成由主代理汇总。共享树不单独 Git 提交，不覆盖其它 agent 的未提交修改。

- [x] 核对原入口、Core freshness、VoiceOutput public API 与原来源双 epoch；确认旧 voice v1 / WAIT 取消须成对替换。
- [x] 成对 voice v2 能力协商，拒旧版本/无 software hold/非法上限。
- [x] accepted 与 drained speech frame 都先更新原 VoiceOutput 的近端语音屏障；采集不等待控制 SQL/RPC。
- [x] 新鲜 evidence 后区分 Wait、Acoustic Backchannel、严格完整 Continue、Stop 与新问题；Evidence 仅报告语义请求。
- [x] Resume 前重验原凭证/来源；pin 原 Channel Arc；新响应 request_id / producer / cancel 所有权固定，session 不保留 producer。
- [x] `policies.duplex.hold_budget_ms` 默认 60000，解析验证 1..120000；实际 `--test config` 4/4 通过。
- [x] 该配置受双方能力与原 owner 生命周期限制的实际 voice/Delivery 联合验证。
- [x] 真实 PG 的正常 Delivery start/recovery fixture；暂停/撤权/新采集/替换/关闭/背压定向回归。
- [x] 更新真实 Easy-Turn gRPC smoke，不使用伪 Context/Response ID；记录实际运行或未运行原因。
- [x] 默认 Server 定向检查/测试、格式、日志与已知限制完成，源码冻结交主代理。

此前 Embedded Ready 原 Binding 修复见 `duplex-protocol.md`；不能将其通过数替代本包验证。PG 只使用任务临时数据库与独立随机 schema，运行窗口与 B agent 协调。

## 额度暂停检查点：2026-09-22

用户明确要求暂停。当前共享工作树源码是**部分实现，未完成验收**，本 agent 未 Git 提交。仅收尾已有编译后冻结，未启动 voice PostgreSQL 或真实 Easy-Turn 测试。原始日志在 `run/duplex-voice-validation/`；详细恢复顺序、dirty 文件归属、源码摘要和命令结果见 `run/duplex-voice-handoff-20260922/HANDOFF.md` 与 `RESULT.json`。

已保存实现：v2 能力/Ready、新鲜 evidence 分流、speech capture 更新原 VoiceOutput、固定请求/producer、原 Channel Arc、原来源再鉴权、配置和 bootstrap 接线。上述未勾选项已有草稿代码和回归用例，但测试尚未跑，不能视为完成。

| 已执行命令（均使用 `CARGO_TARGET_DIR=/Users/ke-qiao/Developer/project-alia/target`） | 结果 |
| --- | --- |
| `cargo check -p alia-server --lib --offline --locked` | exit 0，生产库当时编译通过，日志 `production-first.log` |
| `cargo test -p alia-server --test config --offline --locked` | exit 0，4/4，通过含新预算边界，日志 `config.log` |
| `cargo test -p alia-server --lib grpc::voice_session::tests --no-run --offline --locked` | exit 101，当时被其它 owned 区域 playback/tests 的 11 处 v3 接口错误阻塞；Root 后续已迁移，尚未由本 agent 复跑，日志 `actor-compile.log` |
| `cargo test -p alia-server --test voice_stream_live --no-run --offline --locked` | exit 101，当前 live fixture 调用了不存在的 `OutputBinding::validate`，日志 `live-compile.log`；按暂停要求保留失败现场 |

最先恢复：修 live fixture 第 234 行不存在的 `binding.validate()`（使用真实现有契约验证，不能放松身份校验），第 545 行 `fixture.count("responses")` 应核对真实表 `delivery_responses`；重编译 actor/live；协调 PG 窗口后跑 actor；最后以真实模型/真实 canonical PG source→Context→Response 运行 live。当前 live 设备输出是明确的 Core 软件状态机 fixture，不能宣称物理音频或完整 LLM/TTS 已验证。共享树完整 all-targets/workspace 由 Root 后续统一验证。


## 额度恢复验证：2026-09-23（本子包定向验证通过）

从整合快照 `ae75a4ee92b1ea2e8c11a1d20c488ba62125d81f` 接续；六个 owned 文件恢复前的 SHA-256 与 2026-09-22 RESULT 完全一致。未改原暂停/恢复生产语义；私有 session runner 新增重验间隔参数，生产入口仍传固定 30s，三个周期重验数据库测试使用 100ms 真实间隔，避免虚拟时间越过正在执行的真实 PG 查询期限。

- live fixture 使用 Core `OutputBinding::is_valid` 与 `OutputRegistration::validate`，保留原 binding/backend 精确匹配；修正真实表名 `delivery_responses`。
- live 通过 `query_control` 等待真实账本 Applied 回执，检查 Pause→Resume→Pause→Confirm、原 Response/Context/backend、不续期的 WAIT，以及协商 30000ms 对配置 60000ms 的限制。设备仍为 Core 软件状态机 fixture。
- actor 所有数据库 case 统一捕获 panic、取消/关闭 Delivery 后删除本用例 schema。migrations/start/recovery 使用生产入口；不跳过恢复，不推进真实数据库所在 runtime 的时钟。
- 新增 Continue 重验 await 期间进入新 speech 的定向回归，验证重验后 drain 丢弃旧 Continue。actor 共 23 项：4 项不接库（2纯函数+2 SessionLease），19 项使用真实 PG，但停在 canonical Response bind 之前。
- 新 retirement 回执：actor 只对已知、未注册 scope 返回 NoNativeOwner；live 精确校验原 binding/backend，并 retire 软件 owner 后返回 Settled，空 progress 表示本 fixture 没有启动 PCM/native write。
- 刷新凭证测试准确命名为 repeated periodic revalidation，3轮凭证各经过4次短真实周期验证；不再声称虚拟时间代表实际超过5分钟。过期/错误签名仍由独立 signed refresh case 检查。

| 本轮命令 | 实际结果 | 日志 |
| --- | --- | --- |
| `cargo test -p alia-server --lib grpc::voice_session::tests --no-run --offline --locked` | exit 0，仅编译/链接，无执行结论 | `actor-compile-final.log` |
| `cargo test -p alia-server --test voice_stream_live --no-run --offline --locked` | exit 0，仅编译/链接，无模型/PG结论 | `live-compile-final.log` |
| `cargo test -p alia-server --test config --offline --locked` | exit 0，4 passed，0 failed，0 ignored | `config.log` |
| 第一轮 `cargo test -p alia-server --lib grpc::voice_session::tests --offline --locked -- --nocapture` | exit 101，20 passed/3 failed/0 ignored；3项虚拟时钟用例在 shutdown 报后台恢复 SQL 超时，实际语义断言未失败；清理回原3schema | `actor-run.log` |
| 修正真实间隔后同23项 actor 命令 | exit 0，23 passed/0 failed/0 ignored，3.26s；正常 start/recovery 与新 retirement 回执 | `actor-real-clock.log` |
| 第一轮 `cargo test -p alia-server --test voice_stream_live --offline --locked -- --ignored --nocapture` | exit 101，0 passed/1 failed/0 ignored，8.36s；第一段Complete在模型8000ms期限超时，尚未进入Pause/Resume断言 | `live-run.log` |
| 原8000ms预算同live命令复跑 | exit 0，1 passed/0 failed/0 ignored，1.85s；完整canonical Response/Context与4条Applied账本回执断言通过 | `live-repeat.log` |
| owned五个Rust文件 `rustfmt --check --edition 2024` 与owned diff whitespace检查 | exit 0 | `rustfmt-final.log`、`diff-check-final.log` |

日志目录 `run/duplex-voice-validation-20260923/`；所有 Cargo 命令使用 `CARGO_TARGET_DIR=/Users/ke-qiao/Developer/project-alia/target`。失败后的 schema 与 before 基线精确一致，只有原3个 pipeline_delivery 历史schema，没有遗留 voice_actor 或 delivery_test schema。

### 首次模型超时的独立诊断

8091 `/health` 再次读取 ready/model_loaded，固定 Easy-Turn/source/Qwen revisions 与交接吻合，CPU float32。production Rust detector默认5000ms，voice live fixture8000ms、event等待10000ms，独立 `easy_turn_live` fixture30000ms；HTTP服务capabilities的inference_timeout为180000ms。临时AcousticCandidate3000ms、配置hold60000ms、本次协商30000ms均未放宽。

三段官方PCM加700ms静音的独立HTTP实测均200且标签正确：Complete音频3256ms，inference501.17ms/wall520.06ms；Backchannel音频912ms，363.06/367.38ms；WAIT音频2560ms，421.90/423.45ms。记录在 `model-capabilities.json`、`model-diagnostic.json`、`model-followup-diagnostics.json`、`model-health-after-diagnostic.json`。首个超时可能与冷缓存或当时负载有关，目前只是推断；复跑未修改8000ms模型预算或任何暂停期限，真实gRPC/PG最终通过：Complete590.66ms、Backchannel400.14ms、WAIT453.08ms（模型报告的inference_ms），测试总1.85s。直接HTTP诊断不另计成正式用例。

本子包最终定向结果为 **23 actor + 4 config + 1 live = 28 unique passed / 0 failed / 0 ignored**；此前失败轮次保留，不叠加重跑次数。范围是Server VoiceSession/正常Delivery supervisor/规范PG账本与固定真实Easy-Turn；live输出为软件fixture，没有实际麦克风、VAD、LLM/TTS、声卡或Pi物理播放证据。两个暂停模式、旧Continue失效、队列背压、source双epoch/原adapter、producer生命周期由actor回归覆盖；live负责canonical bind和真实控制账本。完整workspace、Server/Embedded no-default、真实paired设备及平台验收仍由Root汇总，不能用本子包28项替代。

最后PG交回C：`schemas-after-live-final.txt` 与 `schemas-before.txt` 精确一致，`clients-after-live-final.txt` 为0。所有自有exec已退出，没有启动/停止模型或数据库服务，没有Git提交。精确文件hash与命令摘要见同目录 `RESULT.json`。

## V2 冻结源码补充验证：2026-09-23

Root 的 `run/validation-20260923/final-source-manifest-v2.json` 包含 605 个构建输入，SHA-256 为 `1939a948191002882ba70e2578ad87d266d428b2ba423ccfa31aa178a34998d4`。本 agent 在执行前、PTY 后及模型后逐项校验，均无漂移。此前 actor 23 项的定向日志仍保留为原轮次证据，不改标为 V2 全套重跑。

| V2 实际执行 | 结果 | 证据 |
| --- | --- | --- |
| `cargo test -p alia-server --lib --offline --locked -- --ignored --nocapture --test-threads=1`，真实 PTY、无输出重定向 | exit 0，3 passed / 0 failed / 0 ignored，0.34s | `run/validation-20260923/final-pty-model/pty-v2.log` 与 `pty-v2-result.json` |
| `cargo test -p alia-server --test easy_turn_live --test voice_stream_live --offline --locked -- --ignored --nocapture` | exit 0；Easy-Turn 1 passed，0.59s；VoiceStream 1 passed，1.71s；两个目标均 0 failed / 0 ignored | `run/validation-20260923/final-models-v2.log` 与 `final-models-v2-result.json` |

模型顺序为 easy_turn_live → voice_stream_live；没有追加诊断或预热请求。第一次受测请求为 Easy-Turn Backchannel，inference 586.044ms；随后 VoiceStream Complete / Backchannel / WAIT 分别为 545.904 / 344.788 / 430.174ms。保持原 fixture 模型预算 30000ms / 8000ms，所有暂停预算不变。本轮没有重启服务或清模型缓存，因此不能称为模型冷启动验收；此前首个 8s 超时仍保留。没有把重复的 VoiceStream 通过再次加入前述 28 个独立用例数。

本轮真实 PTY 覆盖 setup 输入释放、CLI 空闲 shutdown 与 UTF-8 console write 的终端恢复。模型测试保持前述软件输出 fixture 边界，不能证明物理播放。PG 前后 schema 完全一致，仍只有原 3 个历史 pipeline_delivery schema（另有系统/public），other clients 为 0；无活动自有任务，PG 已交给 B 的 Linux V2 验证。完整记录见 `run/validation-20260923/final-pty-model/RESULT.json`。
