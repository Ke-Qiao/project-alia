# Extism 纯 Rust 插件接入研究

> 实施更新（2026-10-18）：Rust WASM 执行器、表达 ABI 3、工具 Execution 适配及 Embedded processor 已进入代码；当前部署与限额见[WASM 插件操作说明](../operations/wasm-plugins.md)。本页其余历史选型/目标文字不代替当前接口或测试结果。

初始调研日期：2026-10-18。研究时尚未接入、编译或完成沙箱验收；实施状态见页首更新。本文只负责 WASM 插件；Rig、工具 Schema 和模型适配见主研究报告。

## 1. 选择与交付范围

建议 Rust 宿主直接依赖 `extism`，第三方作者使用 Rust `extism-pdk` 编译 `wasm32-unknown-unknown` 产物。发布包只包含 WASM、严格 manifest 和静态资源；安装期间不运行 Cargo/build.rs、脚本或下载依赖，不引入 Bun/Python运行时，也不需要安装 C SDK 动态库。

Extism 已封装字节 ABI、内存交换、typed wrappers、host functions、fuel、timeout/cancel 和实例管理，底层是 Wasmtime。首轮不另写 WASM ABI、通用进程平台或实例池。Alia 保留授权、生命周期、Context/Execution/Delivery/Body 的业务职责。

