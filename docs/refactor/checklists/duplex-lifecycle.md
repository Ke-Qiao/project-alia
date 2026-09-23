# 输出生命周期阶段验收 · 2026-09-22

状态：2026-09-23 用户额度恢复后继续。下列 2026-09-22 结果作为历史证据保留；Root V2 完整 workspace 已通过 967/0/5，另补 5/5 真实 PTY/模型；Linux 定向矩阵 325/0/0。下方历史未完成项以[总 checklist](duplex-integration.md)为准。暂停源码已保存为 `ae75a4ee92b1ea2e8c11a1d20c488ba62125d81f`。
交接位于 `run/output-lifecycle-handoff-20260922/HANDOFF.md`，精确命令、哈希和资源记录见同目录 `RESULT.json`。

- [x] 控制账本固定提交 `4846b4a` 与 `56e9acc` 已合入；完整真实 PG 13/13。
- [x] 后加入 Audio 的硬 scope 期限持久收紧，Resume 不丢失该限制；单次 hold 不永久压缩整个输出寿命。
- [x] 队列 enqueue 前保留原 MediaOwner；drop waiter/最后 producer 不丢排队媒体，重复查询不重放。
- [x] ACK 丢失仅 Observe 原 command；无法确认保持 Unknown，不能自动重发 Control。
- [x] 实际 SQL 许可到期时关闭本地 gate 并退休；不把未收到的 ACK 写成已确认结果。
- [x] Held 期间原媒体 owner 的 SQL heartbeat 持续，未重新 reserve/dispatch。
- [x] 原 scope fence 超时仍保留实际 future/channel，真实 join 前 shutdown 不成功。
- [x] 明确本地取消/退休先持久化，不等待未知设备 stop；同时保留原 stop future 到实际结束。
- [x] 原 `delivery_service` 12 例迁移到真实原 producer/binding，原 exact cancellation 故障语义保留。
- [x] 默认两目标真实 PG：`delivery_service` 12/12（11.35 秒）与 `output_lifecycle` 6/6（5.62 秒），exit 0。日志 `run/output-lifecycle-pg.log`。
- [x] 最后 cleanup-only 调整后，受影响单项再次通过（2.58 秒，exit 0）；日志 `run/output-lifecycle-fence-pg.log`。唯一测试数仍为18，不将重跑累加。
- [x] 两份测试文件冻结；只读审查及生产修复分工已记录，共享测试尚由 Root 统一提交。
- [x] 本代理测试 schema 已清理。另按 Root 授权核对其 SIGABRT 后差集，仅删除2个新增 pipeline schema；原3个历史schema保留，其他客户端连接0。PG窗口已释放。
- [x] 2026-09-22 的 Pipeline stack overflow/SIGABRT 已于 2026-09-23 定位并修复，见下方栈分析和默认栈运行证据；原中止日志作为失败证据保留。
- [ ] 用户恢复后完成 A/C/Root 剩余 actor/live/paired transport 验收。
- [ ] 完整组合源码冻结后的 workspace/特性与真实端到端验证；不能将以上定向结果称为全workspace通过。
- [ ] Root审阅dirty归属并完成正常提交；原账本树6项Core-only临时覆盖继续排除。

## 2026-09-23 独立只读审查

