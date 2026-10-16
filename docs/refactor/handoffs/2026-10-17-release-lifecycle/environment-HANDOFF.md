# Release lifecycle：noop staging 环境 HANDOFF

日期：2026-10-17。负责人：`v01_fixture`。基线 `1c63218`，应用源码 `4e2968f18d765f326e76d24a3bcdf5bba9034efc`。本 agent 审计配置、启动路径和既有脚本；在 Root 保存确定性 RED 并授权后，仅修改 sensor sync 的取消 token 来源。未运行 Cargo、Docker、进程、数据库或硬件。采用既有 IoT 工作流程的模拟／实机证据边界和 Ponytail 最小完整审计原则。

## Checklist

- [x] 读取 `config/embedded.staging.example.toml`，沿 factory、registry、motion、vision、discovery 路径确认禁用条件。
- [x] 审查 Server 示例和 Oct06 release smoke，区分 noop provider 与政策设置，确定本轮最小配置。
- [x] 确认未 provisioned／未绑定设备的公开状态读取和 Server 管理员 device/status 路径。
- [x] 核对环境覆盖、配置 fragments／settings、插件与模型前置入口，给出隔离要求。
- [x] 核对默认 shadow、默认 Memory／cache 政策与 noop health 的预期，报告已有内部 discovery 行为。
- [x] 已核对首轮失败、确定性 RED／GREEN、Linux V2 冻结环境、完整 workspace／release、两轮实际恢复及精确资源回收原件。本 agent 交接 6/6；本轮 noop staging 软件生命周期验收通过，源码提交身份由 Root 统一记录。

## 可复用产物与环境

首轮复用上一轮独立保留的 Linux ARM64 release，未重新编译或引入依赖。以下是原失败轮的准确产物身份；修复后的 V2 必须从冻结源码重建 release，不能将旧 Server 产物当成修复版本：

| 产物 | bytes | SHA-256 |
| --- | --- | --- |
| `run/validation-20261017-linux-release/artifacts/alia-server` | 46092832 | `3528c5e02708e126d560cdaade8a431c0ed3e60de6c86892f931dcb0250de330` |
| `run/validation-20261017-linux-release/artifacts/alia-embedded` | 49540200 | `159becdbd3783b77ad308d02096ed39158fc54cfef43f648091a2c0f015f4003` |

Linux image 固定 ID 为 `sha256:6cd7a7a06976f8651cdb57fcde457e9dc7e2abe20587900dd750e1f373a25c70`，PG image 固定 ID 为 `sha256:306e00ca129081f4a69081c09ff0bdf0ef5169779019200a5c22ce9e0c6148f3`。继续 `--pull=never`、既定 `linux/arm64`；Root 新建任务专属 internal network、PG、holder，保留精确 ID／label，沿用上一轮系统库。上一轮任务容器与 network 已删除，不能把旧 container 名称当成可运行环境。没有 host network、宿主端口发布、设备透传、宿主音频／D-Bus／串口／I²C socket 挂载，也不挂已有个人配置和身份。

每个运行实例使用新的私有配置／状态目录，配置路径绝对指定；生成的 key、SQLite、settings、日志均在本轮目录，恢复轮次重用本轮同一目录和同一任务 PG。独立 runtime 目录应为 0700，含临时凭据的配置为 0600。不要把密钥原文写入 evidence，比较 hash 即可。

## Embedded 模板与实际启动边界

建议直接复制 `config/embedded.staging.example.toml` 到任务私有目录，只把 `device_runtime.listen_addr="127.0.0.1:0"` 换为本轮选定 loopback 端口；保留其余配置，包括：

