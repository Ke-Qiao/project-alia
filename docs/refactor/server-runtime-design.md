# Server library runtime 设计

状态：待实施设计，2026-09-08。本文依据当前 P2 工作树的只读核查，准备 P2 提交后的独立构建包；不表示下列运行时能力已经完成。源码行号可能随 P2 收尾移动，以同时列出的函数名定位。

目标范围是 Server 库的启动、资源所有权和退出；保留现有认证、Execution 账本和 Memory 来源边界，不重开产品架构规划。Embedded 的 `ShutdownHandle` / `ShutdownIncomplete` 语义可作为接口参考，不能据此假定 Server 中任意原生线程可以被强制终止。Web/iOS/Android 产品开发仍搁置，但已开放的 HTTP/WS 入口必须安全退出并遵守账号边界。

## 已具备的基础

`bootstrap::run` 已与日志、信号安装分离，使用调用方 token 的子 token；HTTP/gRPC 在模块启动前绑定。监听服务异常会触发同级取消。Continuous voice 有会话取消、鉴权续期和精确 scope 关闭；Execution 有独立持久账本、5 秒提交期限和单次 10 秒关闭预算，关闭等待被丢弃不会丢失 worker JoinHandle。上述局部保证仍不足以证明整个 Server 退出完成。

## 已确认缺陷

| 缺陷 | 代码证据 | 实际后果 |
|---|---|---|
| 调用方丢弃 `run` 只触发 token，没有保留整个清理流程 | [bootstrap::run:41](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/bootstrap/mod.rs:41)、[run_connected:189](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/bootstrap/mod.rs:189) | 后面的 pipeline、Execution、模块和 DB 清理 await 被跳过；取消请求不能替代 join 或成功证明。pipeline 先 `take()` 再 await 也会失去原句柄的集中所有权。 |
| Builder 对选定模块吞掉 init/start 错误，部分对象退出路径没有 rollback | [init_module:679](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/module/builder.rs:679)、[start_module:736](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/module/builder.rs:736)、[register_initialized_channel:752](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/module/builder.rs:752)、[STT/TTS factory:135](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/module/factory.rs:135) | 失败模块仍进入后续步骤；扩展/渠道初始化失败或注册失败的对象没有统一 stop。未知 STT/TTS 配置回退 Noop。现有 tools.start 失败分支的局部清理无法覆盖全部构建失败和 drop。 |
| 监听器 drain 不等于连接与会话结束 | [raw gRPC incoming:168](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/bootstrap/mod.rs:168)、[drain:243](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/bootstrap/mod.rs:243)、[WS:74](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/ws/handler.rs:74)、[voice actor spawn:172](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/grpc/voice_session.rs:172) | 10 秒后 abort listener 不能证明框架内部 HTTP/2 连接或升级后的 WS 已结束。WS 收发均未监听 runtime cancel；voice actor 有取消但没有纳入可 join 的总所有者。legacy 上传已有 40 秒上传期限，但 auth/owner await 与 runtime 退出没有统一取消。 |
| 后台生产者丢弃任务句柄，部分 stop 只记录或通知 | [scheduler:511](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/scheduler/cron.rs:511)、[rules:245](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/rules/engine.rs:245)、[insights:1967](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/providers/insights/scheduled.rs:1967)、[sensor cleanup:702](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/providers/sensor/registry.rs:702)、[EmbeddedChannel::stop:724](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/channels/embedded/channel.rs:724) | 根 token 能通知这些循环退出，但不能证明退出或获取异常。scheduler/rules 未提供相应 stop join；insights stop 删除 scheduler 项却未 join 两个循环；embedded 渠道的发现、连接、传感同步循环同样缺集中 join。 |
| 超时与错误传播不能保证依赖安全 | [ModuleRegistry::shutdown:54](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/module/registry.rs:54)、[ChannelRegistry::shutdown:199](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/channels/registry.rs:199)、[bootstrap cleanup:198](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/bootstrap/mod.rs:198)、[discovery shutdown:114](/Users/ke-qiao/Developer/project-alia/packages/alia-server/src/discovery.rs:114) | 每模块各 10 秒不是全局预算；timeout 会丢弃 stop future。渠道 stop 错误被吞掉，pipeline 子任务 panic 只记录。Execution 报告仍有 live worker 后，bootstrap 仍继续停渠道、插件并关闭 DB，可能破坏回执写入。discovery 没有等待停止确认。多个错误最终只保留首个返回值。 |

