# L10 Provider 初始化依赖交接

实施阶段的交接记录保留如下；当前完成状态和验证结果以末尾 **Root 最终验收** 为准。

日期：2026-10-10（Asia/Shanghai）

基线：`cb19992`；分支：`codex/architecture-refactor`。

本文件由 `/root/l10_provider_dependencies` 维护；本域实现与静态核对完成，运行验证由 Root 统一执行。

## Checklist：7/7

- [x] P01 审计六个模型适配器的实际初始化依赖、工厂、公开构造与测试消费面。
- [x] P02 OpenAI / Anthropic / Gemini LLM 构造绑定选定 profile/provider section。
- [x] P03 OpenAI Embedding 构造绑定选定 profile/provider section；保留模型维度推导与 profile identity。
- [x] P04 Whisper 构造绑定 provider section；GPT-SoVITS 构造绑定 provider、default voice、voice profiles、expression styles。
- [x] P05 全部 `init(&mut self)` 无参；删除本域 `ModuleContext` / `ServerConfig` 依赖、无参构造、`for_profile` 与 adapter `Default`；迁移目录内 fixtures。
- [x] P06 TTS 补本地 HTTP 初始化副作用回归；静态行为比较 19/19、文件格式检查、范围内 diff check 通过。
- [x] P07 Root 完成工厂/Bootstrap/外部测试迁移，统一运行默认与无默认配置测试，并记录同源证据。

## 最终 API

- `OpenAiLlm::new(profile_id, &LlmProfileSection, &OpenAiProviderSection) -> anyhow::Result<Self>`。
- `AnthropicLlm::new(profile_id, &LlmProfileSection, &AnthropicProviderSection) -> anyhow::Result<Self>`。
- `GeminiLlm::new(profile_id, &LlmProfileSection, &GeminiProviderSection) -> anyhow::Result<Self>`。
- `OpenAiEmbedding::new(profile_id, &LlmProfileSection, &OpenAiProviderSection) -> anyhow::Result<Self>`。
- 以上 `profile_id: impl Into<String>`；chat/embedding kind 校验保留在构造；选定 provider 的枚举解析由 Bootstrap 工厂负责，具体 provider 类型由参数保证。
- `WhisperCppStt::new(&WhisperCppProviderSection) -> Self`。
- `GptSovitsTts::new(&GptSovitsProviderSection, default_voice: &str, &BTreeMap<String, VoiceProfileSection>, &BTreeMap<String, ExpressionStyleSection>) -> Self`。

不保存全局配置，不增加配置包装层、选项、动态 trait、依赖或 feature。原来的单份 HTTP client 与运行配置锁继续承担请求及热设置。

## 行为保持与副作用

六个旧 init 均只读取 `ctx.config`，没有数据库、服务 slot 或 EventBus 依赖。构造只绑定内存配置；`Client::new()` 与之前一样在构造阶段创建。TTS 默认 voice 缺失仍采用 `VoiceProfileSection::default()`，随后把 default voice 加入 voice map；非 GPT-SoVITS 默认 voice 仍警告并使用共享 voice 字段。

GPT-SoVITS 的 `configure_remote_weights()` 仍只在受生命周期清理保护的 `init()` 及原有相关热设置流程调用。初始化请求次序仍为 GPT weights、SoVITS weights；错误仍记录警告并返回成功，继续使用服务端现有模型。不能把权重设置提前到 Bootstrap 构造阶段。

Whisper `model` 字段仍不参与适配器请求；初始 language 仍为 `auto`，只有原有热设置更新它。本轮没有更改该既有行为。

profile/provider ID、设置键、健康检查、请求/流处理、超时、provider 错误分类、Embedding 维度与 identity、TTS 音频预算和所有热设置实现均保持。静态对照 `cb19992` 的六个 production health/hotsettings/protocol 尾段逐字一致，TTS 权重 HTTP/response budget 函数逐字一致。

## 新增回归与静态证据

`providers::tts::gpt_sovits::tests::http::remote_weights_wait_for_init_and_setup_failure_is_nonfatal` 使用本地随机端口，无真实模型：

1. 配置包含 mock 权重路径，构造后 TCP listener 尚无连接。
2. `init()` 才按顺序发送两个权重请求并保留 query 参数。
3. 分别验证成功和第二次请求返回 HTTP 500，初始化均完成。
4. fixture 关闭本地 HTTP server。

