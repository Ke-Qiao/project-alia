# Noop staging 真进程生命周期与恢复 — 独立审查

2026-10-17；基线 `1c63218`，修订前应用源码 `4e2968f18d765f326e76d24a3bcdf5bba9034efc`，最终源码提交 `6c62c84ee60e3a5fd387f2172ead432031907599`。当前 **5/5，最终 GO**。已确认并修复一个 sensor 取消归属缺口，V1 原失败保留；V2 在本轮限定的正常关闭／重启与期望设置恢复范围通过，提交与归档身份已独立核对。审查者只读源码、脚本和证据，不运行 Cargo／Docker／数据库／设备，不改生产或测试源码。

## Checklist

- [x] 核对最新 release 产物与旧进程验收的版本及范围。
- [x] 审查当前 main／启动／关闭和设置持久恢复路径。
- [x] 识别旧脚本假阳性、期限及失败清理风险，提出有限门槛。
- [x] 静审本轮实际脚本和最终原件，核对 V1 失败与 V2 正常退出／重启恢复的独立结果。
- [x] 核对源码／产物／配置身份、资源回收和归档，给最终 GO。

以下按审查阶段保留过程记录；各阶段的“待完成”状态已由末尾最终交付身份核对关闭。

## 基线与已有证明

上轮 [Linux 构建／测试／release 包](../2026-10-17-linux-release/README.md) 已由本审查者独立核对：同一 677 输入，Linux 默认 workspace 1285/0/5，双端 release 实际构建，ELF／动态库／Sherpa native 输入和任务清理通过。其两个独立保留产物为：

| 产物 | 字节数 | SHA-256 |
|---|---:|---|
| Server | 46,092,832 | `3528c5e02708e126d560cdaade8a431c0ed3e60de6c86892f931dcb0250de330` |
| Embedded | 49,540,200 | `159becdbd3783b77ad308d02096ed39158fc54cfef43f648091a2c0f015f4003` |

上轮只做链接和 Embedded 安全 help，没有普通服务启动。本轮应使用这两份已验产物并核对内容，不能把旧 cache 里的同名文件当作当前 release。

[10 月 6 日 release-smoke](../2026-10-06/evidence/linux/release-smoke.py) 对源码 `4434be4` 做了两次正常启动／SIGTERM／重启，验证账号登录、同私钥、shadow 政策、设备在线但未绑定、拒绝匿名控制和旧 RPC。它没有验证本轮源码、SIGKILL 后恢复、运行时 `settings.toml` 的重新读取或失败启动回收，不能覆盖这些新增出口。

## 当前实现与证据含义

Server main 正常创建 runtime、读取显式配置或进入 setup，然后调用 `ServerBuilder` 和 `ServerRuntime::run`；没有安全 help 分支。Embedded main 的 help／check-config 会在 runtime 前返回，正常入口则创建身份、加载 SQLite 和构建模块。仅 help 或 check-config 成功不能作为启动证明。

Server 监听任务将意外提前结束记为错误，并由运行时 cleanup 收口；Embedded 对服务结果与 cleanup 结果分别处理。两端 main 在库级关闭后另有一秒 runtime 回收边界；该最终超时不是“所有 native 线程已安全停止”的证明。全 noop 配置可以验证操作系统进程及网络／数据库生命周期，不能推导实际 native 硬件工作时的关闭行为。

当前 Embedded staging 模板的七个模块均 noop，motion／hardware／calibration／vision／mmwave／发现均关闭，`skip_binding=false`，本轮不得为快速通过而开启设备或跳过绑定。正常启动仍会写 key／SQLite 和监听，必须使用任务独立目录、数据库与端口。Server staging 应显式选择 noop LLM／STT／TTS／embedding、关闭 advertisement 与非本轮动作，保留 shadow 和现有政策。Server Embedded channel 自带 mDNS browse 并不由 advertisement 开关关闭；本轮保留该既有行为，在任务 internal network 中隔离，不修改生产开关，不声称“零网络发现”。

Server 启动 `load_persisted(settings.toml)` 后应用保存值；`PATCH /api/v1/settings` 则保留用户已确认的 SQL／模块／文件分阶段完成语义。HTTP 200 也可能 `success=false`，必须核对 JSON 的 `success`、`applied`、`module_reports`、`errors`。重启后 GET settings 同值首先证明期望设置重新读取；没有对应业务观测时，不宣称每个模块实际生效。release 的 `storage::checkpoint` 没有测试故障注入，普通进程终止不能被描述为已经覆盖任意 fsync／rename 中断、断电或部分成功自动回滚。