- [x] 读取 Root runtime/output lifecycle HANDOFF、失败日志及整合 checklist；未启动 PG、测试或共享 target 构建，未修改 Root Pipeline/DeliveryService 源码。
- [x] 对原失败二进制仅执行 `lldb target create` 和 `disassemble`，没有运行被调试进程。证据位于 `run/lifecycle-review-20260923/stack-prologues.txt`，二进制 SHA256 位于同目录 `stack-binary.sha256`。
- [x] 原 binary 的 debug poll 栈入口明确分配：`process_text` 1,203,648 bytes、`process_event` 355,840 bytes、`execute_llm_response` 218,784 bytes、测试 `respond` 107,904 bytes、large-chunk 内层测试 117,680 bytes。常规嵌套 poll 链这些帧合计 2,003,856 bytes，尚不含 runtime、fixture、select 和 stream 子 poll；失败用例的 12 KB 文本本身为堆上 `String`，不能把 UTF-8 分割当作已证实根因。
- [x] Root 将 `cancellable_stage` 改为普通构造函数，在构造 wrapper 前把原 Future 放到堆上，保持同一 Future/结果所有权。独立测量五帧合计 2,003,856 → 632,096 bytes（减少 68.5%）。Root 默认栈单项 1 pass（3.54s）及原 Pipeline 套件 20 pass/0 fail/0 ignored（7.78s）；未增大 `RUST_MIN_STACK`。测量二进制和随后实际运行二进制不同，哈希、归属与日志已分别记录到 stack README；后续新源码仍需复验。
- [x] **Bind 在设备 ACK 前取消的 Response 身份盲点**已真实 red→green：保存源码中 `bind_output` 在 `register_output(...).await` 成功后才设置 `Inner.registration`；`supervise_output` cleanup 仅在 registration 已存在时写 Response cancellation。Pipeline `prepare_response` 也在 `output.bind(...).await` 成功后才设置 `turn.response_id`。Root 将本地候选身份与设备登记分离，在 gate 锁内固定不可变候选再 enqueue；cleanup 从数据库验证完整候选后才取消，不信 caller clone。Pipeline 在 prepare 后立即保存规范 ID；service.cancel_response 用已核验的规范记录匹配候选。
- [x] Register ACK barrier 的真实 PG 回归：原 Response prepare → Bind 已进入设备但 ACK 未返回 → retire 或 3 秒语义期限到期；另加同一次 poll 中 enqueue Bind → retire 的更早窗口。三项旧源码均因缺少持久 Response cancellation 失败（8.50 秒，exit 101）；修复后通过。断言原 Response 持久取消、无 reserve/媒体、新 Response 或凭空控制登记。
- [x] 另外两项覆盖 `service.cancel_response` 在 Register ACK 等待期间结束 Bind；以及伪造另一 request 的 Response clone 不得改变数据库原行，shutdown 必须明确报告身份不一致并完成 join。
- [x] 三项新测试 `cargo test -p alia-server --test output_lifecycle --no-run` 编译 exit 0（1.54 秒）；仅编译，不计实际用例通过。日志 `run/validation-20260923/lifecycle-bind-compile.log`。原六项测试含义不变；fake stop 使用入场的原 scope 断言，允许测试登记 ACK 前与登记前的 fence。

栈证据的集中入口为 `run/validation-20260923/stack-analysis/README.md`，包含可重复的完整命令、原地址和各 prologue 分配算式。

- [x] 最终真实 PG `cargo test -p alia-server --test output_lifecycle -- --nocapture`：**11 passed / 0 failed / 0 ignored**，6.49 秒，exit 0。6 项既有 + 5 项新增，不累计中间 10 项重跑；旧 `delivery_service` 12 项本阶段未复跑。四份源码前后 SHA256 一致，命令、日志、全部尝试和清理见 `run/validation-20260923/lifecycle-result.json`。
- [x] 测试结束 namespace 与入场 baseline 完全相同，仅原 3 个 `pipeline_delivery_*`；`delivery_test_*` 无残留、其他客户端 0，没有额外 DROP。PG 已交 C 单项诊断后再交 A。
- [x] 新 typed output retirement 契约下，三个 fake 设备明确校验原 scope/binding/backend 并等待各自软件 stop barrier，返回 `NoNativeOwner` 或原 backend 的 `Settled`；不依赖 trait 默认 noop，也不新增/伪造 media progress，原 Unknown/ACK 故障断言保留。
- [x] Pipeline prepared-but-unacknowledged 错误 guard：旧逻辑真实 PG body 失败，错误处理创建第 3 个 Context（原 2 个）；新 guard 使用可信 `turn.response_id`，未等设备 ACK 就固定同一规范 Response。回归同时断言 Context 集合数量不变、原 Response/Context/request 不变、原 Response 持久取消、无替代 Response/文本/音频，任何 TTS 调用直接失败。red 日志 `pipeline-error-guard-red.log`（3.69s）。
- [x] 新契约下实际运行：Pipeline **21/21**（8.13s），DeliveryService **12/12**（11.34s），OutputLifecycle **11/11**（6.38s），均默认栈、0 fail/0 ignored。PG 入场/结束 schema 相同，仅原 3 baseline、其他 clients 0，无活动本代理 exec/PG。完整命令、binary hash、结果见 `run/validation-20260923/retirement-lifecycle-result.json`。
- [ ] 这 44 项不能称为统一冻结验收：Pipeline 前后 10 份相关源码一致，但 lifecycle 阶段后 Core `output_control.rs` hash 从 `ba4718…` 变成 `610b83…`，其他 9 份一致。C 报告该差异来自加入类型后的 rustfmt，未做后续 Core 逻辑/测试修改，但无格式前字节快照可独立证明；仍保留漂移限制。Root 随后的有界 failure-reporting 修改亦需最终整合冻结验证覆盖。