另有同文件的已确认 WS 边界问题：`ws_handler` 仅 decode JWT，未重新验证 active account；转发整个 EventBus，只按事件类别过滤，未按用户授权过滤。`pending` 在每次串行 send 前后增减，并不形成 64 条有界队列。此入口不得因 frontend 被延期而继续把任意已登录用户当作全局观察者；修复前应显式禁用，或在同一入口包补齐最小身份和事件授权策略。

## 目标所有权与合法状态

采用一个实例级运行时所有者，公共 `AppState` 只提供业务服务，不承担“最后一个 Arc drop 就算停止”的隐式职责。建议接口方向：Builder 创建可提前取得关闭句柄的启动对象；宿主等待 ready 得到 `ServerRuntime`，由 runtime 运行服务；宿主可在启动等待或 run future 被丢弃后，通过同一个 `ShutdownHandle` 等待最终结果。具体 Rust 签名在实现包内确定，但以下语义不可省略。

| 所有者 | 必须持有的资源 | 释放条件 |
|---|---|---|
| Server runtime | 配置、实际 listener 地址、启动记录、服务/连接/actor 任务集、关闭状态 | 清理完成，或将仍在运行的清理所有权保留在可等待句柄中。 |
| 单个模块/渠道 | 自己创建的循环、子 token、原生或 async join handle | stop 完成表示真实 join；超时不能移走唯一句柄。 |
| ExecutionService | 已领取 effect 的 executor Arc、worker handle、持久化责任 | 确切结果写入或明确失败；调用方停止等待不能释放依赖。 |
| DB 生命周期所有者 | 自建 pool，或宿主明确借用的 pool | 所有持 DB 引用的工作已结束后关闭自建 pool；库不得自行关闭宿主借用的共享 pool。 |

顶层合法迁移为 `Building → Starting → Running → Stopping → Stopped`。任意失败、取消或调用 future drop 都进入同一个 `Stopping` 流程。超过预算是 `Stopping` 的一次 `ShutdownIncomplete` 报告，不是伪造的 `Stopped`。最终结果包含原始启动/服务错误、各阶段停止错误和仍未结束的资源名称；后续 join 成功不抹去已报告的失败事实。

每个资源在首次可能分配外部状态的 init/start await **之前**进入清理记录。允许部分 init、部分 start 失败后的 stop；实现必须区分“未实际激活”和“需要实际恢复”的状态，避免回滚时制造新的外部动作。相同模块 Arc 注入多个角色时只清理一次。显式禁用或测试注入 Noop 合法，已选定必需能力的 factory/feature/init/start 失败必须阻止 ready；可选能力的 Degraded 必须有明确配置和可观察原因。

运行时使用一个清理所有者，不给每模块新建 Tokio runtime，不把所有 async stop 都包装成多层 blocking supervisor。已存在的同步原生 join 在其后端保留单一所有权，必要时交给受管理的 blocking 工作。异步 timeout 不能抢占正在同步阻塞的原生代码；此时必须保留未完成证据。

## 停止顺序与预算

1. 封闭新 HTTP/gRPC/WS、pipeline、scheduler/rules/insights 的业务 admission，并发出生产者取消；撤销发现广告的新连接入口。已领取的 Execution 不随响应 token 直接中止。
2. 取消并 join 所有入站会话、请求等待者及生产者循环。Continuous voice 的精确 close fence 要在 outbound channel 仍可用时完成。WS 的 recv、send 都响应取消；握手、Refresh/Auth 和 legacy stream 的等待统一受 runtime 控制。
3. 给 listener 和现存连接有限排空时间；期限后关闭实例拥有的 socket 并等待连接计数归零。追踪升级后的 WS 与框架内部连接，不能只 abort 顶层 serve 任务。请求任务的 join/panic 错误写入清理报告。
4. 请求 Execution 停止 admission、取消全部未领取会话自动意图，等待已领取任务及回执。若局部 10 秒预算已耗尽但 worker 仍运行，总清理所有者继续保留 outbound/plugins/DB，向宿主返回可继续等待的 incomplete；不继续拆除这些依赖。
5. Execution 和其他依赖使用者结束后，再按实际依赖逆序 stop outbound、插件、模型/Memory/会话存储等。停止失败继续清理与其无依赖的资源，不能绕过仍在使用的依赖。
6. 等待 discovery 的停止确认，最后关闭自有 PostgreSQL pool并发布最终报告。借用 pool 只释放本实例引用。

