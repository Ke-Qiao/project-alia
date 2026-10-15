# V01 外层截止与清理边界 — 独立审查 HANDOFF

日期：2026-10-15（Asia/Shanghai）；基线 `99c946d`。分工 `/root/v01_review` 只读源码、既有原始证据及后续差异，只写本页，不运行 Cargo、测试二进制、PG、模型、SSH 或硬件，不提交。Root 统一动态验证、资源回收与提交。默认 shadow、政策、25/45 秒、默认测试并发保持，R03 暂停。

## Checklist

- [x] 读取上一包调查与资源边界，区分历史内层 25 秒超时和本轮外层 45 秒/清理问题。
- [x] 检查 `with_db`、`with_scene`、Companion、RPC 与 Runtime 实际所有权及调用点。
- [x] 提出具体可验证缺口、隐私/清理边界与有限验证建议。
- [x] 交叉复审最终差异及作用范围；四项审查问题均修订，final-v2可验证。
- [x] 核对本轮新验证身份、实际计数和最终资源证据。

## 历史证据能支持的范围

上一包同 baseline binary 两批 9/0/0，诊断版完整 workspace 1245/0/5，其中 duplex 10/0/0。三批旧超时未复现，V01 根因继续开放。上一包仅补失败观察，未动态触发整条 Companion 失败清理；外层 45 秒 drop、初始化失败和 cleanup 自身中断明确未得到完整动态见证。最终全量结束即时 schema/连接快照没有留存，不能把基线 0/0 或随后移除容器当作该时点证据。

本包针对确定的外层取消和清理所有权，不再重复上一包三批以求“历史根因修复”。即使以下风险经新回归确认并修复，也不能倒推它们导致 09/28 的三项内层 25 秒超时。

## 当前代码审查

| 边界 | 基线事实 | 风险与需要的证明 |
| --- | --- | --- |
| `delivery_support::with_db` | `CASES=3` permit 与 `Fixture::new` 在 45 秒之前；45 秒只包 `test(f.clone())`；catch 后才调用 `f.cleanup` | 超时会 drop 整个业务 future；关闭 pool、DROP schema 在 45 秒之后。缺少业务 owner 完成证明时，不能仅凭 timeout 返回就开始宣称全部回收。不能为修复取消把整个 fixture 变成无期限。 |
| `RpcServer::close` | 先 `self.task.take()`，把拥有的 JoinHandle 按值交给 3 秒 timeout；Drop 只处理仍在 `self.task` 中的 handle | 3 秒超时或外层取消会丢弃已取出的 handle；此时 Drop 见 None，无法 abort/join 该任务。应以实际持有 listener 的任务证明句柄保留与实际退出，不能只检查 cancel token。 |
| `RpcServer::drop` | cancel 后调用 abort，但不能 async join | 仅“发出 abort”不等于任务已退出或 listener 已释放。正常关闭和 fallback 取消必须分别描述能保证的范围。 |
| `Runtime::close` | 释放 registration/resume、revoke native、Delivery shutdown、Embedded stop、channel stop、RPC close；中途存在 unwrap/assert | 前一 owner 的关闭错误会跳过后续 owner。应保留首次错误/原测试失败，同时仍尝试所有必要 owner 清理，避免一个失败掩盖其它资源归属。 |
| Companion | 资源在 25 秒之前逐步创建；内层25秒 catch 后才显式 heartbeat join、屏障释放、Server shutdown、RPC close、paired close | 初始化 panic 或45秒 drop可以跳过这段显式清理；heartbeat JoinHandle 本身没有 Drop guard。必须区分“已请求停止”与“已 join”。 |
| `ServerRuntime::drop` | 请求关闭并启动现有 `RuntimeCleanup`；`shutdown_handle` 可观察完成 | 生产 owner 已有异步清理，不应为测试再造通用生命周期框架；Drop 返回仍不能证明 DB cleanup 前 worker 已完成。 |
| Native mock | Condvar 等待通过 `revoke_all`/取消释放；某些阻塞逻辑运行于 blocking task | Tokio abort 不能中断已经执行的 blocking/native 工作。必须释放原屏障并核对原 owner 完成，不能用 abort 或删除临时目录替代。 |

作用范围需随最终差异逐项核对：RPC wrapper 被 paired playback 与 Companion voice 两处创建；paired Runtime 覆盖原六项播放场景与三项 Companion 场景。若调整公共 `with_db`，所有 Delivery 相关集成 target 都是调用方，不能仅跑新增小测试后声称全局行为不变。尽量复用现有 Runtime、ShutdownHandle、CancellationToken 和 fixture 生命周期，不新增生产抽象/依赖。

