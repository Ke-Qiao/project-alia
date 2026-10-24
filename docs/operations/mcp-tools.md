# MCP 工具服务

Alia 使用官方 `rmcp 3.5.1` 作为 MCP **客户端**。MCP、内建 Rust `Tool` 和 WASM 工具进入同一个 `ToolEntry`／Execution 目录；工具描述、Schema、远端 annotations 都不能自行批准业务效果。Alia 不启动 MCP server，不开放 sampling、elicitation、roots、prompts 或资源读取能力。

配置样例在 [`config/server.example.d/plugins/mcp.toml`](../../config/server.example.d/plugins/mcp.toml)。样例全部注释，默认没有外部进程或网络连接。取消需要的配置块注释、设置实际账号 UUID、地址和凭据后启动服务；`enabled = true` 表示随 Server 启动连接。`enabled = false` 的配置仍可由当前管理员显式启动。最多配置 16 个服务。

`user_id` 是服务凭据所属的 Alia 账号，必须为非空 UUID。只有该用户可看到和提出调用该服务的工具；工作线程派发前再次检查主体。`require_confirmation` 默认为 `true`，可设置 `required_role`；远端的只读、幂等或无需确认声明不降低宿主政策。工具名使用 `mcp_<service-prefix>_<digest>`，保留远端名和完整描述在宿主身份中，避免与内建工具撞名。

HTTP 使用配置中的固定 URL。`token_env` 指定 Server 启动环境中的 bearer token 变量名，不接受模型、API 请求或远端返回的新端点／凭据。HTTP 禁用环境代理、跳转、reqwest 自动重试和 rmcp SSE 重试、失效会话自动重建；不会把凭据跟随跳转发送。HTTP 服务地址由管理员信任，允许内网部署；这不是任意 URL fetch，也不声明 SSRF 隔离。OAuth 交互授权和动态凭据更新暂不提供，修改配置或 token 后须显式重启服务。

stdio 必须显式设置 `allow_host_execution = true`，授予该进程宿主账号的代码执行权限。`program` 必须是绝对路径的普通文件，`executable_sha256` 为其小写 SHA-256。宿主有界读取后验证摘要，并在仅当前用户可访问的临时目录中执行同一份只读快照，避免验证和启动之间源文件替换。程序必须能从此位置运行；依赖可执行文件、解释器、动态库、参数文件和网络环境仍由管理员信任，**单个可执行文件摘要不是完整部署资产 pin，进程不是沙箱**。需要隔离的不可信代码请使用 WASM 插件。stdin/stdout 仅用于 MCP，stderr 不保留也不写日志；环境先清空，再注入 `env` 中的明确配置。父进程和同进程组后代的终止、原 child 的 wait、stderr drain 都由原 owner 清理。

默认优先协商 MCP `2026-07-28` discovery 生命周期，允许回退至 `2025-11-25` initialize；只支持 initialize 的旧服务可显式设置 `legacy = true`。现代服务订阅工具目录变化，旧服务处理 `notifications/tools/list_changed`。每次目录变化立即关闭旧 entry 的准入，重新读取全部分页后发布新目录。Schema、描述、目录修订、服务配置、协议、用户、连接 epoch 都参与 Execution 身份；同名重启、目录变化或服务更新不会继承旧批准。响应缓存关闭。长任务／MRTR 返回按“结果未知”保留，不根据远端 task ID 再次 dispatch。

管理员通过现有 bearer 身份访问：

- `GET /api/v1/tools/mcp/services`：读取状态、连接 epoch、工具数和有界错误。
- `POST /api/v1/tools/mcp/services/{id}/restart`：清理原连接后建立新连接，返回 `connecting`。后续查询状态直到 `ready` 或 `failed`。
- `POST /api/v1/tools/mcp/services/{id}/stop`：关闭准入并清理原 owner。若清理超过 3 秒，返回错误并保留 `stopping` owner；查询状态后再处理。

管理写操作会重查当前管理员账号，并在操作期间持有账号事务读锁；HTTP 调用方断开不会丢失原管理任务。Server shutdown 永久关闭该 manager 的启动准入。连接错误或调用超时关闭该连接，不自动重放任何业务调用；管理员 `restart` 是重新连接入口，不能证明之前的未知效果没有发生。

首次 `POST /api/v1/tools/execute` 回包丢失时，可用原 `request_id` 调用 `GET /api/v1/effects?request_id=<UUID>`。此查询沿用当前活跃账号校验，仅返回该账号的原账本，工具停止、移除或重连不影响读取。前端重试已有请求时先自动分页查询，找到 invocation `0` 且工具名、JSON 参数一致的记录就展示原状态；查询失败或内容冲突时不提交，确实没有该 invocation 才以原 UUID 重试 POST。读取不会重新批准或派发；确认仍重查目录政策，派发前仍重查实现版本与执行政策。

每服务最多 4 个活动调用，协议单消息和普通 HTTP/SSE 响应最多 1 MiB，普通 SSE 响应最多 256 个事件。现代订阅及旧协议 GET 通知流允许长期空闲；每个 60 秒窗口最多 1 MiB／256 个事件，单个未完成 SSE frame 始终最多 1 MiB，不随窗口重置。HTTP/SSE 在解析前限制字节，含注释和未完成行；不以整条订阅的累计流量或寿命切断健康连接。响应头最长 30 秒，普通 JSON body 最长 30 秒、普通 SSE 响应最长 60 秒，工具调用仍受更短的调用截止控制。目录最多 128 工具、16 页、1 MiB，单 Schema 128 KiB；仅接受 JSON Schema 2020-12、深度 32，以及无环的根内 JSON Pointer `$ref`；不支持 `$id`、anchor、dynamic／recursive 引用。引用展开最多 2048 节点、64 层，验证前再限制展开节点数乘实例编码字节数不超过 4 MiB，防止小 Schema 的引用组合放大。保留接受的完整约束，拒绝超限结构；这是一项保守准入预算，不是库提供的硬 CPU 截止。目录刷新最长 10 秒、握手 20 秒、工具发送和响应各 30 秒，取消尝试最长 2 秒。进度通知不能延长总响应期限。超过限额不会静默截断为成功结果。

Execution 持久回执的原有上限仍为 **64 KiB**。MCP 回执保存原结果、服务／连接／协议请求身份和类型化展示内容；因此可持久化的实际正文小于 64 KiB。更大的响应、图片或重复富内容不会变成成功，而是记录 unknown；本轮没有新增大附件 artifact 存储。text、image、resource link、embedded resource、audio 等按原类型保留；不自动打开 URI、图标或嵌入资源。模型看到的内容始终是工具结果，不提升为系统消息。原始 JSON 不能伪造宿主的类型化 presentation；只有匹配当前调用、已成功落账的规范回执才向模型展开富内容。

本地测试使用回环 HTTP fixture 和受控 stdio 子进程，覆盖握手、完整 Schema 拒绝、主体隔离、目录失效、重启、停止中未知结果、禁止重发、响应预算、跳转拒绝和进程清理。它们不表示已验证某个生产 MCP 服务或真实模型效果。
