# 构建记录

更新：2026-09-13。分支：`codex/architecture-refactor`；源代码基线：`3530395`。

用户已授权开始构建，当前 Easy-Turn 只运行在 Server。当前记录是 P1、P2 与运行时基础改造的结果，首轮整体交付尚未完成。前端和 Embedded 通用适配仍按 [台账](deferred.md) 搁置。

## 已提交构建包

| 提交 | 内容 | 已取得的证据 |
| --- | --- | --- |
| `525046f` | 可调用的 Server 启停、可取消 CLI/OOBE、事务迁移、共享音频/轮次领域契约与 Easy-Turn HTTP 边界 | Server 回归；真实 PostgreSQL/pgvector 空库、回滚、恢复、并发、重复及校验和测试；两个真实 PTY 生命周期测试 |
| `b40120a` | 端侧有界播放与独立停止控制、本地 generation 失效、统一 CPAL/ALSA 控制循环 | 播放控制 11 项、已有 audio 集成 9 项通过；Linux ARM64 构建通过 |
| `29e9ab6` | 固定官方源码/权重的常驻 Easy-Turn 服务与可复现依赖 | 14 项服务契约测试；真实 CPU 严格加载及 16 个官方样本 HTTP 推理通过 |
| `33a7c98` | 持续采集 v1 协议、端侧 server_turn 模式、Server 窗口与模型决策、可靠语音入口、可监督交互管线 | 实际 gRPC→Rust→CPU 模型；136 个 Server 常规单元测试及全部 Server 集成测试（带真实数据库）通过；Embedded 无默认特性 98 项通过 |
| `1ecb2ce` | boot/session/generation 播放协议、原子入队失效、凭证续期与解绑关闭、端侧信号退出 | macOS 默认特性整个 workspace 409 项通过；真实 DB 套件通过；两项真实模型测试另跑通过；Server 无默认特性 all-targets 通过；Linux ARM64 端侧变更检查通过 |
| `c257f07` | Embedded 启动失败回滚、可等待的有界关闭、实际监听地址、长 HTTP/2 连接清理及原生任务所有权 | 无默认特性 122 项通过；macOS 默认 all-targets 和 Linux ARM64 无默认 all-targets 检查通过，见 [Embedded 生命周期](embedded-runtime.md) |
| `5aecb38` | 持久 Execution 共用入口、Memory 规范来源与隐私栅栏、可信输入贯穿消息和工具 | 空数据库默认 workspace 500 项；真实 CPU 模型与 gRPC 两项另跑通过；固定提交 Linux ARM64 Server 检查及 152 项库测试、Embedded 默认完整链接通过 |
| `2f8797c` | Server 运行时所有权与故障回滚、Memory 原子批次与批量遗忘、数据库排序的可信 source epoch | 默认 workspace 561 项通过；真实 PTY 与 CPU 模型 4 项另跑通过；固定提交 Linux ARM64 Server 库和真实 PG 共 280 项通过，Embedded 默认完整链接通过 |
| `ee9cbfd` | 规范 Response、逐句音频 Delivery、播放 v2、真实后端提交回执与生命周期监督 | macOS 默认 workspace 636 项通过；4 项真实 PTY/模型另跑通过；固定提交 Linux ARM64 Server + Embedded 库及 Server 集成共 542 项通过，Embedded 默认完整链接通过 |

## TextDelivery 与 D3 身体动作（已冻结）

文本输出已接入共用 DeliveryService，UTF-8 分块先持久化再交给 CLI native writer，保留实际字节前缀和准确取消，模型断流不提交完整助手历史。文本、音频和身体分别记录证据，未将 Submitted 解释为已读、已听到或机构到位。

身体链路包含 Core 严格类型、双向签名 gRPC、可信配置公钥、规范 Response 对应的唯一 dispatch permit、设备共享 boot、control lease/sequence 和唯一 native writer。绑定/voice 撤销、回执淘汰、动画完整预检、部分串口/I²C 写入、旧身份取消和关闭纳入同一生命周期。设备明确拒绝的新身份会留下签名 Rejected；取消携带完整原意图，先退休尚未入队的序号，阻止迟到 preparation 执行。Server 对未确认的原取消限频重试，永不重试 Submit。