## 本轮证据要求与有限验证建议

1. 先用保留的旧实现跑一次能区分缺口的定向 RED：受控任务持有真实 loopback listener，取消/超时 `RpcServer::close` 后核对任务未退出或旧 owner 仍保留资源。不要用通用 Tokio 教科书模拟替代实际 helper；端口重绑定成功本身也不能证明是原 listener 已退出。
2. 对外层 45 秒采用实际路径和明确阶段/屏障观察，区分 test future drop、取消请求、各 owner join、DB cleanup 开始/完成。可以把新用例自己的可控 future 缩短以验证相同 helper，但必须明确这是加速机制用例，不能写成真实 45 秒全路径实测；至少应证明生产测试的45秒常量未改。
3. 一次失败应保存原结果与 cleanup 结果，定向 GREEN 验证成功/timeout/panic 和 cleanup 错误后的所有必要 owner 仍被处理；不要在 stderr 打印原始 SQL/身份/来源/授权内容。沿用固定阶段与匿名计数，不能根据并发日志顺序杜撰场景身份。
4. 完成定向修复后，按最终真实影响范围做一次默认并发回归：仅 duplex fixture 则完整该 target；若公共 `with_db` 改变则 Root 统一受影响集成目标或完整 workspace。除新失败、代码变化或身份不匹配外不重复同一批次。旧历史复现上界不因此重新开始。
5. 当前回归结束且任务 PG 仍运行时及时保存本轮 schema/连接观察，随后按精确任务 ID/label 清理；进程、listener、schema、容器是不同证据层，不能相互替代。缺失的观测保留 unknown，不为补历史时间点重启数据库。

以上为审查建议，不是已执行结果。初始审查后等待 Root 的最终 diff 与新证据，不另跑测试或扩张工作范围。

## 第一版实现差异复审（尚未冻结为最终版本）

已读三份既有测试文件差异与新增 `cleanup.rs` / `db_cleanup.rs`。范围仍为 test fixture，复用已有 `ShutdownHandle`、`start_build`、`LocalBoxFuture` 和 Tokio 能力，无生产改动/依赖。`RpcServer` 改为借用字段中的 JoinHandle，timeout 时 abort 后 await；partial paired owner 在 `channel.init` 前保存，ServerStartup 的完成 handle 在 wait 前保存；这些方向与已确认风险相符。

冻结前需处理的三项已发给 Root，不在本分工改源码：

1. **原45秒语义**：第一版把 context cleanup 完全移到45秒之外，可能使 body+context cleanup 超过原 deadline 后成功。Root 已独立确认并安排采用同一个绝对截止：截止后标记失败但排空原 cleanup future，DB cleanup仍在原范围之外。不能重建 cleanup future或延长成功资格。
2. **paired Delivery 不完整退出**：`DeliveryService::shutdown` 超时会保留 supervisor JoinHandle；第一版 `Runtime::close` 仅在尝试其它 owner 后 assert pending/error，随后 `finish_case` 仍关闭数据库。需区分“已结束但失败”和“仍在运行”，证明退出或明确保留资源，不能把 pending worker 与已完成的 heartbeat panic 等同。Companion 的 builder owner已有 shutdown handle等待，paired 原 Delivery owner仍需对应证明。
3. **同步 revoke panic**：两处 `revoke_all()` 尚在 catch_unwind之外；底层测试 Device 的 Mutex unwrap 可能 panic，跳过后续 owner。与 async shutdown 一样，应在fixture内收集失败后继续必要清理，避免最前一步中断整个清理链。

新增 paused-clock用例能够验证原45秒timer与真实初始化后的资源退出，但其45秒为Tokio虚拟时间推进，不是45秒墙钟现场超时；应在交付证据中明确。heartbeat panic用例证明已退出但报错的任务不掩盖原失败，不独自证明 shutdown仍pending的边界。当前没有运行任何测试，GREEN/RED与资源身份由Root提供，待修订版继续复核。

## 第二版实现差异复审

上述三项已在源码修订：`run_context` 固定同一45秒绝对deadline，`finish_case` 只超时借用同一个已pin的cleanup future，过期后继续await原future但最终不能成功；新增两项纯timer用例覆盖44+2秒清理和原panic保留。paired Delivery 首次pending时保留scene/数据库，先尝试其它资源清理，再等待原running结束并收集原join，首次失败不转成功；两处同步revoke均纳入catch。现有production API没有更细的completion句柄，极端supervisor panic导致running不复位仍是既有边界，watchdog退出不算清理完成。