- `runtime.provisioned=false`、空 `identity.device_id`、`dev.skip_binding=false`、`discovery.require_physical_confirm=true`。
- 七个实现槽全部 `noop`：wakeword、vad、direct_response、audio_capture、audio_playback、face_tracker、discovery。
- motion.enabled／hardware_enabled／calibration_confirmed 均 false，approved_animations 为空；vision、vision_debug、hmmd_mmwave 均 disabled；mDNS advertisement 与 BLE provisioning 均 disabled。
- 原模板 shutdown_grace_ms=5000、grpc_drain_timeout_ms=1000；不为本轮验收提高它们。
- `staging/state/embedded.sqlite3`、`staging/identity/device.key`、`staging/plugins`、`staging/cues` 均相对于新配置文件所在目录。保持插件目录为空，不复制真实插件／动作／模型。

已核对的实际路径：

1. `packages/alia-embedded/src/main.rs:54` 读取显式配置；首次空 device_id 根据新 keypair 公钥推导身份，随后 `save_runtime_config` 写回。相对路径在 `config/mod.rs:24`／`:81` 从配置目录解析。首次启动的配置字节必然可能改变，重启比较应以第一次身份写回后的文件／ID／key hash 为基线。
2. `module/factory.rs:24` 起的各 `noop` 分支直接构造 Noop 实现，不构造 CPAL／ALSA、Sherpa KWS／VAD 或 discovery backend。`module/noop.rs:106` 的 Noop capture `start_capture` 无设备动作、`is_available=false`；`module/registry.rs:78` 因此不启动 speech pipeline。
3. motion 并非可选 registry 槽，但 `module/motion/runtime.rs:477` 在 enabled=false 时返回 disabled runtime，强制 hardware_enabled=false；`module/motion/servo.rs:123` 选择 mock backend，无 I²C／串口初始化和启动归中。vision_debug.enabled=false 在 `module/vision_debug.rs:204` 直接返回，不监听调试 web；HMMD 仅在 enabled=true 时创建（`module/builder.rs:398`）。
4. 核心启动仍会绑定任务 loopback gRPC listener、创建 key／SQLite、扫描私有插件目录，并维护生命周期所有者。`plugin/registry.rs:66` 扫描仅加载 manifests 到 Installed，不会因此自动执行插件；保持空目录使此路径无插件输入。

必须清除 `ALIA_EMBEDDED_CHECK_KWS_WAV`：`main.rs:62` 在普通 noop factory 之前检查这个环境变量，若设置就会直接读取 WAV 并创建 Sherpa KWS。这是已存在的专用诊断入口，不是 staging 模板失效；本轮通过受控子进程环境排除，不改变生产行为。

## Server 最小配置与政策

仓库没有独立 Server noop staging TOML。`config/server.example.toml` 当前选择真实 STT／TTS／LLM role 且 discovery_enabled=true，不可整份复制。复用 Oct06 `run/validation-20261006-cleanup/linux-arm64-v3/evidence/release-smoke.py` 的最小 server 配置结构，调整为本轮独立路径／端口／数据库／随机 JWT，保留以下值：

| 配置 | 本轮要求 |
| --- | --- |
| `[server]` | 新实例 ID，host=`127.0.0.1`，HTTP／gRPC 各用本轮端口，discovery_enabled=false；grace／drain 使用已审核既有值，不提高期限 |
| `[database]` | 仅本轮 PG／任务 schema，max_connections=4、min_connections=0 可沿用旧 smoke；不继承宿主 DATABASE_URL |
| `[auth]` | 新随机临时 JWT、私有相对 server_keypair_path；原 Argon2 参数 19456／2／1，正常账号权限和认证不变 |
| `[[channels]]` | 仅一个 enabled Embedded channel，endpoint 固定为本轮 loopback gRPC；不建 CLI，不设置 owner、设备 pin 或信任动画。可沿用模板 speech.channel_id=`pi-living-room` 作为 channel id |
| `[modules]` | stt=`noop`，tts=`noop` |
| `[llm.roles]` | minor／major／default／embedding 全部=`noop`；memory_capture／sentence_expression 省略或显式 noop |
| `[providers.easy_turn]` | enabled=false，符合当前默认值，防止本轮无意启动模型请求 |
| `[plugins.home_assistant]` | enabled=false，符合当前默认值 |
| `[policies]` | 使用当前默认政策，不复制旧 smoke 的 memory.enabled=false，不修改工具可见性、执行批准或 Companion 政策 |