目录内旧 HTTP fixtures 已改为显式配置构造，不通过全局 context 初始化。

静态结果：`run/l10-provider-20261010/static-audit.json`，19/19。执行过 scoped `rustfmt --edition 2024 --check` 和 `git diff --check`，通过。未运行 Cargo、PG、容器、真实模型或外部服务；没有提交。

## Root 待衔接

- `bootstrap/factory.rs`：从已有选定 provider ref 匹配具体枚举，传入 profile/provider section；STT/TTS 工厂增加实际配置参数。
- `bootstrap/builder.rs`：工厂使用加载完持久策略后的配置；清理保护下无参 init 的时序保持。
- `tests/provider_adapters.rs`：唯一外部直接调用 `WhisperCppStt::new()` 的 fixture 改为显式 `WhisperCppProviderSection::default()`。
- Root 运行完整验证后再勾 P07，不能把静态检查计作运行通过。

本域没有未解决的实现缺口；运行结论与跨目录集成在 Root 的最终证据中确认。

## 追加：L10 架构守卫（4/4）

- [x] G01 `ModuleContext` 加入所有 Server src/tests/examples 的退役符号检查，覆盖声明、引用、别名和重导出。
- [x] G02 `init` 只允许 `&mut self`（允许格式化尾逗号），包含生命周期 trait 与测试实现；换名聚合依赖参数同样拒绝。20 个确定迁移的具体实现文件拒绝 `ServerConfig` 引用与 alias。保留 Bootstrap 配置选择、Memory Context/Appraisal policy 转换、adapter 中既有配置转换和外部测试配置装配。
- [x] G03 新增 `create_rule_engine`、`create_scheduler`、`create_tools_with_sensors` 固定工厂声明/调用禁令；正负 self-test 与 scoped diff check 通过。
- [x] G04 Root 清除剩余跨目录初始化消费后，最终 full guard 通过并收录最终源码证据。

守卫路径：`scripts/check-server-boundaries.py`。

实际 SHA-256：`07c9330452ac47f918b2e6ccb3926b5b2db33cab65aab6436d71f625e42db6cf`。

证据：`run/l10-provider-20261010/guard-audit-v1.json`。自检前后脚本哈希一致；self-test exit 0，diff check exit 0。该次 full guard exit 1，保留了当时尚未完成的 Root 集成残余：

- `src/api/channels.rs`：旧 Context 引用。
- `tests/cache_context.rs`、`tests/duplex_playback.rs`：旧 Context fixtures。
- `tests/execution_service.rs`：旧 Context 注释。
- `tests/server_runtime.rs`：旧 Context import 和带参数 init。

这些残余属于 Root 的已分配迁移范围；本代理没有修改其他代理或 Root 的文件。该次 full guard 的失败不是最终运行结论，应另存后续成功结果而不改写此份中间记录。

## Root 最终验收（覆盖前述待验证状态）

源码 `4519e757542f644e9eb385d4055551546bf7abd3`；最终 V2 `d361dad3057717c4efb08b62ef8d98084754b276e420d0c2f842923a0e0f3e41`，**637/637** 输入匹配 Git blob。macOS workspace **1058/0/5**、真实 PTY **3/0**、无默认 Insights availability **2/0**、新增提前拒绝回归 **1/0**；Linux ARM64 默认 Server 全包 **776/0/5**。两端 Server 无默认 all-targets、Linux release/native link、fmt 和边界正负自检均通过。所有最终运行均无源码漂移。

本轮 **10/10**，L09—L12 **2/4**，R01—R06 仍 **2/6**。专用 PostgreSQL 和两个 Linux holder 已停止，数据库零残留测试 schema、零其他连接；无待接回的 Cargo/PG 作业。V1 无 feature 回归的报告包装断言失败已原样保留；V2 仅改这条测试断言，636 个输入及全部生产源码与 V1 相同。详见 [最终验证](../../layering-2026-10-10.md#最终验证与证据) 与 [证据索引](evidence-index.json)。

下一轮从 L11 跨域事务所有者开始，L12 和 Skills reload TODO 保留。真实模型、Pi/声学/机械、Linux 全工作区与部署 smoke 未在本轮验证；用户 5433 未访问，默认 Shadow 和政策保持。未推送或创建 PR，未使用 reset 卡。