真实PG外层45秒用例已移除虚拟时间推进，使用真实默认时钟，纯timer与RPC用例才使用paused clock。新增回归预期为纯deadline6项、RPC3项和真实owner/PG2项，共11项；此处仅为源码计数，不是执行通过数。

第二版仍有一项测试证明缺口已即时报告Root：`injected_outer_failure` 在cleanup closure内部assert owner完成与数据库仍开放，但`finish_case`按设计捕获cleanup panic并保留原timeout/primary panic。若这些assert失败，外层仅核对原panic文本和数据库关闭后的状态仍可能通过，形成假阳性；需要独立保存并在最外层断言pre-DB检查全部成功。尤其`SELECT 1 is_ok`检查必须有未被原panic优先规则屏蔽的外层见证。待修订后再关闭diff复审项。


## final-v2 冻结前审查结论

pre-DB假阳性已修复：新增独立 `Arc<AtomicBool>` 初始false，只有全部owner完成、目录释放及数据库仍可查询的断言均通过后才写true；最外层catch验证原失败后再独立assert标志。因此cleanup内部断言失败不能被原panic优先规则掩盖。未发现其它阻断，可执行Root统一的最终全量。

本分工只读重哈希 final-v2 manifest **671/671** 输入匹配当前文件，manifest SHA-256 `021b2fb3d46d64ddd98c2344cb078647b66bc143dc1f41d4b5d1326e19b6a6d2`。原过时全量执行已因上述确定的测试假阳性主动终止：73.33秒、exit -15、900秒进程watchdog未触发、源码无漂移；它不是最终通过，也不是代码断言失败。其后资源记录为2个隔离schema/0其它client，Root删除并重建同一任务数据库，final-v2前为0/0。旧执行、重建与新执行身份分别保留，不把局部完成target累加到最终通过数。

当前本分工4/5，最后一项等待final-v2完整结果和最终即时资源记录；未新增执行、修改源码或运行数据库。paired supervisor极端panic使running不复位的既有局限仍保留，进程watchdog不能证明该owner完成。


## 最终独立证据核对：5/5

已只读核对 final-v2 完整日志与记录，`cargo test --workspace --offline --locked --no-fail-fast` exit0，347.91秒，默认libtest并发、`CASES=3`、原45/25/3秒不变；900秒进程watchdog未触发。按每个Cargo target的最后摘要重新计算 **67 Running + 5 Doc-tests，1256 passed / 0 failed / 5既有ignored / 0 filtered**。子进程重入、旧RED、首轮GREEN和主动中断的全量均不累计。

其中duplex目标 **21/0/0，45.60秒**，包含原10项和新增11项（纯deadline6、RPC3、真实owner/PG2）。真实PG外层45秒用例使用正常墙钟，没有虚拟时间推进；其最外层独立标志已证明所有pre-DB断言完成。纯deadline/RPC用例使用paused clock，各自结果不混称真实45秒等待。

当前671个输入重哈希仍全部匹配 final-v2 manifest `021b2fb3d46d64ddd98c2344cb078647b66bc143dc1f41d4b5d1326e19b6a6d2`。保留二进制 `run/validation-20261015-v01-outer/final-duplex-playback` 实测110661120字节、SHA-256 `bc2c7cb8e1291aa60547963d56e45071703ec8df68c9b4e04e2623d812865184`，与摘要一致；该二进制不是历史V01失败artifact。

旧原件分别保留：RPC RED **1通过/2失败/10 filtered**、exit101；首轮GREEN **18通过/1失败**、exit101（真实PG/runtime被虚拟时钟推进导致cleanup错误，最终改为真实时钟，保留shutdown failure断言）；过时全量73.33秒exit-15由Root主动停止修正被原panic掩盖的pre-DB断言，不是watchdog超时或最终验证。不能把这些旧失败覆盖成一次成功。

最终PG资源记录采集于2026-10-15T07:31:01.579316Z，按run开始时间加347.91秒计算，距命令结束约 **56.11秒**，得到 **0 schema / 0其它client**；此间未重启数据库。该结论是结束后的实际快照，不是“结束当刻”观测，也不能补写上一包缺失的时点。随后cleanup记录逐项exit0，完整容器ID与`v01-outer-20261015` label一致，stop/`rm -v`后确认任务容器不存在；共享astrbot/runtime保留。

最终审查未发现阻断，本分工 **5/5** 完成且停止写入。Root统一提交与中央恢复资料。生产代码/默认shadow/政策/硬件范围未改变；R03仍暂停。没有穷尽partial初始化、poisoned native锁和supervisor panic注入；已关闭本轮确认的fixture缺口，**历史V01 25秒超时根因继续open**，不以本轮1256通过倒推其原因。