`bootstrap/factory.rs` 的 noop 特殊分支在查外部 provider profile 之前返回 Noop 实现；默认 Easy-Turn enabled=false 时 `create_turn_detector` 返回 None。Home Assistant 仅在显式 enabled 时构造 extension。新的私有 Server 目录不携带 `server.d/**/*.toml` 或旧 `settings.toml`：`config.rs:2245` 会排序合并与基文件同名 `.d` fragments，`bootstrap/builder.rs:310` 会加载配置目录中的 persisted settings，二者都可能覆盖预期。

启动子进程环境至少固定 `ALIA_CONFIG_PATH`／`ALIA_EMBEDDED_CONFIG_PATH` 绝对路径，并移除 `DATABASE_URL`、`ALIA_JWT_SECRET`、`OPENAI_API_KEY`、`ANTHROPIC_API_KEY`、`GEMINI_API_KEY` 和 `ALIA_EMBEDDED_CHECK_KWS_WAV`。`config.rs:2108` 证明前五项可覆盖配置字段。保留普通日志配置，不能依赖宿主环境碰巧为空。Server 配置必须在启动前存在，避免 `oobe/bootstrap.rs:133` 转入 setup；setup 默认可能启用独立发现服务，不属于本轮正常 runtime 启动目标。

## 未绑定读取、默认 health 与 discovery 的准确含义

未 provisioned 与未绑定不阻止这次只读连接验收。`runtime.provisioned=false` 映射 provisioning_mode，进入状态元数据；不会替代 dev.skip_binding 或授权条件。

- `packages/alia-embedded/src/grpc/binding_service.rs:119` 的 `GetBindingStatus` 不调用 require_auth，允许读取新生成 device_id／public key；应断言 is_bound=false、bound_server_id 为空，并比较两轮相同身份，不能据此建立设备 pin 或绑定。
- `packages/alia-server/src/api/devices.rs:114` 的 device/status 调用上述 binding_status 作在线探测，不要求设备已绑定。新任务注册得到管理员后可读取无 owner channel；仍需正常 HTTP 身份，普通用户不得凭无 owner channel 越权。预期 online=true、device_id 匹配，cpu_percent／memory_percent／temperature_c 全为 null。
- Noop provider 的 ModuleLifecycle health 默认 Healthy。默认 Memory 保持 enabled：`modules/memory/service.rs:1754` 以 PostgreSQL persistence 和无 last_error 判定 healthy；NoopEmbedding 无 verified profile 会保留 keyword retrieval，不能授权真实 semantic vector 身份，但不要求关闭 Memory 政策。默认 disabled Insights health 也是 Healthy。新任务 PG／index 初始化正常时 `/health` 预期 HTTP 200、status=ok、各注册模块 healthy；记录实际完整 body，不能将任意 degraded／unhealthy 都视作成功。默认 probe=false、整体 6 秒与并发 8 的实现不修改。

已报告 Root 的确认行为：`server.discovery_enabled=false` 仅禁 Server advertisement。`channels/embedded/channel.rs:902` 的 start 仍无条件启动 `spawn_discovery_loop`，其 `:322` 每 15 秒调用 mDNS browse，即使配置了固定 loopback endpoint；当前没有独立 channel discovery_enabled 开关。固定 endpoint 只保证 `resolve_endpoint` 不依赖发现结果。保持启用 channel 才能检验真实 Server→Embedded device/status 链路，因此最小方案是任务 internal network 隔离并如实记录内部 discovery；不能宣称“所有 discovery 均未启动”。本轮不为这项已知行为扩张生产配置框架、关闭必要 channel 或访问真实 LAN 设备。