独立审查发现并修复两条真实竞态：普通拒绝只返回无签名错误导致 Server 永久等待不存在的命令，以及取消先于 preparation 到达后迟到 Submit 仍可能执行。另收紧 Unknown 的迟到证据校验，已确认 terminal/quiescent 后不能再增长 native 提交量。

定向证据：Motion 45 项、真实签名设备 gRPC 6 项、身体 PostgreSQL 6 项、运行时 HTTP 身体/文本接口 1 项；Text repository 5 项、service 4 项、Pipeline 38 项、CLI 7 项通过。Core 20 项、Server 签名身体/pin/原播放 26 项通过。关闭异常的测试包含已完成 JoinError 不被重复 poll，以及继续 join 其余 owner。

默认 workspace 的完整回归 **720 个不同测试通过、0 失败、5 显式跳过**；内嵌子进程重复执行的一项不再累计。5 项随后另跑通过：真实 PTY 3 项、官方音频→CPU Easy-Turn 和持续 gRPC 2 项。当前模型直接 Backchannel 848 ms；gRPC Complete/Backchannel/Wait 为 508/311/372 ms，仅代表这次模型调用，不是端到端声学延迟。两端 no-default-features all-targets 检查通过，60 个变更 Rust 文件格式检查通过。

整包回归修复了一处新测试 fixture：删除本 schema 的表会沿 search_path 回退到其他迁移测试安装的 public 同名表；故障注入改为本 schema 内列定义故障，HTTP 503 与诊断隐藏已通过。末次恢复修复另跑身体仓储 **6 项**、Server runtime **33 项**与签名客户端/pin/播放 **26 项**，全部通过，验证 `latest_checkpoint` 恢复已确认的 quiescence，同时原 Unknown/status/progress 不改写、原动作不重播。这些是对应既有用例的末次回归，不重复累计。

本包新增迁移 `m20260908_000005_text_delivery.sql` 与 `m20260913_000006_body_delivery.sql`，没有修改已提交迁移。前端没有进入构建。真实执行器和音频呈现仍需硬件信息；当前旧 `neck.anim` / `neck2.anim` 超过 v1 短动作时长/帧界限，会明确拒绝，未截断或修改原素材。短动作资产与真实机构验收仍属身体产品化工作。

## 已提交的 Response / Delivery 与播放 v2（ee9cbfd）

本包将规范 Response、逐句音频 Delivery、投递监督和设备回执接入同一个 Server runtime。Response 与唯一助手历史原子提交；输出前持久化意图、原始身份与发送许可。重复调用查询已有事实，过期恢复不重播。CLI 文本也在外发前建立 Response，独立 TextDelivery 发送监督继续进入下一包。接口与状态见 [Delivery API](delivery-api.md)。

播放 RPC 升级为 `alia.playback.v2`，Start 握手后才发送媒体。端侧输入 End 后仍保留观察；CPAL / ALSA 按实际后端提交计数，源帧通过连续 sinc 重采样映射。精准取消等待已进入的输出写入边界收尾并返回原序号最终快照；本地超时不伪造设备回执，迟到证据独立审计。Submitted 不冒充 Presented，PostSpeech 仍需真实呈现证据。

交叉故障审查补齐了三处设备竞态：取消先于在途 writei/callback 返回、End ACK 早于输入 lease 释放，以及等待 Start 时撤权仍先推进 generation。Server 另修复心跳暂停持锁媒体 future 的死锁，保留原始 native/future 直到真实 join；panic 时尝试记录 Unknown，依赖继续隔离持有。Affect 数据库写入也从未托管 spawn 改为实例有界队列，关闭先排空后释放数据库。

当前定向证据：Delivery repository 14 项、service 12 项；Pipeline lib 35 项、memory capture 6 项、pipeline integration 17 项；Embedded 无默认特性 142 项和 Server 播放 8 项；root 真实 PG/HTTP/native Delivery runtime 4 项、Affect writer/清除 runtime 7 项；Expression 库 41 项。各组存在被 workspace 收录的同名用例，不能将定向和整包结果重复累计。

