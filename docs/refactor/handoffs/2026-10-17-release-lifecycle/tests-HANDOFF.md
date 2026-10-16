# Release lifecycle — Tests

2026-10-17；基线 `1c63218`，起始软件内容为 `4e2968f18d765f326e76d24a3bcdf5bba9034efc`。本分工审计 Oct06 release smoke 与当前两端入口、关闭和持久化路径，由 Root 统一执行。审计 **6/6**、确认缺口回归 **5/5**、动态交接 **5/5** 完成。首轮真实 SIGTERM 失败与 RED 保留；一行取消域修复后 GREEN 2/0，Linux workspace 一次 1287/0/5，V2 两轮正常启动／关闭／恢复与资源回收通过。全量计数引用有明确原件映射的派生报告，原日志及 INVALID 解析报告未覆盖；源码提交身份由 Root 统一回填。

## Checklist

- [x] 阅读既有 `2026-10-06/evidence/linux/release-smoke.py` 及其两轮恢复断言。
- [x] 核对当前 Server main → Builder/Startup → Runtime cleanup，以及 Embedded main → Builder → Node cleanup。
- [x] 核对当前 staging 模板、生成身份、配置保存和 SQLite 恢复的真实含义。
- [x] 指出旧 runner 的有界等待、异步就绪、配置摘要和失败清理证据缺口。
- [x] 明确最小两轮正常 SIGTERM／重启矩阵与需保留的原有期限。
- [x] 明确不扩故障注入、绑定、政策、外部模型、硬件和 R03 暂停边界；向 Root 交接。

## 已有实现与可复用入口

原演练：`docs/refactor/handoffs/2026-10-06/evidence/linux/release-smoke.py`。它使用独立目录／数据库 schema、三个 loopback 端口、全 noop 运行时选择；先启动 Embedded，再启动 Server，完成 HTTP/gRPC 断言后依次 SIGTERM Server 和 Embedded，保留同一目录和数据库重跑一次。可直接复用其标准库 HTTP、curl HTTP/2 gRPC、protoc 解码和进程托管方式，不引入客户端框架或依赖。

首轮固定复用 `run/validation-20261017-linux-release/artifacts/alia-server` 与 `alia-embedded` 的独立保留副本，执行前后核对上一轮已审核 bytes/SHA-256；不从共享 target 缓存凭名字取二进制。首轮失败后需对确认修复重新验证并生成独立新产物；旧产物和失败记录不能冒充已含修复。

当前相关源码：

- `packages/alia-server/src/main.rs`：接收 SIGTERM/Ctrl+C 后取消 token，并继续 await 原 server future；库关闭失败继续返回错误。主程序额外 `runtime.shutdown_timeout(1s)` 是最后的 runtime 回收界限，不把未完成 native owner 判为成功。
- `packages/alia-server/src/bootstrap/builder.rs`、`runtime.rs`：ServerStartup 独立 owner 保留初始化回滚；取消后不能交付已经缓冲的成功 runtime。正常 `run` 结束关闭 admission、取消后台任务，使用原 cleanup deadline 等待；不是收到信号立即退出成功。
- `packages/alia-embedded/src/main.rs`、`module/builder.rs`：进程信号同样取消并 await 原 node future；Node 关闭 listener／连接并在同一 grace 截止时间内等待 cleanup。gRPC drain 超期保留错误，不能把强杀退出算正常关闭。
- `packages/alia-server/src/config.rs`：指定 `ALIA_CONFIG_PATH`，从配置目录加载分片；正常 `load` 可能执行已有 publication recovery。本轮新目录不放 pending marker，不扩成 setup 发布失败恢复。
- `packages/alia-server/src/bootstrap/builder.rs`：加载同目录 `settings.toml`，SQL 政策与用户状态来自同一个数据库。Root 已选择追加一个纯非政策设置的实际 PATCH／保存／重启后读取；这不覆盖设置保存失败重试。
- `packages/alia-embedded/src/config/mod.rs`、`identity.rs`、`storage/mod.rs`：配置相对路径锚定配置目录，密钥复用既有文件，SQLite WAL 数据库重开并验证 schema。Embedded 的正常配置保存目前是 `std::fs::write`；本轮正常关闭后的恢复不证明断电、部分写或崩溃持久性。