## 交给 Root 的有界验收建议

沿用旧 smoke 的 readiness 上界 30 秒、单进程 SIGTERM 等待 10 秒和 HTTP／gRPC 原请求期限，首先保留首次失败现场。真实进程应使用上一轮保留产物的 absolute path 和本轮 private CWD；stdin=DEVNULL，输出独立日志。旧脚本硬编码 Oct06 container 内路径／schema，不能原样执行。

第一轮核对身份自动写回、SQLite／server key 建立、管理员注册、health、shadow revision、未绑定状态、拒绝匿名控制及已删除旧 RPC；第二轮使用同一私有目录／PG 恢复并核对账号登录、key／device_id／政策持久性、listener 收尾。只发送状态查询和已授权的本轮软件管理请求；不配对、不下发 Body／Playback、不创建外部任务、插件、skills 或模型输入。正常停止应由子进程真实退出和监听关闭证明，不能以发送 SIGTERM 或取消 token 当完成。失败也必须等本轮子进程回收，再关闭任务数据库与容器。

## 首轮真实进程失败与生产修复边界

已只读核对 `run/validation-20261017-release-lifecycle/evidence/round-1.json`、`server-1.log` 和 `after-round-1-resources.json`。首轮 `/health` 返回 ok、15 个模块均 healthy，settings 保存和未绑定协议检查均成功；Server 收到 SIGTERM 后在 0.007628 秒退出，exit code=1，错误为 `embedded sensor sync: background task exited before cancellation`。此失败是正常关闭结果错误，不是 readiness 超时，也不是尚未绑定导致授权放宽。原件保留，不能把该轮计为通过。finally 等到 Embedded 正常退出 0，没有强杀；约 12.313 秒后的同 PG 快照是 0 schemas／0 other clients，holder 内只剩 `sleep infinity`，这仅证明失败轮资源收尾，不替代成功关闭验收。

根因位于 `packages/alia-server/src/channels/embedded/channel.rs` 的 `spawn_sensor_sync_loop`：内部循环观察 `self.discovery_cancel.child_token()`，而 `BackgroundStart::spawn_loop` 的外层 supervisor 观察 `OwnedBackground` 的独立 token。`modules/platform/background.rs::begin_start` 通过独立任务异步桥接 parent cancellation；parent 已取消、桥接任务尚未运行时，sensor job 可以先返回 Ok，外层 token 仍未取消，因而触发真实的提前退出保护。两层取消信号的时序不一致即可解释实际日志；没有证据要求改变 sensor 权限、外部 RPC 行为或 shutdown 期限。

Root 已保存并由本 agent 只读复核 `run/validation-20261017-release-lifecycle/sensor-owner-red.log`／`.json`：1 passed／1 failed／0 ignored／384 filtered，目标用时 0.01 秒，source_drift=[]。失败项 `sensor_sync_waits_for_background_owner_cancellation` 精确记录 owner_active=true、sensor_job_retained=false、resources_released=true，首次及重复 stop 均返回原 `background task exited before cancellation`，之后才由独立断言报错；另一项通过。此 RED 不依赖真实音频、硬件或外部数据库。

RED 保存后按 Root 授权，唯一生产变化是 `spawn_sensor_sync_loop` 的 cancel 改为 `background.token()`，与监督包装共享取消事实；保留 `spawn_loop` 对在途异步 endpoint 等待的取消、原提前正常退出保护和 Err／panic 报告。discovery／connection 的 `spawn_owned` 涉及自身清理，保持原实现。源码已冻结交 Root 统一 GREEN 与实际 release 生命周期复验，本 agent 未运行测试或提交，也未延长期限、降低并发或更改政策。

## 定向 GREEN 与 Linux V2 环境核对