## 旧脚本中的确认缺口

1. `stop(proc)` 直接 send SIGTERM 后接受 exit 0，没有在发信号前要求进程仍存活。已经提前自行 exit 0 的进程可能被记录为“正常 SIGTERM 关闭”。本轮应在每阶段见证实际 PID 存活，并在发信号前检查 alive，保留所发信号、原 PID、等待结果和耗时；预期故障退出与正常退出分开。
2. `wait_ready` 的 30 秒循环使用固定七秒 HTTP 调用；最后一次 I/O 可跨越 30 秒后仍返回成功。应使用同一绝对期限，单次 I/O 不超过剩余预算，返回成功时也检查期限；不通过增加 deadline 解决。
3. TCP connect 只能证明有人监听，`GET /health` 的 HTTP 200 也不证明模块健康。当前 API 对 unhealthy／degraded 仍返回 200。应检查真实 gRPC 响应和任务设备身份，并按 noop 配置明确允许的模块状态／原因，拒绝未知 unhealthy、timeout、panic 或错误实例响应。disabled 状态若是配置预期，可记录为已禁用，不能写成外部模型在线。
4. 旧脚本仅证明私钥字节和账号一致，没有证明 runtime settings 或 Embedded 持久状态内容恢复；SQLite 文件存在也不是结构完整或状态一致的证明。应选择一个本轮已支持的纯非政策设置并验证保存回应、文件完整性及重启后值。检查实际持久状态时不要删除 WAL／恢复文件或重建状态来使验收变绿。
5. 旧 finally 清理逐进程执行，但任一 terminate／kill／wait 异常可打断后续清理和 summary 写出，kill 后 wait 也无独立上界。新脚本须尽力处理所有已启动进程，原失败与清理错误分别保留，始终输出可解释结果。强制兜底不能计为正常关闭；只统计保留的 Popen 对象也不足证明无其它任务进程，最终还需容器进程／监听／数据库观测。

## 本轮有限完整门槛

1. **身份与隔离。** 固定两份已验 release hash，保存源输入／配置身份，使用专属 internal network、任务 PG、私有文件目录和 loopback 监听；记录真正启动的 executable、PID、阶段和退出状态。配置与 auth 日志不泄露私钥、密码或 token，原始私有配置只留在受控任务目录。
2. **正常生命周期。** 一轮双端真实启动，原有有界期限内完成协议 readiness、账号与 shadow 政策核验；在 alive 见证后 SIGTERM，各进程 exit 0，确认监听关闭及连接释放。从同一目录重启，核对账号／身份／配置与实际选择的持久设置。账号注册或设置变更仅发生在任务实例。
3. **恢复范围固定。** Root 已明确本轮只沿用两轮正常启动→SIGTERM→同配置／DB 重启，另加一个经源码核对的纯非政策 noop 参数 PATCH、`settings.toml` 文件和第二轮 GET 同值见证。初审曾提出 SIGKILL／坏配置等可选后续场景，本轮不执行，也不把它们列为通过门槛。此次“恢复”仅指正常关闭后重启及期望设置持久恢复，不称 crash／断电恢复或真实模型参数效果。
4. **失败清理。** 任一断言失败先保留原日志和阶段结果，再尽力停止所有任务进程；不在失败后重置 DB／密钥／SQLite 或悄然改配置重跑。等待、协议调用、故障观察和 finally 清理均有明确上界，原测试期限及并发不改。关闭后先观测原 PG schema／连接、容器进程和端口，再按精确资源 ID＋label 关闭任务环境，保留共享服务；脚本创建的预期持久 schema 与泄漏 schema 分开说明。
5. **诚实出口。** 正常进程关闭、正常重启、设置恢复及资源回收分别报告；若失败路径需要补救 kill，不把补救计为 graceful 或通过。本轮不执行 SIGKILL／坏配置注入，不覆盖真实录放音／执行器／模型、Pi、断电、长稳或 R05 整机验收；V01 历史根因不因一次进程通过自动关闭，R03 保持暂停。不新增依赖或平台；仅在证实缺口后才由 Root 决定有界修复。

## 本轮候选脚本审查

