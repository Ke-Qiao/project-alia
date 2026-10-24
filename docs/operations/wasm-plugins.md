# WASM 插件部署与撤权

本页描述 2026-10-18 接入的 Rust host / Rust PDK 实现。早期第三方评估文档记录选型依据；当前接口以本页和源码为准。真实模型、Pi ARM64 实机容量及设备原生驱动验证仍需单独进行。

## 构建与后端边界

使用 Rust 1.96。宿主固定 Extism 1.41.0，关闭默认 features；当前 lock 使用 Wasmtime 49.0.3。Rust guest 固定 `extism-pdk = 1.4.1` 且关闭默认 features，编译为 `wasm32-unknown-unknown`。宿主不接受 Python/Bun 命令、第三方 Wasmtime 序列化产物或 Extism 的 URL/文件 manifest。包只提交原始 WASM 与 Alia 自己的严格 manifest。

```sh
cd examples/wasm-plugins
cargo +1.96.0 build --release --locked
```

该目录的 `.cargo/config.toml` 声明 16 MiB 最大线性内存。产物是 `target/wasm32-unknown-unknown/release/alia_wasm_demo.wasm`。同一产物提供 `alia_tool`、`alia_expression` 和 `alia_processor`；共享 DTO 位于 `crates/alia-plugin-api`。不要从其他 cwd 仅使用 `--manifest-path` 构建，因为 Cargo 的配置目录发现规则会遗漏这个 linker 限额。

这里的“纯 Rust”限定为本轮新增的模型/工具/MCP/插件宿主与 Rust PDK 示例，不新增 Python 或 Bun 运行服务。整个 Alia 仍依赖平台库和原生组件：Embedded 默认启用 sherpa-onnx，CPAL 使用系统音频后端，Linux 音频路径还依赖 ALSA；WASM 宿主的 Wasmtime JIT 也运行于本机。不能据此宣称零 native 依赖或已完成 Pi ARM64 实机验证。

运行器拒绝组件、WASI/custom host imports、HTTP imports、start section、GC、threads、memory64、多内存，以及没有声明上限的内存/表。没有网络、文件、环境、数据库、工具注册或设备 host function。插件计算得到的工具结果仍通过统一 Execution 账本；插件不能提交自己的“已批准”状态。

## 从旧插件配置迁移

旧表达 JSON-lines 子进程加载器及 Python 示例已经删除，没有自动转换或兼容执行路径。升级 Server 前移除 `[resources]` 中旧的 `external_expression_plugins_dir` 配置及其旧目录别名；该配置节拒绝未知字段，保留旧键会导致配置解析失败。将插件改为 Rust PDK WASM，在新 `plugins.wasm_dir` 安装包，并分别复核执行 grant 与私有 Appraisal pin。包 ABI 为 1，表达域 ABI 为 3；旧表达 ABI 1/2、`command`、`args`、环境变量或子进程限额字段不接受。

Embedded 旧 ZIP 的 manifest 也需要更新：明确选择 `runtime = "wasm"`、`type = "processor"` 与 `entry = "module.wasm"`，删除旧 `[plugin.resources]`/CPU 百分比等未经执行的限额，改用 `[plugin.wasm_limits]`。保留的设备原生驱动必须显式声明 `trusted_native` 并取得额外启动授权；不能将旧 processor 原样标成 native 来迁移。新客户端还须传入安装返回的 `expected_asset_sha256`。旧实例先停止并完成清理，再卸载、安装新版和重新批准；旧批准不自动继承。

## Server 安装、启用和升级

配置 `plugins.wasm_dir = "wasm_packages"`，相对路径按 Server 配置目录解析。默认不启用动态包目录。管理员 API 位于 `/api/v1/plugins/wasm`，请求使用现有 bearer token，服务端重新核对数据库中的当前管理员身份。上传 JSON 的总 body 上限 24 MiB，WASM 原始字节上限 16 MiB，manifest 上限 128 KiB。管理请求的后台 owner 保留到操作完成，HTTP 断开不取消写入。

| 请求 | JSON 内容 | 结果 |
| --- | --- | --- |
| `POST /plugins/wasm` | `{"manifest":{...},"module_base64":"..."}` | 安装为未启用；返回宿主算出的 `implementation_pin` |
| `GET /plugins/wasm` | 无 | 列出 manifest、pin 和 enabled |
| `POST /plugins/wasm/{name}/enable` | `{"expected_pin":"当前 pin"}` | 授予该版本执行权并注册工具/表达入口 |
| `POST /plugins/wasm/{name}/revoke` | 同上 | 关闭准入、取消并等待原 owner，移除目录入口 |
| `PUT /plugins/wasm/{name}` | `manifest`、`module_base64`、`expected_pin` | 撤销旧版本，原子替换完整资产；新版本未启用 |
| `DELETE /plugins/wasm/{name}` | `{"expected_pin":"当前 pin"}` | 清理执行后删除安装 |

示例完整 manifest 见 `examples/wasm-plugins/package.json`。包名为 1–48 个 ASCII 字母、数字或下划线。安装/激活前检查声明的导出确实是 Extism 可调用函数：无参数、无返回值或返回一个 `i32`。工具使用其 `export` 字段，表达和 processor 分别要求 `alia_expression`、`alia_processor`；缺失或签名错误时不发布入口。静态工具描述包含 `name`、`description`、`export`、`parameters`、`require_confirmation`、`required_role`；`required_role` 可为 null。工具在目录中的名字通常是 `wasm_{包名}_{工具名}`；超过 64 字节时保留前 47 字节并附加下划线和 16 位 SHA-256 后缀，以实际工具目录返回的名称为准。Schema、角色、确认策略、manifest 和实际运行器 pin 均参与 Execution 实现身份。

