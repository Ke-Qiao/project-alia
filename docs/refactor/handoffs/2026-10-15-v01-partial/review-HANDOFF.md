# V01 部分初始化与 paired late-drain — 独立审查

日期：2026-10-15（Asia/Shanghai）；基线 `af5b35b`；分工 `/root/v01_review`。本分工只读源码、原证据与后续差异，只写本页；不执行 Cargo、测试、PG、SSH、模型或硬件，不提交。默认shadow/政策、原45/25/32/10/3秒边界与默认并发保持，R03暂停，无依赖变更。

## Checklist

- [x] 区分上轮已执行证明与尚未覆盖的 partial/late-drain 边界。
- [x] 检查 initializer owner发布、cleanup早期panic和running/nohandle语义。
- [x] 提供可区分的验证建议和原失败保留要求。
- [x] 交叉审查最终diff与测试是否可能假阳性；定向证据通过，可冻结全量。
- [x] 核对实际运行身份、计数与资源证据；最终独立审查5/5。

## 已有证据与本轮范围

上轮归档30/30文件的长度/SHA由本分工重新核对一致。上轮完整workspace1256/0/5，duplex21/0/0；11项新增覆盖RPC取消/timeout、同一绝对45秒deadline、完整初始化后的外层超时和heartbeat失败。真实owner回归在数据库仍开放时检查完成，并以最外层独立标志避免原panic优先遮蔽断言。上轮结束后约56秒资源快照0/0，随后容器回收，均不等同退出瞬间观测。

上轮明确没有动态制造真正的channel初始化中断、ServerStartup等待取消、paired首次10秒shutdown未完成后late-drain、native锁中毒或supervisor panic。本轮应针对这些边界的实际owner和等待顺序，不重新运行旧三批历史超时空跑。历史V01 25秒根因继续开放。

## 初始源码审查

### running=true但无JoinHandle不能解释为完成

`DeliveryService::start` 在spawn前写running=true，只在`run(...).await`正常返回后写false。若最外supervisor panic，`shutdown`可join已结束任务、take句柄并记录failure，running却保持true，paired fixture的late-drain会继续等待。此状态需要明确区分：未启动（running=false/nohandle）、正常停止（false/nohandle）、仍运行（true/Some handle）、异常终止但后代完成未证明（true/nohandle）。

不能仅加一个Drop guard将running设false并声称清理完成：`run`内部拥有两个JoinSet，unwind时drop会请求abort后代，不能证明所有原native/adapter副作用和任务均已结束。现有生产语义本来就保守：adapter panic会保留future/依赖；RuntimeCleanup遇stop panic会保留action及下游资源；builder的Delivery cleanup会反复shutdown直到pending=false，并保留原错误。没有更细的公开completion API，单凭supervisor JoinHandle结束不能降低这些保证。

因此本轮可以先证明paired普通late-drain的真实完成；极端panic/nohandle若不能确认后代quiescent，应保留有界隔离进程中的未完成证据。进程watchdog退出不是清理成功，不因无法安全结束就刷新期限或释放数据库。

### initializer owner发布时机

paired Runtime在channel.init及Delivery.start之前已写入slot，这两个await点取消后可从外部context执行清理。RPC spawn至slot之间没有await，但仍有固定配置解析和构造的同步panic点；该时段RpcServer Drop只请求abort，不执行async join。不能笼统说“所有部分初始化都已证明”。更早的SQLite open/binding await由局部值管理，尚没有paired slot，亦需按所选实际失败点分别记录。

Companion在ServerStartup.wait前已保留ShutdownHandle，wait取消会由原Startup Drop请求取消；独立startup owner应继续rollback。Runtime成功后在channel注册/binding hydrate及voice bind之前已保留ServerRuntime。已有资源应逐项复用，避免为测试引入生产初始化框架。故障注入宜使用现有可控配置、已存在的await/barrier与当前生命周期 API，不能仅向Option手工放一个伪造“半初始化”对象就称覆盖实际initializer。

### cleanup提前panic与数据库顺序

上轮Runtime.close已catch同步revoke与各async关闭，最后才assert；Companion同样收集失败并等待builder shutdown。仍需验证首次Delivery关闭未完成时，后续native/channel/RPC释放确实能解除等待，原10秒失败必须保留；数据库应在真实owner退出之前保持开放。

`finish_case`按设计在context cleanup panic后仍尝试DB cleanup，并优先恢复body原panic。因此只有context cleanup已经完成所有必要owner尝试并保留未完成owner时，才能安全地返回失败。不能用任意注入在cleanup入口的panic来模拟“安全关闭已完成”；若尚有未完成资源，必须明确保留owner/依赖，不能把捕获panic等同于完成。

## 有界验证建议

1. 选择实际partial await点：至少channel初始化中断和startup ready等待中断各自保存已发布owner/资源与原失败。在DB仍开放时检查原owner退出、任务数/通道状态及listener原任务完成，随后验证DB关闭。当前有scope的真实连接/任务证明优先于新端口重绑定。
2. paired late-drain需真的跨过Delivery原10秒shutdown期限：受控adapter/native gate保持原owner，首次shutdown返回incomplete后数据库/通道依然存活，再由独立屏障释放；cleanup完成后仍报告首次失败。不能把外层调用者30ms timeout当成内部10秒分支证明，也不能只测试底层DeliveryService而不经过本轮paired.close。
3. 保留同一45秒绝对成功deadline与25/32秒body边界，过期后排空原cleanupfuture仍算失败；不重建future或刷新机会。不用虚拟时间推进包含真实PG/Runtime的复杂场景。
4. 所有“owner先于DB结束”的断言须由最外层独立证据标志验证，避免cleanup断言被原panic优先遮蔽。原panic身份、首次shutdown失败和后续cleanup错误分别检查，不把日志包含某段文字当完整证明。
5. 新RED/修订/最终运行分别冻结输入和实际artifact；最终默认并发回归覆盖真实影响范围。既有ignored、子进程重入和中断批次不累计。无新增原因不反复空跑。

