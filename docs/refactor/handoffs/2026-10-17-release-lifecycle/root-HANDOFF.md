# 当前 release 生命周期 — Root

2026-10-17；分支 `codex/architecture-refactor`，基线 `1c63218`，受测源码提交 **`6c62c84ee60e3a5fd387f2172ead432031907599`**。本轮 **10/10 完成**，三个 subagent 分别承担环境/实现、回归/计数、独立复核；Root 统一执行测试、进程、数据库和精确资源回收。默认 shadow、当前政策/权限、精确取消、原期限和默认并发保持；无新依赖或平台，R03 暂停。

## 确认的缺口与最小修复

V1 使用上轮当前 release，在健康、管理员设置保存和 gRPC 边界通过后，Server 接到 SIGTERM 却 exit 1：`embedded sensor sync: background task exited before cancellation`。Embedded 随 finally 正常 SIGTERM exit 0，无强杀。失败不是超时。sensor job 原监听 discovery 子 token，外层监督器监听独立 owner token，异步 parent 桥接留出了内层先结束的窗口。

先用真实 sensor loop + 独立 owner + 暂停虚拟时钟保存确定性 RED：1 passed/1 failed，原资源已回收且两次 stop 都返回同一错误。随后仅把 sensor loop 的 cancel 改为 `background.token()`，使 loop 和监督器共享取消事实；GREEN 2/0/0。对照测试仍要求真正提前返回报错且重复 stop 保留错误。discovery/connection 的 native 清理、通用后台监督器、权限和关闭顺序未改。

## 最终验证

| 检查 | 结果 |
| --- | --- |
| macOS 定向 RED / GREEN | 1/1 → 2/0，均0 ignored、384 filtered；58.45 / 23.12秒命令，target均0.01秒 |
| Linux 默认完整 workspace | 一次 **1287/0/5/0 filtered**；67 Running + 5 Doc-tests；405.963秒 |
| Server / Embedded no-default all-targets | exit0，4.345 / 1.465秒 |
| 两端 default release / native link | exit0，113.179 / 0.431秒；真实 ELF/loader/ldd，Embedded170个已定义Sherpa符号 |
| Embedded 只读预检 | exit0；配置不变，无key/SQLite创建；保留CPUID warning原文 |
| V2 正常生命周期两轮 | 0.791 / 0.627秒；两轮均passed、四个进程exit0，无finally兜底/强杀 |

计数为 passed/failed/ignored，定向与整包不叠加。5项ignored仍为3项PTY与2项Easy-Turn，本轮不单独运行。Linux测试包含已有软件null PCM，不代表真实录放音。

原Cargo日志的899/902行出现一次stdout/stderr交错，把标题拆开。原计数器因此报告INVALID，未覆盖原件、未重跑Cargo或改计数器。仅对唯一已审核形态生成派生视图，锁定原hash、保存完整行映射，5项接受/拒绝自测通过；既有计数器和独立重算均确认上述67+5与1287/0/5。见原 `workspace-default.log`、`workspace-counts.json` 和 `reconstruction.json`；confirmed报告行号指向派生视图。

## 两轮实际进程契约

Embedded直接使用当前全noop模板，仅替换loopback端口；保持未provisioned、未绑定、skip_binding=false、全部七个noop槽和设备/发现关闭。首次启动生成设备ID/key并写回配置属于预期，重启比较以首次正常关闭后的配置为基线。Server明确选择noop LLM/STT/TTS并禁用EasyTurn/HA，保留默认Memory/cache/shadow政策。Server advertisement关闭，但EmbeddedChannel内部mDNS browse仍在任务internal network内，不声称完全无发现。

两轮HTTP健康全部模块healthy，管理员device/status在线且无伪造遥测；同账号、device ID、公私key文件hash、两份配置hash、shadow政策及Context ETag `"1"` 恢复。第一轮将纯非政策 `core.profiles.llm.major.temperature` 保存为0.25，回执明确changed+ignored_runtime、非applied；第二轮只读核对，不再次PATCH。settings文件hash相同、权限0600。它证明desired settings持久化/重载，不证明noop模型使用该温度。

未绑定公开状态读取正常；匿名ObserveControl返回gRPC16，四个移除的旧RPC返回12。每轮都先确认进程alive再SIGTERM，Server关闭耗时0.007289 / 0.007970秒、Embedded0.005775 / 0.007013秒，原10秒等待上限未改。三端口均拒连；SQLite同路径、所需表及schema一致、integrity_check=ok，但文件大小4096→24576，不声称全字节相同或实物状态恢复。

## 身份、归档与回收

678输入在宿主、只读snapshot与提交Git blob逐项一致，manifest `0133b6f493b3c8b39b365abe03e3cfc035d88b0f21962e0ed6abd745796ed8c7`；相较V1仅channel.rs及新增回归文件变化。所有最终阶段drift=[]。既有archive及实际15个native静态库构建前后相同。

- Server release：46092208 bytes，SHA-256 `0374aa62602a3e6c2908459b7e8a4a574bd937071d182dbb18172eccb0695616`。
- Embedded release：49540200 bytes，SHA-256 `159becdbd3783b77ad308d02096ed39158fc54cfef43f648091a2c0f015f4003`；源码未改，重建后与上轮一致。
- 独立产物在 `run/validation-20261017-release-lifecycle/v2/artifacts/`，copy/审计/两轮实际进程摘要全部一致。
- [证据索引](evidence-index.json)共 **148份、1541367字节**，含V1失败、原环境预检失败、RED/GREEN、V2、原INVALID和明确标记的派生计数。任务密码/JWT签名密钥/私钥原文扫描通过，私有runtime与完整settings不归档。
- V1失败后12.313秒PG0/0；V2全量后18.796秒PG0/0，两轮后13.373 / 0.098秒PG0/0且holder-only。最终清理再检查0/0，V1与V2各自两个容器、匿名卷和internal network按精确ID/label删除，共享astrbot保留。没有待继续的服务或Cargo作业。

V1最初setup还遇到PG临时Unix socket误就绪，原失败单独保留；正式TCP预检接续。V2从开始使用30秒总预算的TCP SQL就绪，不混同应用修复。

## 恢复与未完成范围

从[当前检查点](../../resume-checkpoint-release-lifecycle-2026-10-17.md)和[checklist](../../checklists/release-lifecycle-2026-10-17.md)继续。本轮仅证明既有Linux ARM64、全noop、正常关闭后的同状态重启；不代表异常强杀/断电、真实模型、Mac/Pi独立服务部署、绑定现场、声学/AEC、执行器或长稳通过。历史V01根因仍open，不由本轮sensor关闭修复倒推关闭；六轮仍2/6。下一软件出口为发布资料收口与真实模型服务端点/版本/运行方式确认，随后在明确范围验证模型链；R03继续暂停。

作者文档及源码diff检查通过；完整归档diff检查仅报告原始gRPC headers的CRLF/空格和日志EOF空行。保留原始字节，不为消除格式提示改写验收原件。