初始静态审计尚未发现生产缺口；随后实际演练已确认正常 SIGTERM 的 sensor sync 取消域错误，见下方单独记录。其余 runner 补证条件不因此自动成为已复现的生产故障。

## 最小必要场景：同一状态的两次正常生命周期

| 阶段 | 必须观测／断言 | 限制 |
|---|---|---|
| 准备 | 独立私有配置目录、空任务 DB/schema、三个明确 loopback 端口；两端二进制摘要；配置摘要和运行时选择 | 不覆盖开发者配置、身份或 DB，不安装服务，不挂设备，不访问真实模型。保持 shadow 和权限检查。 |
| 第一次启动 | Embedded 普通进程提供有效 `GetBindingStatus`；Server `/health`、服务 gRPC 和 channel online 状态可用；每次 readiness 检查同时确认原 PID 仍运行 | TCP accept 只是早期信号；需真实协议响应才算就绪。健康报告按 noop 的真实模块状态记录，不强造硬件 healthy。`/health` 保留默认非外部 probe 路径。 |
| 首轮持久状态 | 注册唯一隔离管理员；记录 account ID、Companion shadow 政策及 revision、两端身份摘要、Embedded device ID/未绑定状态；记录配置首轮稳定版本；按下述单一温度 key 做非政策设置保存 | token、密码、私钥不写原始证据；只保存必要摘要和布尔断言。注册是持久写入，第二轮 login 才能证明读取既有用户；noop 不消费温度。 |
| 第一次正常关闭 | 向原 Server PID 发一次 SIGTERM，wait 原进程 exit 0，再同样关闭 Embedded；三个 listener 不再接受连接；PG 客户端归零；关闭后 SQLite 可只读核对完整性与 schema | 不删除 DB、配置、密钥或 SQLite，不先清理后伪造资源零值。关闭超时／非零退出是失败；兜底清理单独记录。 |
| 第二次启动与恢复 | 同一配置路径、DB/schema、身份文件和 SQLite 路径，不重新注册；login 得到同 account ID；shadow 政策和 revision 相同；密钥摘要、device ID 相同；温度 key 从 settings.toml 恢复；协议就绪与匿名拒绝仍成立 | 必须是新 PID 的实际启动，保留首轮状态；不复制初始目录替换已用状态。未绑定是默认态，不能因此声称验证过绑定／handoff 状态恢复。 |
| 第二次正常关闭与最终回收 | 第二轮两端也 exit 0、原 PID 已 wait；listener关闭、PG客户端归零、SQLite只读可开；保留证据后按本任务 exact ID/label 回收容器、卷和网络 | 任务进程仍存活、强杀、非零退出或资源残留不能以“后续容器删掉了”改判通过。 |

本轮可沿用原低成本权限与契约断言：`GetBindingStatus` 允许公开读取且始终未绑定；匿名 `Playback/ObserveControl` 返回 gRPC 16；已删除 legacy RPC 返回 gRPC 12。它们只验证入口与拒绝边界，不发送播放、动作、配对或退出后重放命令。若 Root 收窄重复的 legacy 断言，应如实记录未重复，不引入新控制命令补足“覆盖”。

### 配置身份恢复必须区分首轮合法写入

Root 已选定真实当前 `config/embedded.staging.example.toml` 的 `identity.device_id=""` 路径。Embedded main 会先生成／读取 key，再派生 device ID，并保存 runtime 配置；这会合法改变首轮配置文本。记录原输入摘要及首轮就绪后已保存的摘要，核对生成 ID、配置相对路径及模块 noop/禁用硬件状态，然后要求第二轮保持首轮稳定版本和身份。不能错误要求输入配置从第一字节到第二轮始终不变。

Oct06 script 则显式固定 device ID，因此它没有覆盖生成 ID 的保存路径；选择这一模式可以断言配置摘要从最初一直不变，但不能声称验证了配置自动保存。Root 应在执行前选定一种，不能失败后更换模式。

仅有 `embedded-state.sqlite3` 文件存在不能证明业务状态恢复。Root 已选择在两轮正常关闭后用已有 Python sqlite3 的只读 URI 检查 `PRAGMA integrity_check`、关键表存在及同一路径，不新增 SQL marker、绑定或音频设置写入。当前流程尚未产生持久 binding_state 时，第二轮 unbound 的正确含义是“未发生意外绑定”，不是“已有绑定恢复成功”。

