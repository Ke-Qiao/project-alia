# Duplex paired playback verification

**2026-09-22 暂停检查点：用户因额度不足要求暂停。本子包保存未提交，5 项配对测试只完成编译和链接，尚未执行 PostgreSQL / gRPC 运行测试，不能称为通过。** 主任务应从此处继续，不能将已提交 C 包的 59 项结果计入这里。完整交接：`run/duplex-playback-handoff-20260922/HANDOFF.md` 与 `RESULT.json`。

此子包在 DU01 integration 工作树验证 Server 与 Embedded 的实际 gRPC 组合，不替换任一端的传输实现或控制状态机。上层使用真实 `DeliveryService` / `OutputProducer`、Context / Response 与 PostgreSQL 随机 schema；Server `EmbeddedChannel` 走真实初始化、签名认证和 Tonic client；Embedded 使用生产 `PlaybackService`、`PlaybackSession`、`PlaybackControl` 和 `AudioSubmission`。

- [x] **DP01 可注入 backend 契约**：公开 `PlaybackControl::{new, apply, retire}` 与 `AudioSubmission::with_control` 四个已有方法，并说明必须保留原 native generation 的控制 Arc。外部公开 `AudioPlaybackBackend` 因而能合法实现带控制门的 backend；没有新增测试旁路或 feature。
- [x] **DP02 实际链接准备**：Server dev-dependency 引用 `alia-embedded`，`default-features=false`。Cargo.lock 仅增加这个已有 workspace 包的依赖边，无新外部依赖。`cargo check -p alia-server --test duplex_playback --locked --offline` exit 0；`cargo test -p alia-server --test duplex_playback --no-run --locked --offline` exit 0，已实际链接可执行文件但没有执行测试。日志分别为 `run/duplex-paired-check.log` 和 `run/duplex-paired-link.log`。
- [ ] **DP03 首次 Start 前暂停**：Response 先绑定，ExplicitWait 期间媒体租约保留；超过实际 5 秒握手期限仍不创建 Push；fresh 明确继续后同一媒体只创建一次 Push，最终只有真实软件提交前缀。
- [ ] **DP04 满队列控制**：native 软件队列以 Condvar 精确阻塞 enqueue；Hold 独立到达且阻止原输出门，恢复后逐样本验证原 PCM 顺序、scope 与单一 submission，没有重播。
- [ ] **DP05 End 后尾部**：已接纳 End、未提交 tail 时暂停，保持超过 5 秒 media idle；原回执继续有效，恢复后提交原尾部，不生成 Presented。
- [ ] **DP06 控制 ACK 丢失**：薄代理先执行真实服务命令，再返回 unavailable 丢弃 ACK；原 actor 通过真实 Observe 查询同一 command，并持久化回执，未获恢复许可前保持 Held。
- [ ] **DP07 注册晚回执**：薄代理延迟真实 RegisterOutput 回复；Fence 在它返回前撤销 scope，晚 ACK 不能填回 Server 本地 cache，重试原绑定仍拒绝。
- [ ] **DP08 运行与冻结**：实际 PostgreSQL + loopback 五项通过，记录 exit / 测试数 / 日志 / 源文件摘要，窗口归还后无活动测试 owner。

唯一 native 替身是测试内有界 PCM 队列。入队和回执仍使用生产 `AudioSubmission`，模拟 callback 明确取得 `try_begin_output` 后才 `finish` 实际计数。它验证软件边界、原队列和取消所有权，**不验证声卡已播放、扬声器已静音、AEC 或声学效果**。

测试路径为 `packages/alia-server/tests/duplex_playback.rs`，复用已有 `delivery_support` 的真实 Context/Response fixture，不用伪造 Response UUID 绕过准入。fixture 用私有 SQLite 绑定状态与临时签名密钥；只请求 loopback 端口。正常结束、断言 panic 和 body timeout 都释放阻塞条件，停止并 join 两端 owner，再关闭 listener；初始化错误也有 listener guard。PG schema 由现有 fixture 清理，不访问用户数据库或既有 schema。

两条 5.5 秒等待专门验证超过实际媒体/握手期限的行为；状态竞态使用 semaphore/Condvar 屏障，不靠概率等待制造。其它状态查询有 3 秒上限，单 case body 上限 32 秒，外层数据库 fixture 上限 45 秒。

当前允许源码边界：新 paired test、Server dev-dependency/Cargo.lock、四个 Embedded backend 方法的可见性/注释，以及本检查清单。Pipeline、Server 控制接线、原有测试夹具与其它生产修改由主任务负责。

下次还需要复核主任务指出的 session replacement 清理契约：旧 output actor 的 `stop_current_playback(oldscope)` 使用 non-close minimum-generation fence；端侧已替换为新 session 时，当前 `validate_fence_receipt` 的 same-session 要求将拒绝回复。只读代码路径支持这一风险判断，**尚无配对运行复现、未新增第六条测试、未放宽生产校验**。应先用此 fixture 验证旧 native owner 已退休且新 session 不受影响，再设计仅供旧 owner 清理使用的有类型退休证据；不能把“active 已换成新 session”当成全部旧 native writes 已收尾或声卡静音的证明，也不能放宽新会话准入的 fence。