固定研究版本为 Extism **1.41.0**（发布 commit `051b765`，2026-10-09），底层 Wasmtime **49**；Rust PDK 当前 **1.4.1**。Extism/PDK 为 BSD-3-Clause，Wasmtime 为 Apache-2.0 WITH LLVM-exception。[发布](https://github.com/extism/extism/releases/tag/v1.41.0)、[依赖](https://raw.githubusercontent.com/extism/extism/051b765/runtime/Cargo.toml)、[许可证](https://raw.githubusercontent.com/extism/extism/051b765/Cargo.toml)、[PDK](https://raw.githubusercontent.com/extism/rust-pdk/main/Cargo.toml)

首轮能力明确限定：表达候选计算、私有 Appraisal 候选计算、无副作用工具计算、已授权工具的有限存储读取、Embedded processor 的单次事件变换。网络、直接设备操作、长驻 guest 事件循环和同步宿主写操作暂不开放；效果通过现有核心入口执行。

## 2. 依赖与产物

以下是待实施配置，不是已经编译通过的 Cargo 修改：

```toml
# Rust host
extism = { version = "=1.41.0", default-features = false }
# Rust guest，独立构建并提交 Cargo.lock
extism-pdk = { version = "=1.4.1", default-features = false }
[lib]
crate-type = ["cdylib"]
```

Host 关闭 `http`、`register-http`、`register-filesystem`、`wasmtime-default-features`；PDK 关闭默认 `http/msgpack`。JSON 与 serde 已足够。Cargo feature 统一后必须检查实际依赖图，不能仅检查这一行。`default-features=false` 不会去掉 Extism 明确依赖的 Cranelift/GC/WASI crate；运行时仍必须 `.with_wasi(false)`。

Host 使用 `extism::Manifest` 和 `extism::Wasm::data(verified_bytes)`，由核心填写预算、config 和 imports。禁止直接把上传内容传给 `PluginBuilder::new(bytes)` 的格式自动检测入口：它也会尝试将非 WASM 字节解析为 TOML/JSON manifest。禁止第三方提供文件路径、下载 URL、指针形式的数据或预编译 native artifact。[加载源码](https://raw.githubusercontent.com/extism/extism/051b765/runtime/src/manifest.rs)

包内只支持一个 guest module 和列出的 exports。Rust PDK 使用 `#[plugin_fn]` 与 `Json<T>`，编译 `cargo build --locked --release --target wasm32-unknown-unknown`；不把 Rust trait object、宿主指针或 SeaORM 类型作为 ABI。[PDK 示例](https://raw.githubusercontent.com/extism/rust-pdk/main/README.md)

可提取一个很小的 `alia-plugin-api` crate，只放 serde DTO、版本和长度常量，供 host/guest 共用；不依赖 Server、Tokio 或 Extism host。作者可选择其他 Rust依赖，但宿主只执行最终经校验的 WASM，不能将 guest 源码构建放进安装流程。

## 3. 最小宿主接口

现有 `ExternalExpressionPlugin` 和 `ToolExecutor` 标识了表达与工具两个业务接入点；可随 Rig/工具重构替换其接口，不要求保留旧 trait。具体 WASM runner 只有一份，并接入统一执行准入。下面为接口草案，名称可随实施调整；不会新增另一套批准系统。

```rust
struct WasmDeployment {
    identity: DeploymentIdentity, // 不可变的资产/host ABI/runtime/policy 摘要
    wasm: Arc<[u8]>,              // 校验后的同一份字节
    admission: AdmissionGate,    // 复用现有关闭准入/RuntimeTasks 机制
}
struct InvocationPermit {
    run_id: Uuid,
    deadline: Instant,           // 可信入口的绝对期限，排队不刷新
    cancel: CancellationToken,
    grants: PinnedGrants,        // 宿主产生；guest 不反序列化此对象
    context: TrustedContext,     // 含来源/隐私/主体；不是 guest 输入
}
impl WasmDeployment {
    async fn invoke(&self, export: Export, input: Vec<u8>,
                    permit: InvocationPermit) -> Result<Vec<u8>>;
    async fn shutdown(&self) -> Result<()>;
}
```

`invoke` 的顺序固定为：检查输入预算与当前身份 → 获取有界名额 → 再核验期限/grant/privacy → 创建独立 runtime → 运行 → 先检查原始输出长度，再解析 DTO → 复验当前授权与来源 → 交给核心采纳。

阻塞编译与 `Plugin::call` 在 `spawn_blocking` 中执行，外层 supervisor 由现有 `RuntimeTasks` 持有。调用方取消只结束其等待；原 supervisor 保存 blocking JoinHandle、cancel watcher 和名额，直到工作线程退出、watcher join、Plugin/Engine drop 后才释放。禁止 drop/abort JoinHandle 后宣称清理成功。

业务期限到达时发送 `CancelHandle::cancel()`，返回 timeout/cleanup-pending；关闭准入和等待清理由原 owner 继续完成。停用/卸载不能只靠调用 future 被丢弃来释放资源。初始化期间 cancel 可能尚无 active timer，因此必须始终设置 initialization fuel，等待终态时仍保留 owner。

## 4. 取消、状态隔离与为何首轮不用 Pool

**首轮每次 invoke 新建 CompiledPlugin/Engine、Plugin/Store，调用结束即销毁。** 使用宿主指定的编译缓存降低重复编译开销；先设低并发且有界排队，不增加池框架。缓存只接受宿主从已校验WASM生成的产物，目录不可由guest访问或替换，并绑定runtime/config/目标平台；第三方预编译/序列化artifact不能进入可信cache或反序列化加载入口。

固定源码证据：`CompiledPlugin` 包含 Engine；`Plugin::new_from_compiled` 与其共享 Engine；每个调用 `set_epoch_deadline(1)`；timer 对某 plugin ID 取消/到期时执行 `engine.increment_epoch()`。同一 Engine 并行调用可能互相中断，这是源码推断，尚未运行复现。[plugin.rs](https://raw.githubusercontent.com/extism/extism/051b765/runtime/src/plugin.rs)、[timer.rs](https://raw.githubusercontent.com/extism/extism/051b765/runtime/src/timer.rs)

串行复用也不能直接宣称完全解决：`Stop` 和 `Cancel` 只是发到后台 timer，无公开完成屏障。旧 Cancel 找不到 active ID 会被忽略；但旧 Cancel/timeout 尚待处理时，新调用可能已经设置同一 Engine 的 deadline。即使旧 call 已返回且 watcher 已 join，旧消息仍可能增加共享 epoch。独立 Engine 避免这一跨调用影响；不得把“发送取消成功”当作 timer 清空。

Extism Pool 复用 Plugin；guest 的 vars、linear memory、globals 可能跨 call 留存。其内部 reset 不是每次调用后的跨用户数据清理承诺；`_start` 或特定内部条件的重建不能代替隔离。首轮不用 Pool，也不跨用户复用 Plugin，即使 config 不含身份仍必须如此。[Pool](https://raw.githubusercontent.com/extism/extism/051b765/runtime/src/pool.rs)、[PDK vars说明](https://raw.githubusercontent.com/extism/rust-pdk/main/README.md)

未来只有测得成本确有必要，且上游版本/修复提供取消屏障、通过并行和延迟取消负例后，再考虑共享 compiled module；也可向Extism贡献带确认的timer barrier修复后再允许复用，不能用任意sleep代替确认。复用 Store 仍需独立的数据隔离设计。

## 5. 权限和真实限额

| 项目 | 首轮配置/实现 | 能保证的边界 |
| --- | --- | --- |
| WASI | `with_wasi(false)`；拒绝 WASI imports | 不提供宿主文件、stdio、时钟、随机/网络等 WASI 能力 |
| HTTP/加载 | 禁相关 features；allowed_hosts 显式空；只用已验证 Data | guest 不通过 Extism HTTP 发请求；库不下载模块 |
| Host imports | 按 kind 固定白名单；未知 namespace/import 拒绝 | guest 不能通过 manifest 自行增权 |
| Memory | 显式 `max_pages`；限制 guest memory/table 声明和数量 | WASM 分配有预算；不是进程 RSS 上限 |
| Fuel | 显式 call fuel + initialization fuel | 初始化/guest计算的有限指令工作量；不是 CPU 百分比 |
| Timeout | manifest timeout + 外层原 deadline | export 计算中断与业务等待上限；初始化/编译另有owner |
| Input/output | 表达沿用256KiB；工具输入16KiB、回执64KiB | 拷贝/解析前检查长度；不能只事后截断 |
| Logs/errors | guest 原文不写宿主日志；错误转换为固定分类 | 私有输入不能借日志/错误dump外泄 |
| Concurrency | 复用有限执行名额；队列有长度与等待期限 | 不因 spawn_blocking 无限排队消耗内存 |

成功输出应先借用 guest 字节（如 SDK 的 `call::<_, &[u8]>` 返回形式），检查长度后再复制或解析，不能直接用 `Vec`/`Json<T>` wrapper 提前分配。错误路径还需库内补丁：1.41.0 的 guest error 转换在 `plugin.rs` 内先执行字符串复制，外层限额和关闭 tracing 无法阻止这次分配。严格错误输出预算需要在复制前限制长度并返回固定错误分类；完成前只能声称成功结果有显式预算，guest 错误暂受 WASM 内存预算间接约束。[错误转换](https://raw.githubusercontent.com/extism/extism/051b765/runtime/src/plugin.rs)

Extism `MemoryLimiter::table_growing` 在 table 未声明最大值时允许增长；`max_pages` 不是所有 table/GC/host allocation 的总预算。因此接入需使用成熟 `wasmparser` 做 guest 准入：限定单模块、imports、memory/table 数量与上下限，拒绝未经支持的 memory64/shared-memory/threads/GC 等 guest 特性；要求 table 有受限最大值，不能信作者使用Rust就自然安全。不要自写二进制解析器。该准入检查必须针对 guest，不能误禁 Extism kernel 自身所需特性。[CurrentPlugin/limiter](https://raw.githubusercontent.com/extism/extism/051b765/runtime/src/current_plugin.rs)

Rust PDK示例的实际 linker产物是否满足上述声明约束，必须作为构建验收；预算数值以真实示例运行校准，不伪称64MiB等初值已覆盖所有插件。Extism 在实例化后会 reset其内部增长计数，更不能把 `max_pages` 直接标成整进程的固定峰值。

Fuel与epoch不能中断阻塞中的host callback，也不覆盖Cranelift编译的CPU/RSS。首轮callback只做有限计算/有期限的只读访问；编译限字节、限结构、限并发。若产品要求对恶意输入强制整进程内存/编译时间上限，单独Extism不满足，需另选带OS资源边界的worker；这不是当前已验证能力。[Wasmtime限制](https://docs.wasmtime.dev/api/wasmtime/struct.Config.html)

官网manifest页面称 `allowed_hosts=null` 允许全部，但1.41.0源码 `None => false`；使用固定源码和负例核验，不照抄概念文档。即使将来开放HTTP，域名allowlist也不能代替Alia的凭据/重定向/SSRF政策；本轮fetch仍延期。[文档](https://extism.org/docs/concepts/manifest/)、[固定HTTP实现](https://raw.githubusercontent.com/extism/extism/051b765/runtime/src/pdk.rs)

`DebugOptions::default()`会读 `EXTISM_DEBUG/COREDUMP/MEMDUMP/PROFILE`；缓存默认也读宿主环境/系统配置。必须显式传入无dump、无profile、无debug的DebugOptions，以及宿主私有cache配置或禁cache。开启WASI时环境变量 `EXTISM_ENABLE_WASI_OUTPUT` 会继承stdout/stderr，首轮关闭WASI。[builder](https://raw.githubusercontent.com/extism/extism/051b765/runtime/src/plugin_builder.rs)、[WASI构造](https://raw.githubusercontent.com/extism/extism/051b765/runtime/src/current_plugin.rs)

Extism内置log imports和guest错误处理会发tracing原文。可以在专用blocking任务内对整个build/call使用 `tracing::subscriber::with_default(NoSubscriber, ...)`，核心仅另记run_id/耗时/固定错误码；绝不能再用 `{error:#}`记录guest错误。私有模式同时禁dump。此抑制方案须以secret-canary覆盖log、error和trap路径，不能凭配置命名判定已通过。

## 6. 表达ABI和私有Appraisal

WASM manifest指定 `abi = "alia.wasm.v1"` 及 `alia_expression_v1` export。领域请求/响应复用现有最小DTO；以新的运行模式显式迁移，不把旧JSON子进程manifest隐式解释成WASM。

`WasmExpressionPlugin`接入统一表达业务接口；意图、变换、安全策略均是候选结果，核心最终安全校验仍执行。可以删除JSON进程实现的command/working_dir/env/stdio/stderr/process-group/waitid代码和旧示例启动链；DTO、预算、输出校验和registry shutdown保留。示例由Rust PDK导出一个纯函数，不保留新旧两条默认外部运行路径。

私有Appraisal输入仍只有 `user_text` 与可信 `now`。不注入user/channel ID、完整memory、数据库、网络或存储imports。events最多64，值域/来源/TTL由现有转换与核心政策重新构造；插件不能声明自己是可信用户证据。

`provenance_pin`必须由宿主在部署时构造并固定，包括：资产/所有WASM字节摘要、业务ABI/export和Schema摘要、宿主build digest（含实际Extism/Wasmtime/kernel版本）、有效runtime限制、host import契约版本、公开config摘要、有效grant/policy修订。显示version或单个wasm hash不等于全部部署身份；不要将每次run_id或用户正文放进实现pin。

复用当前持久 `approved_appraisal_engines[name] == pin`入口，并同时校验当前调用主体、grant、Context来源和隐私epoch。等待准入后/调用前再次校验，输出采纳前再次核验；撤权后旧结果丢弃。未审核或无签名可安装，但只有明确批准该pin的私有数据能力后才接收正文。签名和人工审核不作为安装硬前提，也不自动授予私有数据权限。

没有完成文件/网络/日志/跨用户残留负例前，私有入口继续fail-closed；不能因为方法能返回一个hash就接通。

## 7. 工具插件及Host functions

安装manifest静态声明工具名、export、Schema和所需能力，登记由核心完成。guest没有 `register_tool`、raw ToolRegistry/DB/配置读取接口；Rig工具适配仍调用Execution入口。部署pin作为统一工具登记的implementation revision。

首轮 `alia_tool_v1` 返回数据或有限的候选效果列表，工具guest不直接发网络/设备/存储写。计算型工具可以直接完成；需要副作用时，候选必须经核心作为新的普通ToolCall提交Execution，绑定原来源、parent effect和核心分配的稳定index，再按该具体目标/参数取得批准。最多少量候选；禁止把描述文本直接执行、递归运行工具或自动重试unknown。原计算调用成功不代表候选效果已经成功。

无需另造工作流DSL：沿用现有ToolCall、EffectRun和批准UI，仅增加明确的“候选结果”DTO。若该映射尚未实现，则该类能力显示不支持，首轮只开放计算工具。

可有一个只读host函数 `alia.storage_get_v1(collection, key)`：namespace固定为当前deployment，user/channel/conversation scope由可信permit派生；guest参数不能指定owner、任意plugin或raw scope。限制collection、单条/累计读取字节、调用次数；未授予就不链接此import。

该callback通过 `CurrentPlugin::host_context<InvocationContext>()`读取当前permit；缺失/类型不匹配/过期/撤权返回明确拒绝，不能unwrap/panic。Extism在初始化后才注入call host_context，因此初始化阶段调用必须拒绝；不得把上次请求context存入共享UserData补洞。[host_context](https://raw.githubusercontent.com/extism/extism/051b765/runtime/src/current_plugin.rs)、[调用顺序](https://raw.githubusercontent.com/extism/extism/051b765/runtime/src/plugin.rs)

同步callback在blocking线程里桥接现有async只读storage时，使用原deadline与cancel等待，操作返回前复验scope；不在Tokio异步worker上同步阻塞，不从guest调用再占同一Execution worker队列。任何后续host写能力都必须另行定义效果身份、批准绑定和unknown，不能只是给这个通用callback加一个write动词。

## 8. 安装、启用、升级、撤权和卸载

复用Embedded现有有界ZIP私有staging、路径/链接拒绝、资产摘要、原子发布；可抽出共享asset校验代码给Server，避免两套解包器。新manifest默认拒绝未知字段。现有16MiB archive/64MiB expanded预算可先保留；manifest只能请求比host更小的预算。

| 操作 | 可实施状态与顺序 |
| --- | --- |
| install | 校验包/manifest/WASM结构后原子发布为Installed；不instantiate、不执行healthcheck、无grant |
| grant | 当前管理员复验；记录精确deployment、主体/scope、能力、限额、policy修订；不自动启用 |
| enable | 核验当前资产与grant；验证imports/exports/runtime profile；Starting→Running后才登记可调用工具 |
| invoke | 当前run下独立Engine/Plugin；run identity由host生成，名额/owner保留至完整退出 |
| upgrade | staging新资产→关闭旧准入→按原期限排空旧调用→旧owner结束→切换新identity；新pin重新授权，不继承旧效果批准 |
| disable/revoke | 原子关闭准入并提升grant修订→取消原run→Stopping；清理未完保留Stopping/pending，不发布替代实例 |
| failure | 记录Failed及固定分类；仍有worker时owner保留，不能按状态名推断可卸载 |
| uninstall | 必须无活跃/清理owner；依现有retain/delete/export-required规则处理数据，再删除资产并审计 |

stop/upgrade请求等待超时只是该管理请求结束，不撤销已开始的清理。旧任务回报必须匹配原deployment/run_id，不能覆盖同名新部署。资源不支持时保留Installed/不可启用；绝不回退原生执行。

## 9. Embedded适用范围和删除边界

纯processor/事件转换可迁移为 `alia_event_v1`：宿主按bounded事件队列逐次调用，同一输入和结果仍带现有device/boot/instance/command身份；WASM返回候选事件，由宿主验证类型、预算、序列与当前grant后发布。

Sensor/Actuator/Bridge目前常需持续事件源、USB/GPIO或设备库，不能因Rust PDK可编译就宣称可全迁移。首轮设备I/O保留受信Rust driver，WASM仅拿到明确授予的有限snapshot或产出动作候选；实际动作继续走Body/Execution及现有命令回执。没有直接USB/GPIO imports，也没有guest线程/进程。

现有echo示例可改成一个Rust PDK命令/事件变换示例；相应JSON子进程transport可在全部caller迁移后删除。PN02的资产校验、registry状态、run/command身份、事件预算、gRPC协议和unknown语义保留；过程杀组逻辑仅在原生backend彻底移除时删除。已有受信native能力若产品继续支持，明确独立授权模式，不冒充WASM沙箱。

manifest旧 `max_cpu_percent`不能映射为fuel百分比；WASM模式用fuel/timeout/页面预算，旧字段迁移时显式拒绝或人工转换。`max_memory_mb`不能继续展示为已经实施的全进程RSS承诺。

## 10. 迁移步骤与验收

1. 固定host/guest工具链与依赖，构建唯一Rust PDK纯表达fixture；完成最小runner和无HTTP/WASI配置，替换受信JSON示例。
2. 用真实WASM负例验证无限初始化/无限export、memory/table增长、超输出、malformed JSON、log/error secret-canary、未授予import、未知export。
3. 加入取消/超时/卸载竞态：原owner完成、名额不早释放、旧handle延迟取消不影响下次invoke；对共享CompiledPlugin单独做反证fixture，不能用独立Engine通过来宣称上游共享安全。
4. 通过不同用户交替调用验证global/vars/memory不残留；安装→grant→启用→调用→升级→撤权→卸载和storage retention完整闭环。
5. 接入计算工具及限定只读storage；候选副作用必须通过新的Execution效果批准后才算支持该类工具。
6. 私有Appraisal的pin/grant/隐私epoch/输出采纳负例全部通过后才开放正文；最后在Pi ARM64测同套fixture及实际资源成本。

## 11. 已知验证边界

本机Rust为1.95.0，仅安装aarch64-apple-darwin target；没有Extism/Wasmtime/WAT缓存。Wasmtime49当前要求Rust1.96，因此本次没有升级工具链、安装新target或修改repo依赖；也没有调用真实模型。[Wasmtime版本/MSRV](https://docs.rs/crate/wasmtime/latest/source/Cargo.toml)

曾尝试读取官方release metadata为临时fixture取证：sandbox DNS失败，提升后GitHub API返回403。按研究范围停止二进制获取；固定Rust源码可读取。**共享epoch串扰、初始化取消窗口和日志方案是源码核查/设计结论，未执行1.41.0 runtime复现。** 不以旧版或模拟timer结果替代实测。

Wasmtime当前把Apple ARM64与Linux GNU ARM64列Tier2，Intel Mac列Tier1；ARM64 musl更低。能支持目标架构不等于Alia已通过Pi验证。新依赖的最终二进制增量、冷/热编译时延、每invoke独立Engine的CPU/RSS、实际fuel值、长时运行泄漏均未测量。[官方平台等级](https://docs.wasmtime.dev/stability-tiers.html)

若独立Engine成本不可接受，后续先寻求Extism取消屏障修复/保证；若明确需要WIT Component Model、异步WASI、严格可中断host I/O，才评估直接Wasmtime。当前优先复用Extism已有ABI/运行机制，保留少量Alia必须拥有的权限、状态与效果边界。