### 唯一 Server 设置保存／恢复见证

建议固定 `core.profiles.llm.major.temperature`，把初始值改为 **0.25**；开始前 GET 必须确认该 key 存在、schema 为 `number` 且 `requires_restart=false`，并确认初值不是0.25。最小 Server 配置将 `llm.roles` 选为 `noop` 时，默认 `ProfilesSection` 仍会注册 major chat profile；不需要新增 profile 或启用 provider。该 key 不属于 `context_policy::owns`，也不是 embedding profile 的 model／dimensions／endpoint，不触发政策提交或模型调用。

1. 管理员 `GET /api/v1/settings?prefix=core.profiles.llm.major.temperature`，返回以完整 key 为索引的对象，值为 `{"value": ..., "schema": {...}}`。记录原 ETag（带引号的 context policy 数字版本），仅保存此条非敏感值与必要 schema。
2. 同一管理员 `PATCH /api/v1/settings`，body 仅 `{"core.profiles.llm.major.temperature": 0.25}`，可带刚读取的 ETag 作为 `If-Match`。断言 HTTP200、`success=true`、`errors=[]`、`changed` 包含该 key、`requires_restart` 不包含该 key、`context_policy_version` 等于原 ETag 数字。
3. NoopLlm 使用默认空 `apply_settings`，因此必须诚实断言该 key **不在 `applied`、在 `ignored_runtime`**；所有 module report 没有 error。这里成功表示注册表目标值已保存，不能表述为 noop 温度已经实际作用于模型。
4. PATCH 后立即按同一 prefix GET，value 为0.25、ETag 不变。首轮正常关闭后只读解析同目录 `settings.toml` 中这一 key，核对文件摘要／0600权限；文件包含其它非政策敏感设置，不能完整归档。
5. 同目录第二轮普通启动并登录后，再 GET 确认 value0.25、ETag不变，Companion shadow 政策及 revision 仍与首轮一致；无需再次 PATCH。这样区分持久重载与第二轮重新写入。

如果实际 staging 配置不再包含默认 major profile，应在运行前修改设计或明确配置该既有 profile，不能靠 PATCH 未注册的任意 key 返回成功来冒充现成参数持久性。任一失败先保留原件，不能通过多次 PATCH 重试覆盖本次结果。

## 原期限与 runner 必补条件

保留 Oct06 外部观察期限：每次既定 readiness 窗口 **30 秒**，HTTP 单次 **7 秒**，gRPC curl **5 秒**，正常进程停止 wait **10 秒**。单次 HTTP/gRPC 超时需受剩余 readiness 时间限制，避免最后一轮观察越过原总窗口；不因为机器慢或失败而扩大值。

产品 grace/drain 不是外部 runner 期限。旧 smoke 使用 **3000/100 ms**，本轮选择的当前 Embedded staging 模板使用 **5000/1000 ms**；Server 侧运行值也须在开始前锁定并记录。不能在失败后放宽，也不能以外部10秒 wait 宣称产品自身 grace 是10秒。无需修改生产默认值或库 deadline。

旧脚本的几个补证点：

1. `/health` 成功后立即单次断言 channel online，可能撞到现有异步连接过程。用显式 ready 条件，在原 Server readiness 预算内等待目标 channel 的确定状态；不要另加无限重试或每个条件重新刷新期限。发现意外身份或权限返回时保留原响应，不作为临时错误吞掉。
2. 原脚本只记录初始配置 SHA，没有在关闭／重启后核对；按上面的首轮合法写入规则补齐。密钥只记录摘要，不归档原文。
3. 旧 `finally` 中 terminate/wait 也可能抛错，从而跳过 summary。每个 owner 独立 try/finally，保存原业务失败、进程状态、清理错误和强杀次数，确保 summary 最终落盘；清理成功不能覆盖原失败。强杀只用于本轮 owned process 的失败兜底，不是预定故障注入。
4. 端口先预留后释放再 bind 存在通常的竞争窗口。必须观察原 PID 和实际协议响应；若真实撞端口，保存该次失败，不能换端口重跑求绿。进程退出后的连接拒绝只接受明确关闭结果，不能把任意客户端异常一概称为 listener 已关闭。
5. 原停止循环的 `proc.wait` 要始终保留原 Popen 所有权；记录具体 PID／返回码／耗时，再检查 listener、PG 和 SQLite。不得只等 log 关键字或容器进程列表便认为关闭已 join。