已完整只读审查 `run/validation-20261017-release-lifecycle/evidence/lifecycle.py`、`run-stage.py` 和纯逻辑 `selftest.py`，结论为 **允许开始两轮动态验证的静态 GO**。这不等于动态验收已通过。

- readiness 使用同一绝对 30 秒期限，协议调用预算受剩余时间约束，成功返回后再次检查期限和进程存活；`runner-selftest.log/result.json` 为 6/6、exit 0、677 输入无漂移，覆盖晚成功、提前退出和就绪过程中退出的拒绝。`/health` 明确要求整体 ok 且各模块 healthy，设备绑定协议、设备身份、匿名控制拒绝和旧 RPC 状态另行核对。
- stop 在发 SIGTERM 前检查原 Popen 仍存活，记录原 PID／信号／耗时／exit code；只有 exit 0 可通过。异常清理逐进程保留错误，任何额外 terminate／kill 兜底均使本轮失败。三个端口仅接受明确 connection refused；任务环境进程与 PG 连接仍由 Root 在每轮后另行观测。round 2 除脚本的前轮 passed 和 PG 0 client 门，还需 Root 先确认 holder-only。
- `core.profiles.llm.major.temperature` 属当前未选用的 major profile，roles 使用 noop。脚本要求 PATCH 为 success、errors 为空、changed 与 ignored_runtime 同时包含该项，而 applied／requires_restart 不包含它；同时验证政策／ETag 不变。两轮 GET、0600 的 `settings.toml`、同 hash 及值 0.25 证明期望设置持久恢复，明确不证明真实模型效果。
- 账号、两端 key、设备身份、配置和 shadow 政策保持；Embedded SQLite 使用只读连接检查 integrity、两张既有表和同路径／同 schema。没有通过重建状态文件、放宽绑定或换库取得通过。
- runner 固定精确任务容器 ID，记录真实命令退出码，前后核对 677 输入及两份 binary hash，保留原日志；1800 秒容器 watchdog 和 1850 秒 host 最终边界不代替应用内部期限。若触发兜底，Root 仍须独立观测并精确回收任务环境，不能将 watchdog 退出当正常关闭。

`embedded-preflight.result.json` 已是 exit 0、无源漂移；`after-preflight.json` 明确配置未变、未创建 key／state。保留的 onnxruntime CPU vendor warning 来自既有 native 装载，不代表模型调用，亦未据此删除原 stderr。

初次 setup 在 PostgreSQL entrypoint 的临时 Unix socket 阶段误判就绪，原 `setup.json` 与诊断保留；后续对同一 owned PG 使用正式 TCP SQL 校验，再创建 holder，并记录 `setup-completed.json`。这是本轮验证脚本的已确认前置缺口，发生于应用启动前；不能描述为生产应用失败、重新创建 DB 后通过或应用重跑。

## V1 首轮实际失败与有界生产修复审查

`run/validation-20261017-release-lifecycle/evidence/round-1.json` 明确 `passed=false`。双端实际启动、healthy、账号、shadow、设置 PATCH／读回及所有协议检查已完成；对仍存活的 Server PID 66 发 SIGTERM 后，0.007628 秒返回 exit 1，`server-1.log` 保留原错误：`embedded sensor sync: background task exited before cancellation`。Embedded PID 43 由失败 finally 关闭并 exit 0，不能计作该轮预定正常关闭成功。原失败及原 binary 身份必须保留，不能用后续修订版替换。

只读代码复核确认取消归属不一致：`spawn_sensor_sync_loop` 的内部循环监听 `discovery_cancel.child_token()`，`BackgroundStart::spawn_loop` 包装器却按 `OwnedBackground` 自己的 token 判定正常退出。`EmbeddedChannel::stop` 先取消 discovery parent；在 owner 的 relay／`background.stop()` 设置自身取消之前，sensor job 可先返回 `()`，于是被既有 early-return 防护正确地识别为“owner 未取消时提前返回”。这解释了本轮正常 SIGTERM 被报告为清理失败，是生产缺口，与早先 setup 就绪脚本缺口分开记录。

最小修复方向为 sensor loop 直接使用 `background.token()`，使循环退出与监督者的取消身份一致；保留通用 `spawn` 对真正无取消提前返回的失败判定。discovery／connection 的 `spawn_owned` 必须继续保留原 native 清理 future，不改成可被丢弃的 async loop。无需改权限、shadow、协议、期限或并发，也不吞掉其它真实后台错误。回归应有确定性见证 owner 未取消、parent 已取消的阶段，不能仅靠重复 stop 碰概率；最终候选及 RED→GREEN 仍待独立核对。

