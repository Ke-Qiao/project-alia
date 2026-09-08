# 构建记录

更新：2026-09-08。分支：`codex/architecture-refactor`；源代码基线：`3530395`。

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

## 本轮 P2 整合包

Execution 已成为工具入口共用的持久执行边界，拥有意图、确认、Outbox、认领、结果、幂等与 unknown 恢复。提交/等待/关闭有整体期限；批准已提交但 ACK 丢失时仍保留会话取消所有权。操作入口与证据见 [Execution API](execution-api.md) 和 [数据库验证](execution-validation.md)。

Memory 在真实事务内处理用户隔离、规范 revision、隐私 epoch、来源关系、递归删除和投影失效。可信 Server 输入时间在排队前固定；用户消息在模型/工具之前提交，Execution 保留真实来源 ID，AI MemoryStore 在执行时重新验证原来源而不能制造新时间。语义去重保留中文、否定和数值标点。详见 [Memory 记录](memory-findings.md) 和 [通道身份](channel-identity.md)。

最终默认特性 workspace 结果为 **500 passed、0 failed、4 ignored**，包含两端实际默认特性编译/链接。Execution repository 19 项、service 25 项、Memory 原有 13 项、Memory 隐私 14 项、gRPC 身份 6 项和迁移 7 项全部通过。两项 ignored 的真实模型测试随后单独运行通过；另外两项 PTY 测试本轮未重复执行，先前基础包已实际通过。

最新 Server 无默认特性 all-targets 检查也通过（18.05 秒）。本包尚未取得新的 Linux/Windows 原生运行结果，不能把早前固定提交的平台结果当成新源码已验证。

```sh
ALIA_TEST_DATABASE_URL='postgresql://alia_test:alia_test@127.0.0.1:32768/alia_refactor_p2_verify2' \
  cargo test --workspace --locked

ALIA_EASY_TURN_PCM="$PWD/run/easy-turn-backchannel.pcm" \
ALIA_EASY_TURN_PCM_DIR="$PWD/run/easy-turn-stream-pcm" \
  cargo test -p alia-server --test easy_turn_live --test voice_stream_live --locked -- --ignored --nocapture
```

数据库为本任务独有的临时实例。首次全空数据库运行发现 pgvector 被安装到临时 schema，已将数据库扩展准备固定到 public，并从另一个无扩展、无表的空库完成上述 500 项回归；未修改旧迁移内容。最新真实 CPU 模型直接 Backchannel 约 827 ms，gRPC Complete/Backchannel/Wait 约 497/326/396 ms，仍只是模型调用时间，不含 LLM/TTS 和声学呈现。

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
2. 规范 Response / Delivery 事实、投递取消和呈现证据、表达回执与身份栅栏，依据 [Delivery 设计](delivery-design.md) 实施。Execution 持久账本已完成本包整合，不代表投递或硬件完成。
3. Memory 的 LLM 原子整理/批量遗忘、缓存与在途 ContextPack/输出的遗忘传播、Companion 状态与主动表达，以及端侧动作安全与恢复。
4. Server 装配、后台生产者和长连接的完整生命周期，依据 [Server 生命周期设计](server-runtime-design.md) 实施。Embedded 的独立运行时包已经提交；Server 在 Execution 返回未完成时仍需保留整个依赖链继续清理。
5. 其他 Server 平台、Pi 真机联调和至少 24 小时混合负载。

播放网络身份栅栏和持续会话凭证续期已在 `1ecb2ce` 实现。续期测试推进到虚拟 365 秒，同时用真实签名检查错误签名/身份/scope 和 301 秒旧 iat 被拒；绑定变更由设备立即及每 250 ms 复核，覆盖上传结束后队列尾部。真实设备长时会话仍需要单独验收。

模型观测成功、文本回复生成、音频被接收、音频提交给设备和设备实际呈现必须分别记录。当前主播放协议只证明 accepted；旧 direct response/表达旁路仍有估算完成或错误升级回执的问题，已在 Delivery 设计中明确列出，不能称全链路已证明声学完成。旧广播事件入口仍有可见的丢弃策略，尚未全部替换为可靠命令入口。

## 待补充的实机信息

需要 Pi 系统版本、测试连接方式、麦克风/声卡/扬声器与 AEC 能力、执行器与反馈/限位信息，以及最终 LLM/TTS/Embedding 服务配置。已在对话中提出，独立代码与协议构建继续进行；依赖这些信息的实机结论保持未验收。