原有 libtest 默认并发、CASE permits 与期限保持不变。上一轮 workspace 1285/0/5、确认缺口的定向回归与两轮进程演练分别记账，不相加成测试通过数量；本分工不自行运行 Cargo。

## 首轮失败与确定性 sensor 回归

`run/validation-20261017-release-lifecycle/evidence/round-1.json` 和 `server-1.log` 已保留真实首次结果：两端实际就绪，noop 健康、未绑定及权限拒绝正常，纯非政策温度设置已保存；Server 收到 SIGTERM 后 **exit 1**，错误链为 `embedded sensor sync: background task exited before cancellation`。Embedded 兜底 SIGTERM 为 exit 0，不能抵销 Server 失败；本次不能记为首轮正常关闭通过。父任务报告关闭后同 PG schema／client 均为 0，最终资源与后续轮次仍需单独核对。

确认原因：真实 sensor loop 内部原先使用 `self.discovery_cancel.child_token()`，外层 `BackgroundStart::spawn_loop` 使用 owner 本地 token。正常 stop 先取消 discovery；在 parent bridge 或 owner stop 尚未同步取消本地 token 的间隔，sensor loop 会先成功返回，wrapper 按现有正确保护将其判为未取消的提前退出。fixture 分工已将这个内部 token 与 `background.token()` 对齐；discovery／connection 的 `spawn_owned` 保持不变。

新增测试仅在 `packages/alia-server/src/channels/embedded/channel_lifecycle_tests.rs`，`channel.rs` 只增加测试模块接线：

- `sensor_sync_waits_for_background_owner_cancellation`：暂停时钟、当前线程，使用仍有效的独立 parent／owner，先取消 discovery，再启动真实 sensor loop；registry/client 为 None，不发 RPC。commit 后的 1ms 虚拟 sleep 是 runtime 空闲屏障，小于既有 30s interval；首 tick 即便先到也不产生 IO，旧实现随后必退出，新实现继续由 owner 持有。记录 owner 和 job Arc 状态后先两次 stop/join，确认资源回收，再断言无假提前退出。
- `actual_loop_early_return_stays_a_shutdown_failure`：同一空闲屏障让真实成功返回的短 loop 完成，再取消／join owner，验证未取消的提前退出仍报具名错误，且重复 stop 保留同一失败。

这不是 wall-clock sleep、固定 yield 次数或重复运行求竞态；未改变生产期限或测试默认并发。父任务统一运行 `cargo test -p alia-server --lib channels::embedded::channel::lifecycle_tests:: --offline --locked -- --nocapture`，结果 **1 pass／1 fail／0 ignored／384 filtered**，目标体 **0.01s**、命令 **58.45s**、exit101、`source_drift=[]`。原件为 `run/validation-20261017-release-lifecycle/sensor-owner-red.log` 和 `.json`，源码清单 SHA-256 `0714c79ef8e59eb5a22ace87b883206090c531c841307dbacd37887900cea311`。

独立复核打印证据为 `owner_active=true`、`sensor_job_retained=false`、`resources_released=true`；两次 stop 都返回同一 `embedded sensor sync: background task exited before cancellation`。原句柄已 join、资源已回收后，第一项精确失败于不得误报提前退出的断言；真实提前退出对照通过。因此 RED 证明确认缺口，不只是超时或夹具失败。本分工仍未运行测试，测试源码冻结不再修改。

V2 runner 静审：`v2/evidence/lifecycle.py` 正确将温度0.25作为 desired setting，明确要求 `ignored_runtime` 包含该 key、`applied` 不包含；GET、原子保存文件、0600、重启读取和 ETag／shadow 政策不变构成持久恢复证据，不能描述为 noop 模型温度实际生效。prepare 为新私有目录并使用真实空 device ID 模板；round2 在同目录／DB／身份上登录，校验第一轮成功与 PG client 归零，要求配置、身份、SQLite路径及schema、settings文件均与首轮一致。关闭后才读取SQLite，未执行绑定或物理动作。