最终默认 workspace 在独立空数据库 `alia_refactor_delivery_final` 上 **636 个不同测试通过、0 失败、4 显式跳过**；日志中已有测试启动的 1 次子进程内部运行不重复累计。Server runtime 入口 32 项、Delivery repository 14 项、Delivery service 12 项全部通过。Server 无默认特性 all-targets 检查、60 个变更 Rust 文件格式检查及 `git diff --check` 通过。

4 项跳过测试均在冻结源码后显式运行通过：真实 PTY 2 项，以及真实 CPU Easy-Turn 和 gRPC 持续语音 2 项。直接 Backchannel 为 307 ms；gRPC Complete / Backchannel / Wait 为 476 / 305 / 381 ms，仅为模型调用时间，不代表端到端声学延迟。Linux ARM64 将使用整包固定提交归档验证。硬件呈现、AEC、身体 D3、完整 ContextLease 撤销及 Companion 仍是未完成主线，见 [Delivery 边界](delivery-design.md)、[身体命令设计](body-command-design.md)与[上下文失效设计](context-revocation-design.md)。

```sh
ALIA_TEST_DATABASE_URL='postgresql://alia_test:alia_test@127.0.0.1:32768/alia_refactor_delivery_final' \
  cargo test --workspace --locked --no-fail-fast
cargo check -p alia-server --no-default-features --all-targets --locked
# 在真实 PTY 内执行：
cargo test -p alia-server --lib --locked -- --ignored --nocapture --test-threads=1
ALIA_EASY_TURN_PCM="$PWD/run/easy-turn-backchannel.pcm" \
ALIA_EASY_TURN_PCM_DIR="$PWD/run/easy-turn-stream-pcm" \
  cargo test -p alia-server --test easy_turn_live --test voice_stream_live --locked -- --ignored --nocapture
```

## Server runtime、原子 Memory 和来源版本整合包（2f8797c）

本包建立可嵌入的 ServerRuntime：启动失败完整回滚、实例独立取消、连接和后台 actor 排空、统一关闭期限，以及超时后继续保留真实 worker 和依赖的 ShutdownHandle。工具目录写入或显式 Provider 配置出错会拒绝 ready。现有 WS 是管理员诊断流，依据数据库当前账户和角色授权、定期复核，并随宿主关闭实际连接。详见[运行时契约](server-runtime-design.md)。

Memory 的 LLM 整理改为一次完整原子批次，模型只可退役记录；用户批量遗忘在一个事务内处理全部目标。最终回归还复现并修复了时间戳的因果漏洞：当 Server 时钟比数据库快 60 秒，旧请求可能在 clear 后重新写入记忆。现在所有可信入口在数据库接纳时固定 source_privacy_epoch，排队、Execution、用户消息和后台抽取保留该版本；版本变化后旧请求被拒绝，新请求不受快慢钟影响。具体边界见[来源接纳](source-admission.md)和[Memory 记录](memory-findings.md)。

最终默认特性 workspace 为 **561 passed、0 failed、4 ignored**。日志另含故障测试启动的 1 次子进程内部运行，统计去重；Server 常规 lib 161 项、Execution repository/service 19/27 项、Memory 隐私 24 项、后台 13 项、Server runtime 21 项均通过。新来源迁移已在本任务专用的全新数据库上实际运行。Server 无默认特性 all-targets 检查也通过。

4 项 ignored 测试均随后显式运行通过：两个真实 PTY 退出与终端恢复测试，以及真实 CPU Easy-Turn 与 gRPC 持续语音两项测试。模型直接 Backchannel 322 ms；gRPC Complete/Backchannel/Wait 为 511/337/405 ms，均为模型调用时间，不是端到端声学延迟。最终回归同时修正两项过时 Provider 测试，明确未知配置必须失败、noop 必须显式选择。

