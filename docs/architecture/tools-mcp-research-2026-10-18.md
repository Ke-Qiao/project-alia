# Rig 工具构建与 MCP 接入研究

实施更新：同日已接入 Rig Tool／单 DTO Schema／统一 Execution，以及 rmcp 3.5.1 的 stdio 和 HTTP 客户端。当前配置、身份、限额和未支持项以 [MCP 工具操作说明](../operations/mcp-tools.md)为准。定向工具 fixture 已通过；生产 MCP 服务与真实模型尚未验收。

调研日期：2026-10-18。以下正文保留实施前设计与固定源码证据；其中计划不等于已经实现的功能。调研阶段只读取 Alia 和发布包源码，没有修改 Cargo.toml、安装新运行环境或连接第三方 MCP 服务。模型和 Extism 的选型由其他文档展开，本文不继续研究 fetch。

## 建议与固定版本

采用 **Rig 0.44 的 Tool/动态 handler + schemars 1.2.3 + jsonschema 0.59.0 + 官方 rmcp 3.5.1**。一个工具目录保存 Rig 标准定义、实际 handler 和 Alia 的执行政策；模型只看到这个目录的投影，所有调用由 Execution 认领后派发。内置工具直接调用 Rust handler，MCP 只负责接入外部服务，不需要将内置工具重新包装成本机 MCP。

优先直接实现一个面向 Alia Execution 的 rmcp adapter。不是因为跨 major 本身不可接受，而是现有 rig-rmcp 的超时、目录刷新和通用失败模型仍需改造成 Alia 的副作用证据语义；直接接 rmcp 更容易让原执行 owner、连接代次和账本身份只有一个来源。可以将通用结果转换、协议升级等改进提交到 rig-rmcp 上游，但本轮没有发送此类消息。

| 包 | 核查版本与源码提交 | 事实与影响 |
| --- | --- | --- |
| rig-core / rig-derive / rig-rmcp | 0.44.0；`2d1071e0e0c602f994e9d6cfd8cf5faea6f53650` | MIT；Rig MSRV 1.95.0。以下 API 以发布包为准，不混用旧版教程。 |
| rmcp | 3.5.1；`79437f291b2c44053d00dcd5db969fd0cca7c887` | manifest 标 Apache-2.0，MSRV 1.88；固定提交 LICENSE 说明正在由 MIT 转 Apache，部分原贡献保留 MIT，分发应保留适用 notices。 |
| schemars | 1.2.3；`4791bb7352555d339e45658ca09c092ed9fa1f5d` | MIT；发布包 MSRV 1.74。一个 Rust 参数 DTO 同时负责反序列化与 Schema。 |
| jsonschema | 0.59.0；`d59ab79cb6eab73bb2d3caa6d1b4e538ca81567f` | MIT；MSRV 1.85；默认启用 HTTP/file 引用解析，必须显式关闭。 |