当前已确认上述一个生产缺口。动态修订版、最终资源／归档仍待 Root 原件，不自行启动测试或设备。

## V2 最终候选与确定性回归

源码最终静审 **GO**：生产差异仅 `spawn_sensor_sync_loop` 将 token 改为 `background.token()`；另有 `cfg(test)` 接线和两项回归。该函数只有 channel `start` 的一个生产调用点；其启动 gate、parent relay、`spawn_owned`、body admission 封闭、join、离线标记／provider 注销／client 释放／revoke 顺序均保持。通用 `BackgroundStart::spawn` 的真实提前返回失败判定未改，权限、精确取消契约、原期限／并发及政策未变。

两项 `current_thread + start_paused` 测试以独立未取消 parent、预取消 discovery、真实 sensor loop 和空 registry 建立原竞争阶段。commit 释放 gate 后，1ms 虚拟时钟只在可运行任务耗尽时推进，且小于原 30 秒 sensor interval；不依赖实际 sleep 碰概率。Arc 计数独立观察 job 保留／释放，两次原 stop/join 在任何观测断言前完成。另一项直接正常返回的 loop 必须保留带任务名且重复 stop 仍可见的错误，限制了吞错式修复。

已只读核对原件 `sensor-owner-red.log/json` 与 `sensor-owner-green.log/json`：

| 阶段 | 结果 | 独立观察 | 退出／范围 |
|---|---|---|---|
| 原生产 RED | 1 passed / 1 failed | owner active；sensor retained=false；resources released=true；两次 stop 为原 early-return Err | exit 101；384 filtered；无源漂移 |
| 一行修复 GREEN | 2 passed / 0 failed | owner active；sensor retained=true；resources released=true；两次 stop Ok | exit 0；384 filtered；无源漂移 |

以上是两项定向回归，不能累计为完整 workspace 通过，也不将原 RED 计入最终通过数。

已静审 V2 `run-lifecycle-stage.py`、`retain-artifacts.py` 和两轮脚本：stage 对 `inputs.json` 中实际留存 binary 在运行前后校验 SHA；产物留存要求 release／link 阶段 exit 0、无漂移，并把 release audit 的 hash／字节数与独立 copy 核对；round 2 再检查同名实际 executable 与 round 1 SHA 相同。源码输入仅两处既定差异，V2 冻结清单为 678 项。最终应逐项交叉核对 stage 的 manifest hash、留存 inputs 与两轮实际 hash，并保留 V1 失败及旧产物身份。

V2 Linux 完整测试、release/link、两轮实际启动／重启以及最终清理与归档尚待 Root 完成并提供原件；本审查者不运行额外批次、不改生产源码。

## V2 实际结果独立复核

已只读核对 `run/validation-20261017-release-lifecycle/v2/evidence/` 最终原件，结论为本轮动态范围 **GO**；上一节“尚待”状态由本节替代。全部 stage 为 exit 0，运行前后源无漂移，均指向 678 项清单 SHA-256 `0133b6f493b3c8b39b365abe03e3cfc035d88b0f21962e0ed6abd745796ed8c7`；已逐文件重算快照 SHA，与清单全匹配。

完整 Linux workspace 原命令只有一次，exit 0，405.963 秒。原 stdout／stderr 合并日志在 899 与 902 行出现唯一 Cargo heading 断开，原计数器如实保留 `INVALID_OR_INCOMPLETE_PARSE`（66 Running），未修改原日志、原 sidecar、原无效报告或计数器。独立逐字节复核派生映射为原 1–898 行、原 900–901 行、原 899 行去 LF 接原 902 行、原 903–1818 行；除此之外没有字节变化。重组脚本锁定原日志 SHA 和唯一精确块，五项自检包括四类拒绝，未重跑 Cargo。

已独立按每个 Cargo target 的最后 libtest summary 重算派生日志：**1287 passed / 0 failed / 5 既有 ignored / 0 filtered**，**67 Running + 5 Doc-tests**，排除 3 个嵌套 summary。Server lib 为 383/0/3，Server main 为 0，appraisal_provenance 为 20，duplex_playback 为 31，server_runtime 为 85。五项 ignored 仍为三项真实 PTY 和两项真实 Easy-Turn；没有新增 ignored。原日志、无效报告、sidecar、计数器与派生输出的现 SHA 均匹配重组记录的 before／after／outputs，派生计数不冒充原始 stdout。