整体关闭预算覆盖以上阶段，采用同一个绝对 deadline；可以给连接排空较短子预算，但不能按模块数累加超时时间。预算耗尽只结束宿主这一轮等待，清理所有者继续存在。`main` 如需承诺 CLI 返回期限，应显式管理 Tokio runtime，并将最终 `shutdown_timeout` 写为独立尾部预算；库不安装 logger、信号或退出进程。原生任务仍活跃时必须以失败结束 CLI，不能宣称硬件或线程已安全停止。

## 分包实施与独占范围

以下均为待实施。按包提交，每包先通过自身真实故障测试，再进入下一包；共享文件由 root 串行集成。

| 包 | 完整结果 | 建议独占文件 |
|---|---|---|
| R1：启动与统一清理所有者 | 构建/启动/drop rollback、必需模块严格失败、typed 关闭报告、提前可取句柄；不再由裸 AppState 代表运行时所有权 | `bootstrap/mod.rs`、新增 `bootstrap/runtime.rs` / cleanup 模块、`module/{builder,registry,lifecycle,factory}.rs`、`state.rs`、必要 `config.rs` 和 library runtime tests。 |
| R2：连接与入站 actor | 实际 HTTP/2、WS、legacy、continuous 会话全部可取消和 join；WS active account/事件授权及背压边界 | `ws/handler.rs`、`grpc/{audio_ingress,voice_session}.rs`、必要连接包装模块及真实 loopback tests；bootstrap 装配由 R1 所有者合并。 |
| R3：模块后台任务与依赖排空 | scheduler/rules/insights/sensor/channel 的所有循环句柄被保留并 join；Execution incomplete 时依赖保持活跃；停止错误不丢失 | `scheduler/cron.rs`、`rules/engine.rs`、`providers/{insights/scheduled,sensor/registry}.rs`、`channels/{registry,embedded/channel}.rs`、`channels/pipeline/supervision.rs`、`discovery.rs`；Execution 只在必要接口处与其所有者协调。 |
| R4：可组合配置和 CLI 总退出 | 独立 HTTP/gRPC listen 配置支持端口 0、暴露实际地址、mDNS 使用实际端口；两实例独立身份/目录；CLI 尾部 runtime 预算 | `config.rs`、`main.rs`、`discovery.rs`、配置示例及 bootstrap 组合测试。不得让旧 `http_port + 1` 把随机 HTTP 端口变成 gRPC 端口 1。 |

R2/R3 可独立开发其任务所有权，但没有集成 R1 的关闭所有者之前不宣告整体退出完成。R4 不引入跨平台插件系统，也不要求此次实现进程内媒体传输。

## 真实测试出口

- 显式 fake module/channel 在 factory、init、start 各阶段失败；记录调用次序，确认部分初始化对象也停止一次、依赖逆序、必需模块失败没有 ready。禁用模块仍可启动。
- 单线程 Tokio 宿主取消/丢弃 startup 和 run future：关闭句柄仍可等待，端口释放，所有 fake worker 已 join；重复/并发 shutdown 不重复清理。两个库实例停止一个不影响另一个，宿主共享 pool 仍可使用。
- 临时 loopback 的占用 HTTP/gRPC 端口、serve 错误、无限 HTTP/2 request、持续 WS、legacy 永不 EOF、continuous auth 等待。必须验证对端连接断开和 actor/连接计数为零，不能只断言顶层函数返回。
- 用实际 PostgreSQL 独立 schema及可控 executor：stop 期间 effect 仍在执行时，channel/plugin/DB 不先关闭；释放 executor 后成功写入最终事实。表锁和回执写失败要进入可检查错误，未完成 effect 不被写成已取消或成功。
- 可控 stop 永远不返回：关闭调用在预算内报告准确 pending 集合，释放 stop 后同一 handle 能收到真实终态。非合作原生任务用受控测试替身验证“未完成”，不测试或宣称可强杀硬件。
- scheduler/rules/insights/sensor/embedded 重复 start 的处理明确；循环 panic 与 stop 错误传递到宿主。停止后没有新的 effect admission、数据库写入或设备重连。
- WS 账号停用、跨用户订阅、慢接收端与事件滞后必须有真实 socket 测试。任何收到消息的断言必须证明账号授权和实例取消边界。

实施验收命令至少包含 Server 定向集成测试、默认与 `--no-default-features` 的 `--all-targets --locked` 检查，以及独立 PG 故障套件。Embedded 已完成的 122 项替身测试只提供接口参考，不作为 Server 的验收替代。