已只读复核 `sensor-owner-green.log`／`.json`：相同两项用例 2 passed／0 failed／0 ignored／384 filtered，命令 23.12 秒、目标 0.01 秒，source_drift=[]。取消回归记录 owner_active=true、sensor_job_retained=true、resources_released=true，首次及重复 stop 均 Ok；`actual_loop_early_return_stays_a_shutdown_failure` 同时通过，表明真实提前退出仍是失败。上述 RED 和 GREEN 均保留原始记录，不以本轮成功覆盖失败证据。

已读取 `run/validation-20261017-release-lifecycle/v2/setup.py`、`evidence/setup.json`、`native-inputs-before.json` 和 `environment.log`，并独立只读核对：

- `v2/evidence/source-manifest.json` 与 GREEN manifest 字节一致，SHA-256=`0133b6f493b3c8b39b365abe03e3cfc035d88b0f21962e0ed6abd745796ed8c7`。678 项输入同时匹配当前工作区及 V2 source snapshot 的文件大小／SHA-256，差异 0；包括本轮一行修复及新增回归文件。
- 两个镜像沿用前述 exact SHA／linux arm64。新任务 label=`release-lifecycle-v2-20261017`，internal network ID=`cf5aaa9d206de97c29b3c63a2d733b60103955b8d995f647ae5ba25eef823c3b`，PG ID=`72b2eab8afdf24b3301494320c8ca38e12c3635e4ab5a395480d8ae762ee565d`，holder ID=`46cf51e178f98f24a8a4e9ddf60741fe00b3421a0f8a7eeaaa1c7a9db28d85ff`。没有 host port、设备或宿主网络挂载；holder 共享仅任务 PG 网络命名空间。
- source、native、artifacts 以只读方式挂载；cache 为上一轮任务 cache 的私有 APFS clone，registry／runtime／evidence 均位于本轮 V2 目录。旧历史 cache 和原件没有作为可写输入挂载。
- PG 在原 30 秒 readiness 上界内通过 `127.0.0.1` TCP `SELECT 1`，preflight 确认为任务数据库、PostgreSQL 17.10／aarch64。先前两次 connection refused 属启动预检的原始记录，之后才开始 workspace，不跳过失败测试。
- Sherpa 本地 archive SHA-256=`86d0d3818770d4f6a087e52691d3ae171393972030f133df064a271fe9fe1749`；本 agent 重新只读计算 archive 和全部 15 个静态库的大小／SHA-256，与保存输入记录逐项一致。没有下载、apt 或新依赖。
- environment 原件确认 Debian 12／aarch64、Rust 1.95.0、Cargo 1.95.0、protoc 3.21.12、ALSA 1.2.8、D-Bus 1.14.10、OpenSSL 3.0.20、glibc 2.36；9 项系统库 hash 已保存。构建 jobs=2，`RUST_TEST_THREADS` 未设置，DATABASE_URL 未覆盖，DOCS_RS／SHERPA_ONNX_LIB_DIR 未设置；真实 native linking 路径保留。

上述环境核对时 Root 已用该冻结输入启动一次完整 workspace 回归；最终结果已在下节独立核对，本 agent 没有操作验证环境或重跑测试。

## V2 最终交付与收尾（6/6）

`v2/evidence/workspace-default.result.json` 确认一次 `cargo test --workspace --offline --locked` exit 0、405.963 秒、678 输入前后无漂移。flow 的 `reconstruction.json` 及 `workspace-counts-confirmed.json` 明确原 stdout／stderr 有一处 Cargo 标题分块，只在派生视图内重组该精确区间，原日志与原计数文件 hash 前后不变，没有重跑测试；确认总数为 1287 passed／0 failed／5 既有 ignored／0 filtered，72 个 target（67 Running、5 Doc-tests）。原始计数解析问题与派生映射均保留，不把派生视图冒充原始命令日志。Server／Embedded no-default 检查分别 exit 0、4.345／1.465 秒；release build exit 0、113.179 秒，link audit exit 0、0.431 秒，均 source_drift=[]。