```sh
ALIA_TEST_DATABASE_URL='postgresql://alia_test:alia_test@127.0.0.1:32768/alia_refactor_runtime_v1' \
  cargo test --workspace --locked --no-fail-fast
cargo check -p alia-server --no-default-features --all-targets --locked
# 在真实 PTY 内执行：
cargo test -p alia-server --lib --locked -- --ignored --nocapture --test-threads=1
ALIA_EASY_TURN_PCM="$PWD/run/easy-turn-backchannel.pcm" \
ALIA_EASY_TURN_PCM_DIR="$PWD/run/easy-turn-stream-pcm" \
  cargo test -p alia-server --test easy_turn_live --test voice_stream_live --locked -- --ignored --nocapture
```

本包随后已对固定 `2f8797c` 完成 Linux ARM64 验证：Server 默认 all-targets、161 项库测试、119 项真实 PG 集成和 Embedded 默认完整构建/链接均通过。排除的 2 项 PTY 在 macOS 已验证。见[固定提交平台记录](platform-validation.md)。持久 Delivery、播放后端提交证据、完整遗忘传播及硬件联调继续属于未完成主线。

## 先前 P2 整合包（5aecb38）

Execution 已成为工具入口共用的持久执行边界，拥有意图、确认、Outbox、认领、结果、幂等与 unknown 恢复。提交/等待/关闭有整体期限；批准已提交但 ACK 丢失时仍保留会话取消所有权。操作入口与证据见 [Execution API](execution-api.md) 和 [数据库验证](execution-validation.md)。

Memory 在真实事务内处理用户隔离、规范 revision、隐私 epoch、来源关系、递归删除和投影失效。可信 Server 输入时间在排队前固定；用户消息在模型/工具之前提交，Execution 保留真实来源 ID，AI MemoryStore 在执行时重新验证原来源而不能制造新时间。语义去重保留中文、否定和数值标点。详见 [Memory 记录](memory-findings.md) 和 [通道身份](channel-identity.md)。

最终默认特性 workspace 结果为 **500 passed、0 failed、4 ignored**，包含两端实际默认特性编译/链接。Execution repository 19 项、service 25 项、Memory 原有 13 项、Memory 隐私 14 项、gRPC 身份 6 项和迁移 7 项全部通过。两项 ignored 的真实模型测试随后单独运行通过；另外两项 PTY 测试本轮未重复执行，先前基础包已实际通过。

该提交的 Server 无默认特性 all-targets 检查也通过（18.05 秒）。随后已对固定 `5aecb38` 获得 Linux ARM64 容器原生检查、152 项 Server 库测试及 Embedded 默认特性完整构建链接证据，详见[平台验证](platform-validation.md)。这些结果不覆盖下一包未提交的运行时变化，也不代表 Windows 或 Pi 音频外设已通过。

```sh
ALIA_TEST_DATABASE_URL='postgresql://alia_test:alia_test@127.0.0.1:32768/alia_refactor_p2_verify2' \
  cargo test --workspace --locked

ALIA_EASY_TURN_PCM="$PWD/run/easy-turn-backchannel.pcm" \
ALIA_EASY_TURN_PCM_DIR="$PWD/run/easy-turn-stream-pcm" \
  cargo test -p alia-server --test easy_turn_live --test voice_stream_live --locked -- --ignored --nocapture
```

数据库为本任务独有的临时实例。首次全空数据库运行发现 pgvector 被安装到临时 schema，已将数据库扩展准备固定到 public，并从另一个无扩展、无表的空库完成上述 500 项回归；未修改旧迁移内容。该包真实 CPU 模型直接 Backchannel 约 827 ms，gRPC Complete/Backchannel/Wait 约 497/326/396 ms，仍只是模型调用时间，不含 LLM/TTS 和声学呈现。

`33a7c98` 中嵌入渠道测试的 feature 属性位置随后修正；修正后默认特性测试及 `--no-default-features --all-targets` 检查均通过。记录验证时应以修正后的分支为准。

## 运行和验证入口