上述提交来自各发布包 `.cargo_vcs_info.json`。可复查 [Rig 发布包](https://static.crates.io/crates/rig-core/rig-core-0.44.0.crate)、[桥接发布包](https://static.crates.io/crates/rig-rmcp/rig-rmcp-0.44.0.crate)、[rmcp 发布包](https://static.crates.io/crates/rmcp/rmcp-3.5.1.crate)、[schemars 发布包](https://static.crates.io/crates/schemars/schemars-1.2.3.crate)、[jsonschema 发布包](https://static.crates.io/crates/jsonschema/jsonschema-0.59.0.crate)、[rmcp 固定 LICENSE](https://github.com/modelcontextprotocol/rust-sdk/blob/79437f291b2c44053d00dcd5db969fd0cca7c887/LICENSE)。这是核查版本组合，不是声称所有包都是最新版本。

## 工具构建：只定义一次参数

Rig 0.44 的 `Tool` 具有 `Args: Deserialize`、`Output: IntoToolOutput` 和 `call(&self, &mut ToolContext, Args)`；元数据是 `description()` 与 `parameters()`。`Tool` 自己不要求 `Args: JsonSchema`，也不会自动执行 JSON Schema 的范围、长度等校验。`ToolAdapter<T>` 将静态 Tool 变成 `ErasedHandler`；`DynamicTool::new_with_context` 接受动态名称、Schema 和异步回调，`into_parts()` 可取出相同的定义、handler 和 liveness。[Tool 与 DynamicTool](https://github.com/0xPlaygrounds/rig/blob/2d1071e0e0c602f994e9d6cfd8cf5faea6f53650/crates/rig-core/src/tool/contextual.rs)、[ToolAdapter](https://github.com/0xPlaygrounds/rig/blob/2d1071e0e0c602f994e9d6cfd8cf5faea6f53650/crates/rig-core/src/serve/adapters.rs)

建议内置业务工具统一采用显式参数 DTO：

```rust
#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct MemorySearchArgs {
    /// 要查找的内容
    #[schemars(length(min = 1, max = 4096))]
    query: String,
}
```

实现 Rig `Tool` 时 `type Args = MemorySearchArgs`；`parameters()` 返回从这个类型生成并缓存的 Schema。显式使用 `SchemaSettings::draft2020_12().for_deserialize()`，不依赖未来可能变化的默认方言。参数 Schema 仍需在 Execution 入队前校验，Serde 不会替代上面的长度约束；执行时的业务检查继续存在。上例只示意参数声明，未作为编译 fixture 运行。[生成器设置](https://github.com/GREsau/schemars/blob/4791bb7352555d339e45658ca09c092ed9fa1f5d/schemars/src/generate.rs)

宿主保存的完整 JSON Schema 2020-12 是执行校验权威。各 provider 能接受的 Schema 子集及 strict 模式只影响模型侧投影，不能用投影替换宿主 validator；无法忠实表达的转换应明确拒绝或作为已测试的受限投影处理，不能静默丢弃约束。生成的 `$defs`、nullable、required 和未知字段行为都应进入三种 provider 的 fixture，同时确认入队前仍使用完整 Schema 校验。

`#[rig_tool]` 确实可从函数生成参数结构、Tool 实现和静态实例，自动排除 `&mut ToolContext`，非 Option 参数默认 required。它也存在需要明确接受的行为：显式 `required(...)` 会给未列参数加 Serde default；生成参数结构没有 `deny_unknown_fields`；函数不能带 `self`。对 Alia 需要依赖注入、参数约束和持有运行 owner 的业务工具，显式 DTO + Tool 实现更统一。不要同时再用 rmcp `#[tool]` 和手写 JSON 定义同一组参数。若将来选择函数宏，就把它作为这类工具唯一参数来源，并复用生成的 Schema，不能再写一份 DTO。[宏入口](https://github.com/0xPlaygrounds/rig/blob/2d1071e0e0c602f994e9d6cfd8cf5faea6f53650/crates/rig-derive/src/lib.rs)、[宏展开](https://github.com/0xPlaygrounds/rig/blob/2d1071e0e0c602f994e9d6cfd8cf5faea6f53650/crates/rig-derive/src/tool/expand.rs)

外部 MCP 工具没有本地 Args 类型，以服务器给出的 `inputSchema` 为唯一来源，登记时编译 validator，调用前验证原始 JSON。不能通过生成临时 Rust DTO 再解释一遍。选择 `jsonschema::options().offline()`，同时 `default-features=false`；前者可抵御工作区 feature 合并意外重新开启 HTTP/file retriever。只解析已登记的本地 `$defs`/明确内存资源；未知方言、外部 `$ref` 或编译失败时拒绝登记该工具。[validator 选项](https://github.com/Stranger6667/jsonschema/blob/d59ab79cb6eab73bb2d3caa6d1b4e538ca81567f/crates/jsonschema/src/options.rs)

Schema 的字节数、节点数、深度、字符串长度、工具总数都应有登记预算；参数也有独立体积预算。默认 fancy-regex 可回溯，外来 Schema 优先用 `PatternOptions::regex()` 并设置编译大小限制，遇到依赖 look-around/backreference 的 Schema 明确拒绝，不能悄悄忽略 `pattern`。若兼容这些表达式确有需要，再用有 backtrack limit 的 fancy-regex 分支验收。所有同步校验都不是可由异步 timeout 强制终止的任务；有界输入和有界并发仍必要。Schema 校验通过不等于工具已授权。

Rig 原始参数解码对某些 typed tool 会将 `null` 回退为 `{}`；Alia 必须在这种转换前以 object Schema 验证，并对进入账本和真正执行的同一 JSON 做规范化，避免摘要签的是 null、执行的却是空对象。与模型侧 Rig fork 配套，provider 的 `ToolFunction` 在归一化 null、空字符串或双编码 JSON 前必须保留原始形态供宿主判断，或者严格拒绝；如果到 Execution 时已经只剩 `{}`，事后 object Schema 无法恢复原输入，单加 validator 不能解决这个问题。Schema 升级后的输出结构变化即使不触发 schemars semver major，也可能改变工具身份，必须重新生成摘要。[解码行为](https://github.com/0xPlaygrounds/rig/blob/2d1071e0e0c602f994e9d6cfd8cf5faea6f53650/crates/rig-core/src/tool/contextual.rs)、[Schemars 版本规则](https://docs.rs/schemars/1.2.3/schemars/)

## 一个目录、一个派发入口

建议替换现有 `ToolExecutor` 和自定义通用 ToolDefinition/ToolResult，而不是长期再包一层同构 trait。`BuiltinToolRegistry` 可改为统一工具目录，内置、Rust 组件、MCP、Extism 都登记为相同的 entry：

| entry 内容 | 来源与用途 |
| --- | --- |
| Rig `ToolDefinition` + `ErasedHandler` | 模型可见定义和原始执行实现；静态 Tool 与 DynamicTool 统一到这里。 |
| 已编译 input validator；可选 output validator | 参数准入及远端结构化结果验收；同一 Schema 来源。 |
| Alia policy | source、用户/角色、确认要求、目标权限等宿主控制字段；不能从 MCP annotations 推导授权。 |
| immutable identity | 实现/资产、目标配置、目录代次、连接代次、授权上下文等；执行时 pin entry。 |
| 原连接/插件 owner 的强引用 | 维持已派发调用及其回收；liveness 只用于新调用准入。 |

DB 工具行仍是路由与检索投影，不是第二个可执行注册表。可继续存 routing hints/embedding。模型工具列表、潜在 MCP server 的 `tools/list`、UI 和 Execution 都从同一个目录快照投影。Rig agent 的 ToolServer 若被采用，应只存通往 Execution 的代理 handler；不能同时把原始 handler 暴露给 agent 自动运行。若宿主直接驱动 AgentRun 的 `CallTools`，则没有必要再维护 Rig ToolServer 注册表。

原始 handler 仅由 Execution worker 使用。执行顺序是：参数准入和来源检查 → pin entry → 写入/校验批准事实 → 当前政策及身份复验 → durable claim → 调用一次原 handler → 保存结果证据 → 给模型投影。登记/撤销与 claim 共用当前 catalog gate；旧 snapshot 不可自动升级成新批准。

Rig `ManagedToolSink` 的 generation token 是 `Arc<()>` 的进程内指针身份，适合防止旧 refresh 覆盖新注册；它不是持久化 revision。不能把该 token 当成现有 Execution `implementation_digest` 的替代。[ManagedToolSink](https://github.com/0xPlaygrounds/rig/blob/2d1071e0e0c602f994e9d6cfd8cf5faea6f53650/crates/rig-core/src/tool/managed.rs)

Rig 0.44 `ToolContext` 是 **可序列化的数据 map**，并非旧版的 live `ToolCallExtensions`。插入会序列化，读取会反序列化，scene 显式保存可能包含 inbound context；`insert_result` 的数据也可能写入 effect log。只传 Alia execution/request/invocation ID 及必要的非秘密来源数据；CancellationToken、credential broker、DB、原进程或连接 owner 保留在实例/Execution 中。不要把 token 作为 McpMeta 塞进 context。即使数据不发送模型，也不等于不会进入日志或持久化。[ToolContext](https://github.com/0xPlaygrounds/rig/blob/2d1071e0e0c602f994e9d6cfd8cf5faea6f53650/crates/rig-core/src/tool/context.rs)

## rig-rmcp：升级桥接与直接适配的实际差别

**rig-rmcp 0.44.0 发布包依赖 rmcp `2`，不是 3.5.1**，并且未关闭 rmcp 默认 features。并列引入两者会生成两个不相容的 Peer、Tool、CallToolResult 类型，也可能连带编译无须使用的 server/macros。允许大改不代表应把两版 SDK 都留下。[固定 workspace 依赖](https://github.com/0xPlaygrounds/rig/blob/2d1071e0e0c602f994e9d6cfd8cf5faea6f53650/Cargo.toml)、[发布包 normalized manifest](https://static.crates.io/crates/rig-rmcp/rig-rmcp-0.44.0.crate)

| 方面 | 升级 rig-rmcp 到 rmcp 3.5.1 | 直接 rmcp + Alia adapter |
| --- | --- | --- |
| 现成价值 | 复用 McpTool、结果富内容映射、ManagedToolSink 接口、refresh 顺序防回滚及 liveness。 | 复用官方 rmcp 协议/transport，Rig DynamicTool 和标准结果类型仍照用。 |
| 必改代码 | Cargo major；`native.rs` 的 CallTool 返回必须区分 complete/input_required/task；新版每请求 metadata；`handler.rs` 增加 discover/协商和 subscriptions，不只旧通知回调。 | 一个 McpConnection owner、工具发现到统一 entry 的转换、一次请求派发与结果投影。没有另一层 bridge registry。 |
| 目录身份 | 原 handler 只将 DynamicTool 交给 sink，server 原始 outputSchema/annotations/meta 不全进入标准定义；需要增加源元数据/部署身份入口。旧 list_changed 刷新失败只记日志，保留旧目录，需要更改失效策略。 | 在完整 rmcp::Tool 上计算摘要、编译 validator后原子提交；无需从 DynamicTool 反推原始信息。 |
| 超时/取消 | 现有 300 秒默认 timeout、1 秒 detached cancellation 丢弃原 handle；要新增由宿主持有的 dispatch receipt/owner 接口，不能只把 Timeout 变成通用失败。 | 原 handle 留在 Execution 管理的任务中，结果分类直接跟账本对齐。 |
| 结果 | 可复用已有保序 rich content 转换和原始响应保留；需另加 Alia 预算、来源与敏感字段筛选。 | 做一次相同映射，原始响应作为证据，模型输出由策略筛选；可将纯转换贡献给上游。 |
| 维护后果 | 合理路径是上游合入 3.x 与宿主生命周期扩展，再使用小 fork；若私有 fork 改动持续深入 native/handler，节省有限。 | 有少量业务适配代码，但通用 JSON-RPC、协议版本、消息类型及 transport 状态机仍由 SDK 维护。 |

因此推荐直接适配；若上游桥接补齐新版协议和可插拔原执行 owner，再用同一组回归重新比较。不能仅把 Cargo 的 `2` 改为 `3` 编译通过就算完成。参考桥接 [调用与取消实现](https://github.com/0xPlaygrounds/rig/blob/2d1071e0e0c602f994e9d6cfd8cf5faea6f53650/crates/rig-rmcp/src/native.rs)、[发现与刷新](https://github.com/0xPlaygrounds/rig/blob/2d1071e0e0c602f994e9d6cfd8cf5faea6f53650/crates/rig-rmcp/src/handler.rs)。

## MCP 的角色和协议范围

当前需求是 **Alia 作为 MCP client/host 使用外部工具**，先不启用 server。MCP server 只有在明确需要其他应用调用 Alia 工具时才增加：`tools/list` 从同一目录投影，`tools/call` 使用调用者身份进入同一 Execution 服务。MCP transport 身份必须映射到 Alia principal，不能凭远端 `_meta.user_id` 赋权；不能公开原 handler 的直接调用入口。

rmcp 3.5.1 的发布源码确实已经包含 2026-07-28 的 `server/discover`、transport-neutral `subscriptions/listen`、MRTR 和 Tasks，不是仅 main 文档的未来能力。`ClientLifecycleMode` 有 Initialize、Discover、Auto；Auto 默认探测超时为 10 秒。最低接入建议：明确支持 `[2026-07-28]`，配置 legacy fallback 为 `2025-11-25`；老服务可显式走 Initialize，禁止无界遍历未知版本。握手和目录获取仍受宿主总 deadline 约束，并记录实际协商版本。[发布版 client](https://github.com/modelcontextprotocol/rust-sdk/blob/79437f291b2c44053d00dcd5db969fd0cca7c887/crates/rmcp/src/service/client.rs)

第一版 client capabilities 使用空默认集合：不声明 sampling、roots、elicitation、experimental 或 tasks 扩展。调用 tools/list、tools/call 不要求客户端声明一个不存在的 tools capability；应检查服务器 tools 能力。2026 目录变更用显式 `SubscriptionFilter::tools_list_changed` 监听；旧版才用 ClientHandler 的通知回调。新订阅里的通知不会再同时投递 callback，不能两个路径重复刷新。[capabilities](https://github.com/modelcontextprotocol/rust-sdk/blob/79437f291b2c44053d00dcd5db969fd0cca7c887/crates/rmcp/src/model/capabilities.rs)、[订阅入口](https://github.com/modelcontextprotocol/rust-sdk/blob/79437f291b2c44053d00dcd5db969fd0cca7c887/crates/rmcp/src/service/client.rs)

2026-07-28 HTTP 已移除协议 session 和旧 GET 通知流，长通知来自 `subscriptions/listen` 的响应流，也不支持 Last-Event-ID 恢复。兼容旧版时才使用旧 session/resume 语义。不能把“HTTP socket 重连”“协议 session 重建”“Alia connection epoch”当成同一件事。[固定版本 transport 规范](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http)

## 连接、重连和读取预算

每个已配置服务和授权 principal 持有一个连接 owner，负责 RunningService、订阅任务、目录刷新、活动请求、关闭和故障报告。新连接/进程重启分配新 epoch；旧 owner 被撤出新调用目录后，已派发请求仍归旧 owner。不要因 UI/调用方 future 被 drop 就把 owner 丢掉。

### stdio

`TokioChildProcess` 提供基本关闭、等待和 kill；默认 stderr 继承，Drop 会 spawn 异步清理。它不是 Alia 需要的可核验进程树回收和预算证明。第一版可采用已有进程 owner 的模式持有 child/进程组/stdin/stdout/stderr，把有界 framing 后的消息流交给 rmcp；命令固定为管理者配置的绝对路径及参数，env_clear 后只注入需要的环境，不自动执行 npx/uvx 下载，也不解析模型提供的 shell 字符串。[child transport](https://github.com/modelcontextprotocol/rust-sdk/blob/79437f291b2c44053d00dcd5db969fd0cca7c887/crates/rmcp/src/transport/child_process.rs)

3.5.1 的 `AsyncRwTransport::receive` 使用 `read_until` 追加到 Vec，没有行长度上限。虽有 `JsonRpcMessageCodec::new_with_max_length`，默认 transport 并未用它限制读取。应直接用这个 codec 的 bounded FramedRead/FramedWrite 和 SDK Sink/Stream transport 接口，或向上游补可配置上限；出站编码也需独立大小检查，因为 decoder 上限不等于 encoder 上限。stderr 要有界 drain，不进入 JSON-RPC stdout。[framing](https://github.com/modelcontextprotocol/rust-sdk/blob/79437f291b2c44053d00dcd5db969fd0cca7c887/crates/rmcp/src/transport/async_rw.rs)

关闭必须等待原 child reap、进程组处理与 pipe drain 的最终证据；超时可先返回“正在停止”，由原任务继续回收。`RunningService::close_with_timeout` 会在 timeout 后失去内部 JoinHandle 的可等待所有权，不应作为“已清理”证据。可让独立 owner 任务执行 `close()`，外部 timeout 等待 `&mut JoinHandle`，超时保留它。[service shutdown](https://github.com/modelcontextprotocol/rust-sdk/blob/79437f291b2c44053d00dcd5db969fd0cca7c887/crates/rmcp/src/service.rs)

### Streamable HTTP

rmcp 3.5.1 HTTP backend 依赖 **reqwest ^0.13.2**，与 Rig 0.44 使用相同 major；整体迁移已选择将工作区统一到 reqwest 0.13，并验证当前 0.12 的其余 HTTP 调用方。MCP-only client 可以直接实现 `StreamableHttpClient` 并传给 `StreamableHttpClientTransport::with_client`，不必把自有 HTTP 政策交给 SDK 默认 client。[features/dependencies](https://github.com/modelcontextprotocol/rust-sdk/blob/79437f291b2c44053d00dcd5db969fd0cca7c887/crates/rmcp/Cargo.toml)

仅传一个设置了 timeout 的 reqwest::Client 不足以限制读取：SDK 内置实现对 SSE 有单事件预算（默认 16 MiB），普通 JSON 用 `response.bytes()`，非成功错误体用 `response.text()` 整体读取。建议在 `StreamableHttpClient` 层加有界 JSON/error/SSE 读取，或者上游补齐相同配置；不自写协议状态机。认证 metadata/token discovery 的 `OAuthHttpClient` 是另一个接口，也须遵守同一目标与日志政策。[HTTP backend](https://github.com/modelcontextprotocol/rust-sdk/blob/79437f291b2c44053d00dcd5db969fd0cca7c887/crates/rmcp/src/transport/common/reqwest/streamable_http_client.rs)

明确设置以下策略，而不是照默认：

- `reinit_on_expired_session(false)`：旧协议默认 true 会在 404 后重新 initialize 并把失败的普通 POST 重发一次；副作用 tools/call 不由 transport 自动重发。
- SSE retry 配置有限次数与最大间隔；默认指数回退两者都无限。恢复旧响应流也不是重新发 tools/call 的许可。2026 订阅断开后新建订阅并重读目录，期间目录不可用。
- 限制并发、队列、总 deadline、idle deadline、每消息/每事件/累计响应字节、日志/进度速率；进度不能刷新总 deadline。
- 默认不跟随 endpoint redirect；目标地址、代理与 OAuth discovery 地址都受配置的服务网络政策约束。MCP 连接是显式授权服务，可以包含批准的内网目标；无需照搬面向任意公网 URL 的 fetch 策略。
- 认证上下文改变时停用旧目录和旧缓存，再发现；不在同一个连接上混用不同用户的 token。

以上默认依据 [HTTP transport](https://github.com/modelcontextprotocol/rust-sdk/blob/79437f291b2c44053d00dcd5db969fd0cca7c887/crates/rmcp/src/transport/streamable_http_client.rs)、[SSE retry](https://github.com/modelcontextprotocol/rust-sdk/blob/79437f291b2c44053d00dcd5db969fd0cca7c887/crates/rmcp/src/transport/common/client_side_sse.rs)。

## 目录变更、批准失效与服务身份

命名使用配置生成的稳定 service_id + remote_tool_name，并映射为 provider 接受的确定性名称；不能使用 serverInfo.name 作为唯一身份，远端可重名。远端描述、inputSchema、outputSchema、annotations、影响行为的 meta 都进入经规范化的目录修订。工具描述变化也要使待批准计划失效：描述虽不是可执行代码，会改变用户/模型对操作的理解。现有 `implementation_digest` 仅有 parameters 等字段，没有 description；接入时必须补齐，不能只调用旧摘要函数。

推荐工具身份包含：

| 字段 | 意义 |
| --- | --- |
| service_id、transport kind | Alia 管理的稳定服务身份；远端 self-report 仅是证据。 |
| endpoint/config digest | HTTP canonical resource URI、允许的 issuer/目标政策；或 stdio 受控可执行文件/包资产摘要、args、cwd、非秘密环境配置。 |
| authorization principal + policy epoch | 账号/租户/权限集合的稳定身份，排除原始 token。只轮换 token 且权限不变可保留逻辑身份；权限变化必须更新 epoch。 |
| connection epoch、catalog revision、tool content digest | 原调用 pin 的实例及工具定义。remove→re-add、重启和重新授权不能恢复旧批准。 |
| adapter/build revision | Alia 自身实现变化仍绑定原 build 事实。 |

HTTP TLS 和 OAuth 能认证目标/授权范围，不能证明服务器正在运行哪份实现。标准 MCP 没有“远端可执行代码 SHA256”保证；serverInfo.version 或 Schema hash 都不是代码证明。服务未通知的行为变化仍是远端信任边界。严格需要代码固定时，只接宿主管理的 pinned stdio 资产，或另有可信部署/签名修订契约，不能把普通远程 MCP 描述成相同保证。

收到 list_changed 时先将该来源标为 dirty，禁止新 claim；刷新所有分页、检查 cursor 循环/页数/工具数/总字节，再在目录锁下原子发布新 snapshot。刷新失败或订阅断开保留旧快照只供查看，不继续执行；旧 refresh 不得覆盖新代次。同样在重新连接和授权变化后完整刷新。

rmcp client response cache 默认开启且 `serve_stale_on_error=true`，可能在目录请求失败时返回旧成功结果。初期用 `ClientCacheConfig::disabled()`，由统一目录管理快照；若后续启缓存，必须关闭 stale-on-error、按 principal 分区并由通知/授权 epoch 失效。[cache](https://github.com/modelcontextprotocol/rust-sdk/blob/79437f291b2c44053d00dcd5db969fd0cca7c887/crates/rmcp/src/service/client/cache.rs)

## 认证、取消与 unknown

HTTP token 由 Alia credential store/ broker 注入 Authorization；不进入模型参数、ToolContext、普通配置 JSON、工具描述或 result metadata。store 按 principal + service/resource URI + issuer 隔离；refresh token 加密保存，轮换串行化，更新持久化后才发布新凭据。rmcp 有 CredentialStore 和 refresh guard 接口，默认 store 只在内存，不承担 Alia 的安全持久化政策。[auth interfaces](https://github.com/modelcontextprotocol/rust-sdk/blob/79437f291b2c44053d00dcd5db969fd0cca7c887/crates/rmcp/src/transport/auth.rs)

采用官方 OAuth 流程时，核对 issuer、PKCE/state、resource/audience、回调与权限升级；要求增加 scopes 是新的授权输入，不是工具可以自行提升权限。stdio 只给需要的环境变量。`serverInfo`、tool annotations、`_meta` 以及 MCP session ID 不是授权主体。官方 HTTP 授权规范也明确要求每请求 Authorization、正确资源 audience，并不适用于 stdio 的凭据传递方式。[2026 授权规范](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization)

rmcp `AuthClient` 的通用 challenge 路径会在 401 后尝试 refresh 并重发一次原消息。对已 claim 的 tools/call，不直接使用这条透明路径：派发前确保凭据，401/403 后记录响应证据，由明确的未执行契约决定能否恢复；默认不自动重发。纯 discovery/read 的安全重试可留给连接 owner。允许 endpoint 声明 idempotent_hint 也不能自动获得重试权。[AuthClient 重试](https://github.com/modelcontextprotocol/rust-sdk/blob/79437f291b2c44053d00dcd5db969fd0cca7c887/crates/rmcp/src/transport/common/auth/streamable_http_client.rs)

Execution ID 是本地副作用身份，JSON-RPC request ID 只是关联身份；远端 task ID 是服务创建的工作句柄。三者分开存，附 service/connection/catalog/authorization identity。远端没有标准通用 idempotency key，不能把相同 JSON-RPC ID 的重发理解为去重。

原 owner 先保存 dispatched 事实，再调用 `send_cancellable_request`，拿到 handle 后记录 request ID。提交期间超时、连接断开、无法确认是否写入都保守归 unknown；只有确定在外部派发前失败，才能认定无副作用。取消等待者与取消远端请求分开：前者立即停止给该会话交付，后者按协议发送取消/关闭响应流，但不写“已撤销副作用”。[RequestHandle](https://github.com/modelcontextprotocol/rust-sdk/blob/79437f291b2c44053d00dcd5db969fd0cca7c887/crates/rmcp/src/service.rs)

2026 HTTP 取消通常通过关闭该请求 SSE 流，stdio 用 cancelled 通知；旧协议由 SDK 兼容。标准允许服务器已完成或无法取消，不能据取消动作证明未执行。按协议已取消请求的迟到响应不再用于完成原调用；可作为有限审计信息，不能重新派发或把其他连接的同 ID 响应套进来。未发送协议取消但仅 UI 离开时，原 owner 仍可接收原有效响应并落账。[取消规范](https://modelcontextprotocol.io/specification/2026-07-28/basic/patterns/cancellation)

`isError=true` 表示远端报告执行错误，不保证操作没有部分成功。现有 Failed 表示有未执行/拒绝证据时，不能机械映射为 Failed；保留 Unknown 及原响应证据，或显式新增“已完成但报错、可能有部分效果”的状态。Schema不符/输出超预算发生在远端执行之后时也一样，不能用 invalid_result 自动重试。`isError=false` 只能记录为该可信服务报告成功的 receipt。

MRTR/Tasks 先不协商。若遇到未协商的 input_required/task，保存协议异常和远端标识，不把它当完成结果。未来需要时：一次逻辑 Execution 管理整个 MRTR round，继续请求绑定原 arguments/requestState/身份且轮数有界，额外输入经过 Alia 策略；tasks/get 只轮询同一服务/主体的 task_id，返回未知/TTL失效不重发原 tools/call，tasks/cancel ack 不等于任务终止。SDK 提供 `call_tool_once` 区分 Complete/InputRequired/Task，`cancel_task` 文档也明确 cooperative。[MRTR](https://github.com/modelcontextprotocol/rust-sdk/blob/79437f291b2c44053d00dcd5db969fd0cca7c887/crates/rmcp/src/model/mrtr.rs)、[Task 与 client API](https://github.com/modelcontextprotocol/rust-sdk/blob/79437f291b2c44053d00dcd5db969fd0cca7c887/crates/rmcp/src/service/client.rs)

## 结果内容与来源

rmcp 3.5.1 保留 ordered ContentBlock、structuredContent、isError 和响应 meta；structuredContent 可以是任意 JSON，显式 null 与不存在不同。Rig ToolOutput 也支持多模态内容。应删除旧“统一拼成字符串”的习惯，保存原证据与给模型的呈现两个视图，但只保留一份可执行结果事实。[rmcp 结果类型](https://github.com/modelcontextprotocol/rust-sdk/blob/79437f291b2c44053d00dcd5db969fd0cca7c887/crates/rmcp/src/model.rs)、[Rig 结果输出](https://github.com/0xPlaygrounds/rig/blob/2d1071e0e0c602f994e9d6cfd8cf5faea6f53650/crates/rig-core/src/tool/output.rs)

| 内容 | Alia 处理 |
| --- | --- |
| text | 保留顺序与来源，进入 tool result 角色；绝不提升为 system/developer 指令。 |
| structuredContent | 按已固定 outputSchema 校验；保留 JSON 类型；不要只取首个 text，也不要把相同 fallback JSON 重复喂给模型。 |
| image/audio | 限制 encoded/decoded 字节、MIME、block 数与合计；能力匹配后才给模型，二进制存受控 artifact，账本存引用/摘要。 |
| embedded resource | 保留 URI、MIME、文本或 blob 及来源；URI 不是读取宿主文件的授权。 |
| resource_link/icons | 作为描述/链接，不自动打开、下载或调用 tools；后续读取是新的受控请求。 |
| annotations / `_meta` / progress / logging | 远端不可信元数据，按允许字段保留；不能让其设置用户身份、隐私策略、信任等级或授权。 |

工具描述、服务 instructions、prompts、resource 和执行输出都是潜在提示注入来源。server instructions 不自动拼进系统提示；第一版不请求 prompts，不启 server-initiated sampling/roots/elicitation。每个可见 block 关联 service_id、remote tool、目录摘要、execution_id 和原输入 lineage；后续 Context 扩展和隐私复验继续适用，来源标记不能被远端 annotations 覆盖。Schema 验证只检查形状，不是文本安全过滤器。官方也明确 ToolAnnotations 是不可信 hints。[工具规范](https://modelcontextprotocol.io/specification/2026-07-28/server/tools)

## 最小依赖与 features

以下是建议清单，未写入项目 Cargo。精确锁定用于初次验收，后续升级走相同 fixtures：

```toml
rig-core = { version = "=0.44.0", default-features = false }
schemars = { version = "=1.2.3", default-features = false, features = ["std", "derive"] }
jsonschema = { version = "=0.59.0", default-features = false }
rmcp = { version = "=3.5.1", default-features = false, features = ["client", "transport-async-rw"] }
```

静态 Tool 手工实现而 DTO 自动生成 Schema，因此不启 rig-core `derive`；若选择 `rig_tool` 才启它。不引 rig-rmcp，不启 rmcp `server`/`macros`/`schemars`/`elicitation`。stdio 进程由宿主管理并使用有界 framing，不必启 `transport-child-process`；若复用它则仍须补足 owner 和帧预算。既有 Tokio/futures/tokio-util/Serde 直接复用，不新建 runtime。

HTTP 按部署需求加 rmcp `transport-streamable-http-client`；自定义 `StreamableHttpClient` 用工作区 reqwest 0.13 的 rustls/json/stream。只有决定使用 SDK 自带 backend 才开 `transport-streamable-http-client-reqwest`，但仍需解决 JSON/error body 上限。OAuth 需要时加 `auth` 和 `reqwest`（后者选 rustls）；不默认启 enterprise-managed、JWT、server。未来对外提供 MCP server 时再单独添加 feature 和入口，不能因为依赖默认值顺带公开网络服务。

Rustls 是 Rust TLS 路线；这不表示加密 provider 或所有传递依赖均无本地编译要求。跨平台构建与具体 crypto provider 由整体依赖验收确认。

## 改动范围与验收

| 现有/拟新增位置 | 应做变化 |
| --- | --- |
| `providers/adapter.rs` 的 ToolExecutor/ToolDefinition/ToolResult | 通用部分由 Rig 类型替换；Alia 身份/政策/来源成为明确附属数据，不保留同构双 trait。 |
| `tools/builtin.rs`、`tools/sensor.rs` | 按业务拆分；每个参数 DTO 定义一次；删除手写 Schema 与散落 `required_str/as_*` 通用解析。 |
| `modules/execution/{registry,service,worker,types,repository}.rs` | pin Rig handler；摘要包含完整目录/服务身份；保存远端证据与原 owner；继续只有一次派发及持久化结果重试。 |
| `modules/interaction/tools.rs` | 使用相同工具目录投影与标准 ToolOutput，删除重复结果规范化；保留业务路由、来源和 memory 扩展。 |
| 拟新增 `tools/mcp/{connection,catalog,executor,transport}.rs` | 连接owner、原子目录、执行证据与有界transport；不重写MCP消息协议。文件分拆可按落地规模缩减。 |
| 配置/Secret store/管理 API | 显式服务配置、principal关联、凭据引用、启停与目录修订可见性。不得模型动态添加任意启动命令或目标。 |

至少运行这些 fixtures，成功才称可采用：

1. DTO 的 required/default/nullable/deny_unknown_fields 与 Schema 一致；范围约束在执行前生效；外部 HTTP/file `$ref` 没有网络/文件读取，复杂正则与大 Schema 明确失败。
2. 同名跨服务不冲突；list_changed 与并行 refresh 不回滚；dirty期间不可claim；remove/re-add、description/outputSchema变化、重连和授权切换使旧批准失效。
3. stdio 只写不换行的超大帧、畸形 UTF-8、stdout/stderr 洪泛、子进程及后代不退出都能有界处理；停止后原资源回收证据完整。
4. HTTP JSON/error/SSE 超量、进度洪泛、慢响应、404/401重试边界；断流后不重复 tools/call；2026没有旧GET/session假设，旧协议兼容路径独立验收。
5. 发送前取消不执行；发送中断/执行后断连为unknown；UI离开原owner继续收尾；取消ack不冒充副作用撤销；晚结果不能误认到新连接。
6. text/image/audio/resource/structured null保持顺序与类型；outputSchema错误不会重发；结果不丢来源，描述/正文中的指令不能提高权限。
7. 不声明的sampling/roots/elicitation/tasks不被执行；未来任务轮询只能复用已保存task_id，MRTR的后续round不得重新获取无关授权。
8. 全路径只有一个登记目录和Execution入口：Rig默认自动工具调用、未来MCP server、插件和管理API均不能绕过。旧通用代码迁移完成即删除。

本轮没有编译这些候选或进行性能、OAuth真登录、真实MCP服务联调。源码检查已证明上述API及默认策略差异，尚不能证明集成正确性；MCP对远端服务行为的信任和取消后的unknown仍是必须保留的边界。
