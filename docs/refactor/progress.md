# 构建记录

更新：2026-09-08。分支：`codex/architecture-refactor`；源代码基线：`3530395`。

用户已授权开始构建，当前 Easy-Turn 只运行在 Server。当前记录是 P1 与并行基础改造的结果，首轮整体交付尚未完成。前端和 Embedded 通用适配仍按 [台账](deferred.md) 搁置。

## 已提交构建包

| 提交 | 内容 | 已取得的证据 |
| --- | --- | --- |
| `525046f` | 可调用的 Server 启停、可取消 CLI/OOBE、事务迁移、共享音频/轮次领域契约与 Easy-Turn HTTP 边界 | Server 回归；真实 PostgreSQL/pgvector 空库、回滚、恢复、并发、重复及校验和测试；两个真实 PTY 生命周期测试 |
| `b40120a` | 端侧有界播放与独立停止控制、本地 generation 失效、统一 CPAL/ALSA 控制循环 | 播放控制 11 项、已有 audio 集成 9 项通过；Linux ARM64 构建通过 |
| `29e9ab6` | 固定官方源码/权重的常驻 Easy-Turn 服务与可复现依赖 | 14 项服务契约测试；真实 CPU 严格加载及 16 个官方样本 HTTP 推理通过 |
| `33a7c98` | 持续采集 v1 协议、端侧 server_turn 模式、Server 窗口与模型决策、可靠语音入口、可监督交互管线 | 实际 gRPC→Rust→CPU 模型；136 个 Server 常规单元测试及全部 Server 集成测试（带真实数据库）通过；Embedded 无默认特性 98 项通过 |

`33a7c98` 中嵌入渠道测试的 feature 属性位置随后修正；修正后默认特性测试及 `--no-default-features --all-targets` 检查均通过。记录验证时应以修正后的分支为准。

## 运行和验证入口

- 模型安装、固定版本、启动和官方样本复现：[Easy-Turn README](../../services/easy-turn/README.md)。模型只在显式 prepare 时下载，权重保存在忽略的 `run/models/`。
- 模型真实结果与 MPS 失败依据：[组件验证](../../services/easy-turn/VALIDATION.md)。本机 CPU 样本通过，当前依赖栈的 MPS `torch.isin` 不可用，启动会明确失败；CPU 路线不依赖 MPS。
- Server 配置使用 `config/server.example.d/providers/easy_turn.toml` 中的 `[providers.easy_turn] enabled = true`，endpoint 默认 `http://127.0.0.1:8091`。
- Embedded 使用 `[speech] mode = "server_turn"`、16 kHz 单声道。唤醒后在播放期间持续上传；VAD 只提供活动证据。没有静默回退成端侧模型。
- 模拟与领域测试验证协议、控制、取消及队列；它们不等同于声卡、AEC 或真机双讲验收。

本次真实验证宿主为 Apple M5 Max / 64 GiB / macOS 26.6.2 / arm64。尚未得到用户确认其就是最终部署 Mac。Python 3.10.20、Torch 2.2.2 的 CPU 首次样本推理约 1,449 ms，其余 15 个样本约 286–537 ms；这是特征提取和模型执行耗时，不含 LLM/TTS、网络设备播放或模型加载。

服务端完整测试使用独立的临时 PostgreSQL/pgvector，未使用现有开发数据库。Linux ARM64 Embedded 检查运行在隔离的官方 Rust 容器中，使用 `--no-default-features`；不能由此推导默认 Sherpa 原生依赖或 Pi 音频外设已通过。

## 尚未完成的主线

这些项目仍属于首轮范围，不能归入用户要求的延期项：

1. 播放的网络身份栅栏：把原始 boot/session/generation 传到输出协议，拒绝迟到旧帧和旧停止请求；本地 generation 单独不足以保证这一点。正在实施下一包。
2. 持续会话凭证续期：已有签名的时间新鲜度限制为 300 秒；当前固定凭证重验会结束较长会话。需要续期协议及绑定撤销测试，不能删除过期检查。
3. AEC、实际播放参考时钟、可恢复暂停、真实设备呈现回执、双讲与自然插话质量。当前 WAIT 明确停止当前回答，尚不能从实际播放游标恢复。
4. 独立 Execution / Delivery 事实、四类生命周期、事务 Outbox、幂等、unknown 结果与恢复。当前可监督工具任务与普通 trace 不等同于持久执行账本。
5. Memory / Companion 状态归属、删除失效、主动表达策略、端侧动作安全与恢复的进一步重构和故障验证。
6. 默认原生依赖构建、其他 Server 平台、Pi 真机联调和至少 24 小时混合负载。

模型观测成功、文本回复生成、音频被接收、音频提交给设备和设备实际呈现必须分别记录；目前未以估算时长证明声学播放完成。旧广播事件入口仍有可见的丢弃策略，尚未全部替换为可靠命令入口。

## 待补充的实机信息

需要 Pi 系统版本、测试连接方式、麦克风/声卡/扬声器与 AEC 能力、执行器与反馈/限位信息，以及最终 LLM/TTS/Embedding 服务配置。已在对话中提出，独立代码与协议构建继续进行；依赖这些信息的实机结论保持未验收。