初始审查3/5，等待Root后续diff。本页不是测试通过证明，也未预设历史V01根因。


## 最终候选差异与定向证据审查

只读检查五个变更/新增文件：`delivery_support/mod.rs`、`duplex_playback.rs`、`initialization.rs`、`partial_runtime.rs`、`late_drain.rs`。生产/配置/依赖未改。实际缺口是公共Fixture初始化在schema创建后发生panic/后续SQL错误时没有统一清理；现在先保留admin/随机schema/未连接work pool的owner，捕获初始化unwind后执行原cleanup并恢复原panic。`Disconnected`工作pool可安全跳过close，`DROP SCHEMA IF EXISTS`处理尚未创建/已不存在的schema。原45秒从完成Fixture构造后计时，未扩张期限或更改CASES=3。

测试注入只有现有测试helper内部三个检查点，不是生产框架。WorkPool用例是schema创建后、连接work pool前的受控panic；UserSeed/Policy用例通过本例schema改列名制造实际后续SQL错误。新旧版本均先保存schema/pool观察，再执行受限兜底清理，最后在最外层验证原错误与没有泄漏，RED不会遗留其故意暴露的schema。不能把WorkPool模拟panic称为真实网络连接失败，也不能把catch_unwind当任意future取消的异步析构。

partial runtime两项调用实际`Runtime::initialize`：缺失`body_deliveries`触发真实channel init SQL错误；ACCESS EXCLUSIVE锁与pg_locks未授予关系锁确认initializer确实卡在该await，然后drop原future。已发布slot在cleanup中被取回并完成RPC/Delivery/session/目录清理，数据库仍可查询；最外层独立AtomicBool保证这些断言不能被原body panic掩盖。此证据不覆盖ServerStartup所有等待点或initializer任意位置取消。

late-drain用例持有真实`SubmissionWrite`，调用同一个paired `Runtime::close` future，借用该future观察11秒；原Delivery内部10秒期限未变。先保存close/supervisor仍pending、DB仍开放、目录/listener保留的观察，再finish原write并await同一个close与原dispatch。所有最终断言在预期失败catch之外，核对原`paired Delivery cleanup failed`、实际owner结束和DB随后关闭；没有重建cleanupfuture、提前释放gate或将原deadline failure转为成功。该用例证明本轮选定的late-drain路径，不证明poison/supervisor panic后可安全收尾。

已只读核对原件：初始化RED **0/3/0，21 filtered**，三项均在`failed initialization left its schema behind`断言失败，原注入/SQL错误日志保留，exit101、命令12.54秒；兜底后任务PG0 schema/0其它client。定向GREEN **6/0/0，21 filtered**，测试11.53秒、命令21.88秒、exit0，原默认并发，source_drift为空；21filtered仅为本次定向选择，不能替代全量验收。同一PG随后为0/0，快照距命令结束约18.22秒，未称退出当刻。

定向阶段 **674/674** 输入重哈希匹配GREEN manifest `25bfee8a75f0782b5668194ac9a34c740ff0429ec6239834f3d29e7ed9e043ef`。RED/GREEN源码身份分开，RED3失败不能掩盖为最终成功。该阶段审查未发现阻断，可冻结全量；当时本分工4/5。最终完整计数、提交身份与资源已在下节收口为5/5；本分工不自行执行测试。


## 最终完整运行与资源独立核对：5/5

本分工读取 `final-workspace.log/json`、`validation-summary.json`、`final-resources.json` 和 `cleanup.json`，并独立重新解析完整日志：**67 Running + 5 Doc-tests，1262 passed / 0 failed / 5既有ignored / 0 filtered**。每个Cargo target取最后结果，不重复累计子进程重入、定向GREEN或RED。完整workspace只执行一次，exit0、405.60秒，900秒进程watchdog未触发；默认libtest并发、原CASE准入与所有业务期限未改变。

最终duplex为 **27/0/0/0 filtered，45.63秒**，包括原21项及本轮6项。5个ignored仍为真实PTY三项与Easy-Turn两项，本轮没有新增忽略，也没有把它们算通过。定向6项与全量6项是重复执行，不增加unique总数。

最终manifest SHA-256 `3f15e093efd3863143d5bcff0be0540ac4fb5c306b01b0650b69935abfd24747`；本分工重哈希 **674/674** 当前输入全部匹配。GREEN与final manifest逐路径内容哈希完全相同，manifest整体SHA差异来自清单元数据，不是源码漂移。保留配对binary实际111180224字节，SHA-256 `a593537e58bceca40c2113456b6255c9e578601b8002b5acef7dfc3d07c104e4`，与最终收据一致。

同一任务PG在命令结束后 **0.584013秒**采集到0 schema/0其它client，期间没有重启；仍表述为结束后快照，不称退出瞬间。资源记录、创建记录和cleanup的完整容器ID一致，`v01-partial-20261015` label核对正确，清理命令逐项exit0，stop/`rm -v`后确认任务容器不存在；共享runtime/astrbot保留。

最终审查无阻断，本分工 **5/5** 并停止写入；提交与归档身份由Root统一补齐。已确认的Fixture初始化unwind/SQL失败清理缺口关闭，实际channel init取消与paired late-drain获得新证明；任意初始DB构造取消、所有ServerStartup等待点、native锁中毒和supervisor panic后的完整清理仍未穷尽。生产代码/默认shadow/政策/依赖/硬件范围未变，R03暂停；**历史V01内层25秒超时根因继续开放**。