pin 是 SHA-256，覆盖模块字节、完整部署声明、资源限额、运行器 ABI 标识和当前宿主可执行文件内容。升级、改变资源配置/描述符或更换宿主二进制都会失效。持久 grant 仅在重启后的 pin 完全相同时恢复。此身份绑定不是发布者签名或操作系统完整性证明；未审核本地导入不自动获得已验证发布者身份。

首次启用及撤权后重新启用会创建新的 Deployment；对仍启用的同一 pin 重复启用是幂等操作。旧 Arc 引用在撤权后永久关闭，不能在后续启用时重新执行。资产使用包含 manifest 和 WASM 的单文件 envelope 原子发布，升级不会暴露新旧文件混合的一代。文件系统持久化错误会返回未确认结果；先检查状态并重试恢复，不把失败响应解释为重启后仍然有效的撤权证明。

## 私有 Appraisal 的额外批准

普通包启用不授予私有用户输入。表达包声明：

```json
{"expression":{"abi_version":3,"capabilities":{"appraisal_engine":true}}}
```

管理员先从包目录取得实际 `implementation_pin`，再通过现有 Settings API 更新 `core.policies.appraisal.approved_interpreters` 中该包名对应的 pin。使用当前 Settings GET 返回的 ETag 作为写请求 `If-Match`，并保留其他已有批准项。例如 PATCH 内容为：

```json
{"core.policies.appraisal.approved_interpreters":{"rust_demo":"实际64位pin"}}
```

Settings 的实际写方法为 `PATCH /api/v1/settings`。持久 Context/Appraisal policy 负责版本化，解释前后继续复验实现 pin；未批准、撤权或版本变化均拒绝私有调用。升级后不能沿用旧批准。Appraisal guest 只收到 `user_text`、`now`；规划/转换/安全 guest 只收到生成需要的文本、对话行为、语气、约束及输出开关，不收到完整 memory、user_id、channel_id 或内部设备状态。内建表达安全校验始终执行。

## 限额、取消与实例状态

默认 Limits：256 页线性内存、20,000,000 call fuel、2,000,000 初始化 fuel、1000 ms 调用期限、256 KiB 输入与输出。可配置上限为 1024 页、100,000,000 call fuel、20,000,000 初始化 fuel、30 s、1 MiB 输入与输出。表最多一张且必须有不超过 65,536 个元素的最大值。

每次 invoke 新建 Engine、Plugin 和 Store；不复用变量、config、线性内存或 SDK pool。不同调用没有共享 Engine 的 epoch 取消串扰。每个 Deployment 同时运行一项，最多保留 17 个运行/排队请求；宿主全局最多同时编译/执行两项。宿主禁用 SDK 默认磁盘 cache、debug/profile、memory/core dump、WASM backtrace 和 guest tracing。

期限包含排队、编译、初始化和执行的业务等待。编译不是 fuel 可中断的 WASM；超时会及时返回 cleanup pending，而 owner 保留 blocking task 和 permit，等待它退出。初始化与导出执行受各自 fuel 限制；取消不会丢弃原 JoinHandle。撤权、卸载、停服等需要实际清理的操作等待原 owner 完成。线性内存和表预算不是整个进程 RSS/编译器内存上限，也不是 CPU 百分比配额。

输出先以借用 slice 检查长度，再复制和 JSON 解码。`vendor/extism/src/plugin.rs` 的固定补丁在 SDK 复制或记录 guest error 前拒绝超过 4096 字节的错误；补丁依据及许可证在同目录。宿主只返回固定分类错误，不透出插件原始 stderr、私有输入或任意日志。

首版禁用编译缓存，独立 Engine 的初始化/编译开销明确存在。未来若加入 cache，只能保存宿主自己生成、绑定 Wasmtime 版本与目标平台的编译结果；不得从第三方包加载序列化 native artifact。

## Embedded processor 与受信设备驱动

Embedded ZIP 继续使用其有界、私有暂存的安装流程。示例 manifest 位于 `packages/alia-embedded/examples/echo-plugin/manifest.toml`；加入编译产物并命名为 `module.wasm`。`runtime = "wasm"` 只支持 processor；`plugin.wasm_limits` 使用上述 Limits，不允许申请 GPIO、USB、network 或物理表达 action。安装与扫描均不会启动执行。

`StartPluginRequest` 必须提交刚审阅的 `expected_asset_sha256`。启动生成新 instance ID；命令必须带该 ID 和稳定 command ID。初始 receipt 是 unknown，完成与结果由原 WASM owner 发布；同 ID 同参数重试读取原 receipt。Stop 关闭准入，等待原运行器 worker 全部结束后才转为 Stopped；随后才能卸载或安装替代版本。

`runtime = "trusted_native"` 仅用于确需宿主能力的 sensor/actuator/bridge，且 Start 还必须显式设置 `allow_trusted_native = true`。原生入口必须是可执行资产，没有扩展名解释器派发。该模式具有宿主账户权限，WASM 限额不适用，不能用于运行未审核 processor；它保留原来的 Unix 子进程组 owner 和清理机制。绑定设备控制方仍须由 Server 管理政策授权，不能把控制 token 本身解释成人类管理员身份。

## 验证入口

`./scripts/verify-software.sh` 先构建 Rust PDK guest，再执行包含运行器、Server 和 Embedded 的检查；PostgreSQL 测试需要实际隔离数据库，缺少依赖会明确失败。运行器测试覆盖禁止的 imports/内存、fresh instance、独立 Engine 超时、旧 Arc 撤权、SDK 错误长度、包升级 pin 和真实 Rust guest。Server fixture 覆盖管理员 HTTP 管理及统一 Execution 账本，Embedded fixture 覆盖实际 WASM echo command/receipt。测试文件不会在缺失 guest 产物时静默跳过。