两端 no-default 检查、默认双 release 和链接审计均 exit 0；release 默认构建 113.179 秒。15 个 Sherpa 静态库与既定 archive 校验无变化。实际 ELF、ldd、loader verify/list 均通过，Embedded 仍含已定义 Sherpa 符号。留存 copy、release audit、`inputs.json` 和两轮实际 executable SHA 全匹配：

| V2 产物 | 字节数 | SHA-256 |
|---|---:|---|
| Server | 46,092,208 | `0374aa62602a3e6c2908459b7e8a4a574bd937071d182dbb18172eccb0695616` |
| Embedded | 49,540,200 | `159becdbd3783b77ad308d02096ed39158fc54cfef43f648091a2c0f015f4003` |

V2 第一轮实际 PID 为 Embedded 9539／Server 9562，第二轮为 Embedded 9594／Server 9617。四个进程均在发 SIGTERM 前存活并返回 exit 0，无 finally 额外终止、无强制 kill；两轮均确认 38745／60017／51503 三个 listener 明确拒连。`after-round-1-resources.json` 的同 PG 0 其它 client、holder-only 观测时间早于第二轮启动，满足两轮间独立门槛。

两轮账号、设备未绑定状态／公钥／ID、私钥内容 hash、配置 hash、shadow 政策、政策 ETag `"1"` 全相同；匿名控制仍返回 gRPC 16，四个旧 RPC 仍返回 12。Settings PATCH 为 success、errors 为空、changed 与 ignored_runtime 包含目标，applied 为空；两轮 GET 值 0.25，`settings.toml` 为 0600、8,232 字节且同 SHA `3d75caecba6e222320c3fb491f470aff4b886cdbdaa5f86fff5a328e2589e9e0`。SQLite 同路径／同 schema、完整性 ok，包含 `embedded_state` 与 `direct_responses` 两表。

同一个 PG 在 workspace 结束约 **18.796 秒**后为 0 额外 schema／0 其它 client；两轮 stage 结束分别约 **13.373 秒／0.098 秒**后也为 0/0，holder-only。这里额外 schema 计数排除了 public、information_schema 与 pg_*，不表示应用 public 表不存在，也不宣称退出当刻快照。`cleanup.json` 已 complete：精确 ID＋label 检查后 stop／rm -v 两个任务容器、删除任务 network 并确认对应 PG volume 不存在；共享 astrbot 仍运行。V1 失败环境亦单独保留清理证据。

原日志中的未启用 Home Assistant 工具警告和既有 onnxruntime CPU vendor warning 均保留；两轮无原 sensor cleanup 错误。通过范围仅为当前 Linux ARM64 noop staging 真进程正常关闭／重启与期望设置持久恢复，**不覆盖真实模型、录放音、执行器、Pi 部署、crash／断电、长稳或 R05 整机验收；V01 历史问题仍 open，R03 继续暂停**。最后一项仅待正常源码提交／归档身份核对，不追加动态批次。

## 最终身份与归档收口

上述待核对项已完成。独立使用 Git blob 内容逐项核对源码提交 `6c62c84ee60e3a5fd387f2172ead432031907599`：678/678 与最终运行清单 SHA 全匹配。提交仅包含 sensor token 一行生产改动、测试模块接线和两项回归，无其它源码／依赖修改。

独立逐文件核对 [evidence-index.json](evidence-index.json) 的 **148 份、1,541,367 字节**：归档路径集合完整，全部字节数／SHA 与 index 相符，也与原 `run/validation-20261017-release-lifecycle/` 对应文件逐字节一致。147 份 run 原件与 1 份明确标记的 derived view 分开，原 V1 失败、RED、GREEN、V2 原始 INVALID 计数和确认派生计数均保留。再次只读扫描选定归档，未包含两轮私有 runtime 的随机账号密码、JWT signing secret 或私钥文件内容；完整 settings／runtime、binary／cache 仅留在 ignored run，不在此归档内。

最终结论为本轮有限交付 **GO**，没有剩余审查阻断；不追加测试、硬件或通用审计范围。