- 模型安装、固定版本、启动和官方样本复现：[Easy-Turn README](../../services/easy-turn/README.md)。模型只在显式 prepare 时下载，权重保存在忽略的 `run/models/`。
- 模型真实结果与 MPS 失败依据：[组件验证](../../services/easy-turn/VALIDATION.md)。本机 CPU 样本通过，当前依赖栈的 MPS `torch.isin` 不可用，启动会明确失败；CPU 路线不依赖 MPS。
- Server 配置使用 `config/server.example.d/providers/easy_turn.toml` 中的 `[providers.easy_turn] enabled = true`，endpoint 默认 `http://127.0.0.1:8091`。
- Embedded 使用 `[speech] mode = "server_turn"`、16 kHz 单声道。唤醒后在播放期间持续上传；VAD 只提供活动证据。没有静默回退成端侧模型。
- 模拟与领域测试验证协议、控制、取消及队列；它们不等同于声卡、AEC 或真机双讲验收。
- 连续采集、认证续期、播放隔离和关闭的实际契约：[语音协议 v1](voice-protocol.md)。

本次真实验证宿主为 Apple M5 Max / 64 GiB / macOS 26.6.2 / arm64。尚未得到用户确认其就是最终部署 Mac。Python 3.10.20、Torch 2.2.2 的 CPU 首次样本推理约 1,449 ms，其余 15 个样本约 286–537 ms；这是特征提取和模型执行耗时，不含 LLM/TTS、网络设备播放或模型加载。

服务端完整测试使用独立的临时 PostgreSQL/pgvector，未使用现有开发数据库。针对固定 `ef07d43` 的 Linux ARM64 Server 默认特性构建及 136 项库测试、Embedded 默认 Sherpa 特性完整构建链接均已通过，详见 [隔离平台验证](platform-validation.md)。新播放变更另外通过 Linux ARM64 Embedded 无默认特性检查；不能把这些容器结果推导成 Pi 音频外设通过。

`1ecb2ce` 的完整 workspace 测试启用了两端默认特性：409 passed、0 failed、4 ignored。四项跳过中，两项真实 Easy-Turn 测试随后显式运行并通过；另外两项真实 PTY 测试未重复运行（前一基础包已实际通过）。最新真实 gRPC 测试还核对了 Complete 2、Wait 3、Close 的原始会话身份；对应 CPU 模型执行约 508 / 328 / 426 ms，不代表端到端声学延迟。

## 尚未完成的主线

这些项目仍属于首轮范围，不能归入用户要求的延期项：

1. AEC、实际播放参考时钟、可恢复暂停、真实设备呈现回执、双讲与自然插话质量。当前 WAIT 明确停止当前回答，尚不能从实际播放游标恢复。
2. TextDelivery 和 D3 的固定提交跨平台验证、短动作资产及真实呈现/位置证据，依据 [Delivery 设计](delivery-design.md) 实施。已实现的唯一发送与软件提交不能代表硬件已完成。
3. 缓存与在途 ContextPack/输出的遗忘传播、Companion 状态与主动表达，以及端侧动作安全与恢复。LLM 原子整理和批量遗忘已在本轮接通并通过专项回归。
4. 其他 Server 平台、Pi 真机联调和至少 24 小时混合负载。Server 和 Embedded 已建立可调用运行时；未来新增 Delivery 等模块仍需纳入同一所有权及关闭契约。

播放网络身份栅栏和持续会话凭证续期已在 `1ecb2ce` 实现。续期测试推进到虚拟 365 秒，同时用真实签名检查错误签名/身份/scope 和 301 秒旧 iat 被拒；绑定变更由设备立即及每 250 ms 复核，覆盖上传结束后队列尾部。真实设备长时会话仍需要单独验收。

模型观测成功、文本回复生成、音频被接收、提交后端和实际呈现分别记录。播放 v2 已提供 accepted/submitted 的软件证据，presented 仍未知。旧全局中断错误提示旁路已停止外发，无法验证的表达 JSON 不再升级 completed；身体 signed v1 已接入，带作用域提示音仍待接入，不能称全链路已证明声学完成。旧广播事件入口仍有可见丢弃策略，尚未全部替换为可靠命令入口。

## 待补充的实机信息

需要 Pi 系统版本、测试连接方式、麦克风/声卡/扬声器与 AEC 能力、执行器与反馈/限位信息，以及最终 LLM/TTS/Embedding 服务配置。已在对话中提出，独立代码与协议构建继续进行；依赖这些信息的实机结论保持未验收。