已读取 `release-audit.json` 并独立计算 `v2/artifacts` 中两份独立文件的大小／SHA-256，确认与实际两轮启动记录的 binary_sha256 相同，且不是 cache 文件的同一 inode：

| V2 产物 | bytes | SHA-256 |
| --- | --- | --- |
| `run/validation-20261017-release-lifecycle/v2/artifacts/alia-server` | 46092208 | `0374aa62602a3e6c2908459b7e8a4a574bd937071d182dbb18172eccb0695616` |
| `run/validation-20261017-release-lifecycle/v2/artifacts/alia-embedded` | 49540200 | `159becdbd3783b77ad308d02096ed39158fc54cfef43f648091a2c0f015f4003` |

两端均为 Linux AArch64 ELF PIE，loader 可解析；Server 6 项、Embedded 14 项 resolved library 身份保存于 audit。Embedded 的 Sherpa 定义符号与安全 `--help` 检查仍在；Embedded 内容未改，因此与上一轮产物 hash 相同，Server hash 已改变。`native-inputs-after.json` 确认原 archive 和全部 15 个静态库 errors=[]。

`v2/evidence/round-1.json` 与 `round-2.json` 均 passed=true：

- 两轮 `/health` 均 ok，15 个模块全 healthy。相同管理员账号可恢复登录；Companion policy 一直 revision=1、mode=shadow，context policy ETag 一直 `"1"`。
- 首次生成的 device_id=`alia-embedded-a177cba3ad73`、Server／Embedded key hash、持久化后的两端配置 hash，两轮运行时与停止后均一致。未绑定状态始终 true；`GetBindingStatus` 返回 gRPC 0，匿名 `ObserveControl` 返回 16，已删除旧 RPC 返回 12，未发生配对或控制下发。
- 非政策 setting `core.profiles.llm.major.temperature=0.25` 正常原子保存，重启读取同值；settings 文件两轮 SHA-256 相同，mode=0600。其首次报告 `ignored_runtime`，未误称 noop LLM 已动态应用。SQLite 两轮 integrity=ok、schema 相同；其文件大小由 4096 变为 24576，不宣称数据库文件字节不变。
- 两轮 Server／Embedded 均 SIGTERM exit 0，耗时分别为 0.007289／0.005775 秒、0.007970／0.007013 秒；finally 均 needed=false、forced_kill=false。HTTP／Server gRPC／Embedded gRPC 三个监听端口均确认关闭。
- `after-round-1-resources.json` 在轮次结果后约 13.373 秒、`after-round-2-resources.json` 在约 0.098 秒，均记录同任务 PG 0 schemas／0 other clients，holder 内仅剩 `sleep infinity`。第一轮与第二轮使用同一任务 PG 和私有状态目录，未通过重建数据规避恢复要求。

`v2/evidence/cleanup.json` 已 complete=true，16 条命令全 exit 0。清理前再次核对原任务 container/network 的完整 ID 与 label，PG 0/0；已移除两任务容器、任务 internal network 及 PG 匿名卷 `b4983e509ee838745dbd07c67a39b4d1b045d9206a49bae62ada3feb66f2fac2`，共享 astrbot 保留且仍是唯一运行容器。本 agent 仅只读核对原件并编辑本 HANDOFF，没有启动应用／验证进程、测试、Docker、数据库、真实录放音或硬件动作。

真实音频、AEC、模型服务、执行器、Pi 部署均不在此证据范围；R03 保持暂停。既有 V01 历史定位范围没有因本轮 process 失败而扩张。

## Root最终归档补记

受测源码已正常提交 `6c62c84ee60e3a5fd387f2172ead432031907599`，678项Git blob匹配；[证据索引](evidence-index.json)包含148份、1541367字节，原失败与派生计数边界均保留。统一checklist、恢复检查点与最终结果见[Root HANDOFF](root-HANDOFF.md)。
