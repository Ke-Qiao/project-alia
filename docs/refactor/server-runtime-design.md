# Server library runtime

状态：2026-09-08，运行时所有权、启动回滚、连接/actor 排空及后台任务停止已实现，并通过最终 workspace、真实 PostgreSQL 和 PTY 验收。本文描述当前实现，不以 Embedded 测试代替 Server 验证。

范围是 macOS Server 库和现有 Pi5 接口的启动、资源所有权与退出。Easy-Turn 仍仅在 Server。Web/iOS/Android 产品延期；现有 HTTP/WS 服务仍必须遵守身份边界并能被宿主关闭。

## 公共入口与资源所有权

```rust
let startup = ServerBuilder::from_config(config, host_cancel).start_build();
let completion = startup.shutdown_handle(); // ready 之前即可保留
let runtime = startup.wait().await?;
let http = runtime.http_addr();
let grpc = runtime.grpc_addr();
runtime.run().await?;
completion.wait().await?;
```

`ServerBuilder::from_config` 创建并拥有 PostgreSQL pool；`ServerBuilder::new` 接受宿主注入的 pool，实例退出只释放引用，不关闭宿主 pool。`build().await` 是 `start_build().wait().await` 的便利入口；`bootstrap::build` 和 `bootstrap::run` 仍可直接使用。业务代码可以 clone `runtime.state()`，但 `AppState` 不是生命周期所有者。

`ServerStartup` 拥有独立启动任务。丢弃 startup、取消其 ready waiter 或取消宿主 token 都触发回滚；丢弃调用方 future 不会 abort 唯一的启动/清理任务。预先取得的 `ShutdownHandle` 可继续等待结果。启动成功后所有权交给 `ServerRuntime`；丢弃 runtime 或它的 run/shutdown waiter 同样触发独立清理。实例使用宿主 token 的子 token，关闭一个实例不会反向取消宿主或同级实例。

| 接口 | 含义 |
|---|---|
| `ServerRuntime::state()` | 已装配的业务状态与有效配置。 |
| `http_addr()` / `grpc_addr()` | 真实绑定地址；未编译 Embedded 服务时 gRPC 为 `None`。 |
| `requested_config` | 调用方原始配置，保留原先的端口 0。 |
| `run(self)` | 启动已绑定的 listener，服务退出后等待同一清理所有者。 |
| `shutdown(self)` | 请求关闭并在本轮全局预算内等待。 |
| `shutdown_handle()` | 可复制的完成观察者；不转移唯一 worker handle。 |
| `ShutdownHandle::wait()` / `wait_until(deadline)` | 等待真实清理结果；后者超时返回可 downcast 的 `ShutdownIncomplete`。 |
| `pending_modules()` / `failures()` / `is_finished()` | 观察仍未完成的阶段、累计错误和完成状态。完成带错误不等于成功。 |

`ShutdownIncomplete` 携带 pending 名称和同一个可继续等待的 handle。deadline 只结束当前调用方的等待，不能据此移除资源、丢弃唯一 join handle或伪报成功。

## 启动和回滚

启动顺序为绑定 HTTP/gRPC、取得数据库并迁移、校验显式 owner、构建选定组件、应用持久设置、启动依赖和渠道，再启动 Execution/主动生产者。Pipeline consumer 在渠道开始发事件前建立；渠道可发送后 Execution 才开始领取 effect。

每个模块在第一次 `init` poll **之前**进入回滚 slot。slot 先持有部分初始化的 Box，再同步转换为 Arc；转换不跨 await。init/start 的错误、取消或 panic 都让构建失败。渠道成功注册后将 stop 责任转给 ChannelRegistry，失败时仍由原 slot 回滚，避免漏停与重复 stop。显式选中的 provider 未知、功能未编译、必需配置不完整或 init/start 失败均返回错误；显式 `noop` 与禁用配置仍合法。配置的 Memory 健康检查失败不会静默替换另一后端。

RuntimeCleanup 按依赖阶段与阶段内逆注册顺序执行回滚。清理所有者在专用 blocking worker 中驱动 stop future，使单线程 Tokio 宿主仍能观察退出 deadline。它使用宿主 runtime 的 I/O 与计时设施；宿主在等待完成期间必须保持 Tokio runtime 运转，不为每个模块创建新 runtime。

## 关闭顺序和完成证明

1. 关闭渠道与入站 actor admission，取消实例子 token，停止接收新连接，并撤销 discovery 广播。
2. 给 listener 一个短 drain 预算；之后关闭共享 `Connections` 中的真实 socket。继续 join listener、HTTP/2/WS 连接和 `RuntimeTasks` 中的 voice/WS/渠道管理 actor。内部框架任务不能仅凭顶层 listener 被丢弃就算结束。
3. 停止并 join Pipeline、sensor cleanup、insights、rules、scheduler 等生产者。各后台组件使用保留句柄的所有者，stop waiter 被丢弃后仍能重新等待。
4. 关闭 Execution admission，等待已 dispatch 的执行和回执。Execution 自己的等待预算耗尽但 `shutdown_pending()` 仍为 true 时，整体清理继续保留 outbound、插件、raw tools 与 DB，并向宿主返回 incomplete。不能先停这些依赖。
5. Execution 确认结束后关闭 outbound channels，再按逆序停止插件、模型和存储等资源；最后关闭自有 PostgreSQL pool。

曾发生的 Execution 局部关闭超时仍保留为历史错误；后来全部 join 会使 pending 归零，但不会把最终报告改成“从未失败”。这与 deadline 当下的 incomplete、最终是否已 join 是两个分别可观察的事实。