本审查报告给 Root 的栈大小来自修复前二进制；其后源码改动不自动继承这些数值或历史 18 项通过结论。

- [x] 完成新增 typed RetireOutput / 原 output supervisor 的独立只读审查：核对 candidate 与设备 ACK 身份分离、late Register admission token、原 scope 与 replacement 隔离、NoNativeOwner/Settled 区分、超时后同一 fence Future 保留、退休错误与 join；未发现新的具体阻塞项。没有为本次审查扩改源码或执行测试。
- [x] 当前本代理没有活动 exec、构建或 PG 窗口；最新可继续交接为 `run/output-lifecycle-handoff-20260923/HANDOFF.md`，保留 44 项定向通过与非冻结限制。
- [x] Linux ARM64 只读准备完成：现有两套 cache、固定本地 arm64 镜像及依赖脚本已核对；最小矩阵与路径清单位于 `run/validation-20260923/linux-preparation/{README.md,RESULT.json}`。未新建/启动容器、访问 PG 或构建。
- [ ] 等待 Root 最终冻结 source snapshot 再执行 Linux ARM64；旧缓存或历史通过不计入本轮验收。

- [x] Root 全workspace暴露的孤立 precise-cancel-panic runtime fixture 已迁移到真实规范 voice Response + 原 producer/bind + 有类型 scope 退休。原全部 panic/native所有权断言保留，并在 scope 退休完成后继续证明精确取消隔离仍保留媒体 future。Mac exact 1/1（4.63s），完整 ServerRuntime 43/43（57.01s）；PG baseline 相同、clients 0。证据 `run/validation-20260923/runtime-panic-fixture/RESULT.json`。
- [x] Linux V1 非PG已实际执行：Core27/Proto4绿，两端checks绿，Server affected链接绿；Embedded163/2失败准确保留为旧fixture复用retired scope契约差异。605冻结文件无漂移，仅本轮V1容器已清理；旧缓存未挂载可写。
- [x] Linux V2（605文件，manifest `1939a948…`）约定矩阵完成：**325 passed / 0 failed / 0 ignored**；Core27/Proto4、Embedded完整无默认lib165、Pipeline21、Voice23、Delivery12、paired6、output_control13、output_lifecycle11、ServerRuntime43。Server default与两端no-default all-targets均通过。冻结snapshot与源码提交 `5810687…` 的605文件运行后完全匹配。
- [x] Linux V2 PG全部schema与baseline字节相同、clients0、无DROP；窗口已交Root最终Macworkspace。只清理本轮V1/V2容器，旧/新缓存、模型、PG与证据保留。B无活动exec/build/test/PG，不再启动测试。结果 `run/validation-20260923/linux-final-v2/evidence/result-summary.json`。