产物审计已动态收口：`v2/evidence/release-audit.py` 要求 Server 摘要不同于旧缓存并检查 ELF／loader；最终 `v2/run-lifecycle-stage.py` 在每个阶段前后将独立 `/artifacts` 副本与 `inputs.json` 核对，round2 也按进程名称比对首轮 binary SHA。独立读取复核确认缓存构建产物、保留副本、release-audit、inputs 和两轮实际启动的 SHA／字节数一致，详见下方身份表。四个 runner 文件也与 `harness-manifest.json` 一致。

- [x] 原实际失败与资源状态单独保留。
- [x] 原因经生产／审查分工独立确认，最小两项回归落盘。
- [x] 旧实现确定性 RED 与资源收口（1 pass／1 fail，原句柄回收后精确失败）。
- [x] 一行生产修复后的 GREEN、必要完整回归及新 release 产物身份。
- [x] 使用修订产物完成两次正常生命周期，保留原失败而不覆盖。

## 最终动态证据与计数

本节原件均在 `run/validation-20261017-release-lifecycle/`；Linux V2 子目录为 `v2/evidence/`。本分工只读复核结果，没有再次运行 Cargo、服务或数据库。

| 验证 | 实际结果 | 范围 |
|---|---|---|
| Mac sensor GREEN | 2 pass／0 fail／0 ignored／384 filtered，体0.01s，命令23.12s | `sensor-owner-green.log/json`；owner、job retained、resources released均true，两次stop均Ok，source drift为空。2项也包含于Linux全量，不相加。 |
| Linux default workspace | 1287 pass／0 fail／5既有ignored／0 filtered，405.963s，exit0 | 一次 `cargo test --workspace --offline --locked`；67 Running＋5 Doc-tests；server lib383、server_runtime85、duplex31、server_background13均无失败。 |
| Server no-default | exit0，4.345s | `cargo check -p alia-server --no-default-features --all-targets --offline --locked`，编译检查而非运行测试。 |
| Embedded no-default | exit0，1.465s | 对Embedded单独同等check，未通过workspace feature合并冒充。 |
| 两端default release | exit0，113.179s | 同次构建Server和Embedded。 |
| ELF／loader／链接 | passed，exit0，0.431s | Linux ARM64、解释器、无缺失动态库、Sherpa定义符号及Embedded help；不是硬件能力验证。 |
| prepare／Embedded check-config | exit0，0.272s／0.201s | 全noop模板、私有目录、空device ID；check-config无设备副作用。 |
| V2 round1／round2 | 两轮passed、命令0.791s／0.627s | 普通启动与SIGTERM；第二轮相同目录、配置、数据库和身份，不重新注册或PATCH。 |

678个冻结源码输入的清单SHA为 `0133b6f493b3c8b39b365abe03e3cfc035d88b0f21962e0ed6abd745796ed8c7`，所有阶段 `source_drift=[]`。没有修改libtest默认并发、CASE permits、产品期限或忽略新测试。5项ignored仍为3项真实PTY及2项真实Easy-Turn输入／服务场景；本轮未重复该范围。

原 `workspace-default.log` SHA为 `304a66e66027fb47df88960c31f4657fb3a92dd24ce493e2bd8e30a8345ab08b`。stdout／stderr在896–904行出现一个确定分块：899行只有Cargo的 `Running` 前缀，前一个main的0-test摘要在900行，appraisal目标后半在902行。既有计数器因此把appraisal误归前main，原 `workspace-counts.json/.txt` 保留 **INVALID_OR_INCOMPLETE_PARSE**／66 Running，不能作为完成计数。

`reconstruct-workspace-heading.py` 仅接受这个原SHA和唯一精确形态，将899前缀移到902前并去掉前缀LF，其余字节按原行复制；`reconstruction.json` 给出完整行映射、5项接受／拒绝小自测和输入前后SHA。既有计数器未改，解析明确标注派生的 `workspace-default.derived.log` 得到 `workspace-counts-confirmed.json/.txt`：**PASSED_RECORDED_RUN，67 Running＋5 Doc-tests，1287/0/5/0filtered**。原main0和appraisal20分别归属正确，原log／INVALID报告／sidecar／counter前后未变。此操作只整理既有证据，没有重跑Cargo求绿，也不声称一般日志解析框架已修复。