`ModuleLifecycle::stop` 的明确契约是：包括返回 `Err` 时，所有自有 worker 已结束，错误表示已完成清理中出现的问题。此类错误累计到最终报告，后续清理继续。若 stop panic，完成无法证明：保留该 action 的原始 owner 和剩余依赖，停止推进 pending 集合，不自动重试 panicked stop；宿主只能得到 incomplete 和可检查的错误，不能得到假成功。内部后台所有者也必须保留尚未证明退出的句柄，不能把这种情况折成普通“已完成 Err”。

清理 action 的 owner 捕获与实际 stop future 分开保留，避免 future 的 panic unwind 摧毁唯一资源引用。非合作原生 stop 或不可恢复 panic 可能永久 pending；库没有安全强杀任意线程的能力。Discovery 能证明 mDNS daemon 的 unregister/shutdown ACK；其第三方库没有 native OS-thread join API，因此不宣称已取得该线程的 join 证据。

## 连接和账号边界

HTTP 与 gRPC 都通过 `alia_transport::connections::Connections` 包装已接受的 TCP socket。`close` 与 `wrap` 使用同一个 admission 锁，关闭后新 socket 不会加入；计数归零并且 listener/actor join 后才继续清理依赖。`RuntimeTasks` 封闭后拒绝新任务，收集失败和 panic，不用裸 `tokio::spawn` 代表已经托管。

现有 `/api/v1/ws` 定义为**管理员诊断流**，因为 EventBus 包含全局设备、工具和会话事件。握手用数据库当前 active 状态和当前角色授权，普通用户 403、停用账户 401；不信任 JWT 中旧 admin claim。连接内每 30 秒复查，角色降级或停用后退出。发送有 1 秒期限；事件滞后断开，不能静默跳过丢失事件。用户级订阅产品不在本轮范围，不能以其延期为由继续向任意登录用户广播全局会话数据。

## 配置与 CLI

`server.grpc_port` 可单独设置，`0` 请求独立随机端口。未设置时固定 HTTP 端口仍默认 `HTTP + 1`；HTTP 为 `0` 时 gRPC 也独立绑定 `0`，不会推导为端口 1。有效配置与 mDNS 使用实际绑定值。`shutdown_grace_ms` 是整体清理等待预算（默认 10000 ms）；`drain_timeout_ms` 是其中 listener drain 的短预算（默认 1000 ms）。两实例应使用不同身份文件和配置目录。

库不安装 logger 或进程信号，也不退出进程。Server binary 显式创建 Tokio runtime；返回库结果后执行独立 1 秒 `shutdown_timeout` 尾部预算。库的 incomplete 或清理错误保留非零退出结果，进程尾部预算不构成原生线程已经安全停止的证明。初始 OOBE 的 `run_setup` 保留独立流程，本包仅迁移其 discovery async start/shutdown 调用，不能把普通 ServerRuntime 的所有权保证外推到任意 setup future。

## 已验证与集成出口

`tests/server_runtime.rs` 使用真实 PostgreSQL 独立 schema和 loopback socket；缺少测试数据库直接失败。当前 12 项运行时测试覆盖：

- init/start 部分失败精确停止一次及逆序清理；构建取消、drop ready waiter 后仍由提前保留的 handle 等待回滚。
- 原生 stop 超时、启动失败后原生 rollback 超时，均保留 owner 与 pool，释放替身后同一 handle 到达真实终态。
- stop 已 join 后返回 Err，仍清理后续资源并关闭自有 pool；重复等待不会擦除错误。
- drop run 后端口释放、worker 排空、宿主 token不受反向影响；同进程两个实例停一个后另一个仍能实际响应 HTTP，宿主 pool 可继续使用。
- 占用 listener 和非法 provider 配置在模块 init 前失败；HTTP/gRPC 端口 0 的实际值可检查。
- 工具目录持久化失败使启动失败；撤除数据库故障后可重新构建并完整关闭，不能带着未持久化的工具目录报告 ready。
- 真实持久 effect 跨过宿主短预算及 Execution 自身 10 秒等待预算时，outbound、插件和 DB 仍保持活跃；释放 executor 后回执为 `Succeeded`，之后才关闭依赖和自有 pool；最终报告仍保留先前局部超时错误。
- 管理员 WS 可用、普通用户与禁用账户拒绝；真实连接上的降级/禁用复查断开；shutdown 排空升级 socket 和 actor。

同一集成测试入口还包含 5 项真实 Insights 来源/错误传播测试和 4 项调度/规则来源版本测试。最终 workspace 中该入口为 **21 passed、0 failed、0 ignored**（40.95 秒）；其中 12 项为上述运行时/WS 验证，5 项为 Insights 验证，4 项为自动化来源验证。

另有独立子进程 panic 故障测试：stop panic 后原 owner 和 DB 阶段仍被保留，deadline 返回 incomplete，依赖未被停止。子进程以显式 runtime 尾部预算结束，避免向正常测试进程遗留故意永不完成的 worker。`cargo test -p alia-server --lib module::lifecycle::tests --locked` 已通过该项。

后台组件另有定向测试，覆盖 scheduler/rules/sensors/channel/insights 的停止、重复启动、回滚和等待句柄保留；它们与真实 Runtime 测试共同验证整体契约。当前尚不以这些替身测试声称 Pi5 真实音频硬件退出、任意外部推理原生线程停止，或 Delivery 播放完成。

最终默认 workspace 为 561 项通过、0 失败、4 项显式跳过；日志另含上述 panic 测试的 1 次子进程内部运行，不重复计数。4 项跳过的真实 PTY 和真实 Easy-Turn/gRPC 测试已分别显式运行通过。Server 无默认特性 all-targets 检查通过。完整命令、独立数据库及限制见[构建记录](progress.md)。
