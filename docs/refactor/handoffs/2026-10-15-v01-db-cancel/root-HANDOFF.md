# V01 初始数据库构造取消 — Root HANDOFF

2026-10-15；分支 `codex/architecture-refactor`；基线 `aca6ca0`，原实现 `dc3ec17`。本轮10/10完成；源码 `8bcda7af892ba36aea98b9de785afa68e3935940`。下文过程按版本区分，最终结果见末节。

## 范围与确认过程

读取上轮恢复检查点、四份HANDOFF与24份原件；三个subagent分别负责所有权、取消复现和独立审查。共同结论是catch_unwind不能接住future Drop，连接关闭也不等于原SQL已完成。本轮只调整测试夹具，不改生产/政策/shadow/依赖/平台范围。R03暂停。

共享Fixture被九个integration目标引用：delivery_service、output_control、body_delivery、voice_stream_live、delivery_text、duplex_playback、output_lifecycle、delivery_text_service、delivery_repository。保留body非Send调用方式、CASES=3、默认libtest并发和原45秒起点。原25/32/45秒、3秒RPC、1秒诊断、10秒Delivery shutdown不变。

## 确定性RED

在原WorkPool checkpoint（CREATE已确认）和UserSeed checkpoint（工作pool与迁移已完成）用oneshot通知位置后，直接drop整个probe_initialization future。hook Drop sentinel确认实际取消；不是注入panic。

一次RED结果 **0 passed / 2 failed / 0 ignored / 27 filtered**，测试0.26秒、命令12.04秒，source_drift=[]。两项schema_remains=true；UserSeed保留的pool句柄仍能SELECT。每项先观察事实，再关闭保留pool、DROP本随机schema、close observer，最后才失败断言，避免RED污染后续。命令结束后同一PG资源0 schema/0其他client。

RED输入675项，SHA-256 `0243d535656e1ffb6d7ccae8631d08f51d5265ccdcd915f52b63431b939c0a49`；RED版本helper和测试源码另外保存。GREEN需要适配有独立owner/完成句柄的新构造入口，仍保留同样取消位置和清理前资源断言，不把不同API伪称完全相同测试文本。

## 修复契约

独立worker从CASE许可到原connect/CREATE/后续SQL、Fixture使用和cleanup全过程持有资源。取消只撤销ready交付和后续阶段；在途真实数据库future继续等待原结果，然后才能DROP原schema。测试pending hook可响应取消，不能将此机制用于丢弃真实SQL。

ready交付Fixture clone，worker继续持有原Fixture；owner Drop或ready waiter Drop只请求取消，正常close显式await原JoinHandle的共享completion。原初始化panic仍经ready传回，cleanup失败独立可观察。所有completion都被丢弃/运行时退出不等于完成，不为该范围增加全局supervisor。

## 定向验证与独立复审

- WorkPool取消时drop整个owner；UserSeed创建ready后完全不poll，只等待独立hook信号再drop。预先保留completion，join后、兜底前证明schema不存在且pool已关闭。
- 真实CREATE在途：另一个事务创建同名随机schema不提交，观察原CREATE实际锁等待后取消，并确认同一backend仍在等待。close与原completion等待均为pending，drop等待不丢原worker；rollback释放阻塞后join，独立观察原backend退出与schema不存在，最后才执行兜底。
- 缓冲ready：只手工poll一次注册waker，独立等oneshot发送唤醒，不再poll消费；drop后重等ready必须报告固定取消错误，不能交付正在清理的Fixture。DropGuard在ready函数返回future前建立，覆盖未poll取消。
- 首次GREEN **7/0/0/24 filtered**，包含原3项初始化失败和新4项取消，target0.62秒、命令11.62秒，无源码漂移。结束约0.152秒后同库PG0/0；不是退出当刻的快照。
- 三个subagent均复核GREEN；独立review无阻断。GREEN与V1全量675项输入逐文件相同，V1 manifest `d406435ee26b4e303476a61aeddc5c1204251f320278530479559b6241c7eada`。当时完整workspace尚在运行，后续失败与V2结果见下文。

历史V01的25秒根因仍open，本轮只关闭已确认的构造取消缺口。未触及真实模型、录放音、硬件、Linux/Pi；R02硬件AEC资料门槛、R03暂停和R05现场出口保持，六轮仍2/6。初始建库与最终SQL teardown仍在原45秒外；native poisoning、supervisor panic、强杀/runtime teardown和数据库失联无法排空等条件不能由本轮普通取消用例推定安全完成。

## 首次全量失败与修订理由

V1完整workspace **1265/1/5**（0 filtered），duplex30/1/0，368.88秒、exit101、drift=[]，退出约0.183秒后PG0/0。失败仅为新buffered-ready用例：其5秒通知观察早于CASES准入和数据库初始化，取消后UserSeed证据为空，随后的unwrap遮住了阶段失败并跳过observer显式close。日志不能进一步断言具体卡在permit还是前置SQL。

保留V1原件、源码快照与二进制后，授权flow只修该测试：UserSeed entered/release握手确认阶段，放行后才进入原5秒通知观察；握手或证据缺失都先join原owner、显式close observer，再断言。原helper/业务期限/并发不变。Root此前进度更新把部分通过误称为目标通过，已在发现日志后立即更正；最终只依据完整结果。修订版独立审查后只运行一次全量，不重复未变源码求绿。

## 最终 V2 与交付

V2 全量 **1266/0/5/0 filtered**，67 Running＋5 Doc-tests，285.11 秒、exit0、source_drift=[]；duplex31/0/0，46.53 秒。5 个既有 ignored（PTY3＋Easy-Turn2）保持，不合并 RED／定向／V1 失败中的通过数。V1→V2 只有 initialization_cancel.rs 内容改变，其余674输入完全一致；helper从首次GREEN以来未改。

源码正常提交 `8bcda7af892ba36aea98b9de785afa68e3935940`，675项冻结输入逐blob吻合，最终manifest `51b11511010fedfa47cf8f01b814eb17731ded45b5404fb7ad01ff2e4ce9c553`。命令退出约0.175秒后同PG0 schema/0其他client，随后按exact ID与label停止并删除本轮容器/卷，共享服务保留。见[完整证据索引](evidence-index.json)，共31份原件；失败源码、日志和二进制身份均保留。

本轮已完成具体初始构造取消边界，不把它归因为历史Companion超时。后续仅在出现新增失败、原artifact或可区分条件时继续V01有界定位，不重复同源批次求绿。R02继续硬件AEC优先和资料门槛，R03用户暂停，R05等待现场出口；六轮仍2/6。进程/runtime强制结束、永久DB失联、生产supervisor自身panic/native poisoning及任意ServerStartup点未穷举；初始DB与最终SQL teardown仍在原45秒之外。