| 独立保留产物 | 字节数 | SHA-256 |
|---|---:|---|
| `v2/artifacts/alia-server` | 46,092,208 | `0374aa62602a3e6c2908459b7e8a4a574bd937071d182dbb18172eccb0695616` |
| `v2/artifacts/alia-embedded` | 49,540,200 | `159becdbd3783b77ad308d02096ed39158fc54cfef43f648091a2c0f015f4003` |

Server产物相对首轮旧缓存已变化；Embedded没有源码修改，产物SHA保持原值。产物身份链已独立逐项核对，不能仅因Embedded未变化认定使用了旧Server。15份Sherpa静态库输入前后均匹配，`native-inputs-after.json` 无错误。

round1的Embedded／Server PID为9539／9562，round2为9594／9617；四个原进程均正常SIGTERM并wait得到exit0，无forced kill、无失败兜底。Server关闭分别0.007289s／0.007970s，Embedded分别0.005775s／0.007013s，未扩大原grace或外部wait。两轮三端口均明确拒绝连接；round1结束后13.373s、round2结束后0.098s同任务PG均0非系统schema／0其他client，容器只剩holder `sleep infinity`。这些是实际采集时点，不表述为停止瞬间已测为零。

空device ID在首轮合法生成，第二轮复用相同device ID和密钥摘要。两轮相同account ID、shadow revision1、context ETag1，温度desired值0.25及0600 settings文件摘要均一致；`ignored_runtime`真实保留，未证明noop模型消费该值。SQLite两次只读integrity均ok，路径和关键schema一致；文件大小4096→24576不作为错误，未声称数据库字节不变或已有绑定恢复。未绑定状态、匿名Playback拒绝16、4条已删除legacy RPC拒绝12均通过。

`cleanup.json` 为 `complete=true`：16条受控清理命令均exit0；回收前数据库再次0／0，exact ID及本轮label匹配后删除两任务容器、一个任务匿名卷和内部网络，原有其它容器未动。最终保留证据和独立产物；没有用删容器后的空状态替代运行后资源取证。

## 明确不扩张的恢复场景

本轮不做 SIGKILL／崩溃／断电注入、磁盘满、配置损坏、目录同步失败、数据库断连、迁移中断、端口冲突注入、startup 随机时机取消、native 阻塞、精确取消竞态或长期压力。相关已测生命周期库契约和 Settings 恢复语义继续有效，但不把正常 shutdown/restart 描述成上述事故恢复通过。

除上述唯一纯非政策温度设置外，不创建 policy PATCH、执行批准、绑定／handoff、动画、提示音上传或模型请求；不启用 dev.skip_binding、真实录放音、执行器或R1 AEC。Server通道既有mDNS browse仅在本任务隔离内部网络内运行，未访问用户硬件发现网络。R03 保持暂停；本轮软件 staging 可补齐 R05 前的软件启动依据，但不能据此增加实机验收完成数。

## 动态交接 Checklist

- [x] 两个固定 V2 release artifact 与本轮配置、源码／基线身份已锁定并独立核对。
- [x] V2第一轮实际协议就绪、持久记录及正常关闭断言通过；V1失败原件保留。
- [x] 同目录／DB／身份第二轮恢复和正常关闭断言通过。
- [x] 无强杀、无孤儿进程／listener／任务DB连接；原失败与兜底明确分开，exact ID回收complete。
- [x] 本分工完整HANDOFF、动态数字和派生计数限制已核对冻结；统一归档、其它checklist／检查点及提交交Root完成。

本分工未运行 Docker、Cargo、数据库、服务、模型或设备，未修改生产行为或生命周期runner脚本；只添加回归测试、测试模块接线和ignored目录中的单次证据重组脚本。最终动态原件已独立核对，本文冻结交Root收口。

未测边界保持：真实录放音／R1硬件AEC／PCA9685及舵机／真实模型服务／绑定恢复、断电崩溃与磁盘故障均未由本轮noop演练证明；R03继续暂停，R02硬件AEC优先路线不变。普通进程恢复证据不替代未来Mac＋Pi实机R05验收。

## Root最终归档补记

受测源码已正常提交 `6c62c84ee60e3a5fd387f2172ead432031907599`，678项Git blob匹配；[证据索引](evidence-index.json)包含148份、1541367字节，原失败与派生计数边界均保留。统一checklist、恢复检查点与最终结果见[Root HANDOFF](root-HANDOFF.md)。
