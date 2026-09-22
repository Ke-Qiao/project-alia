# 可恢复暂停：成对协议与 Embedded 控制接入

2026-09-20 阶段交付。基于 Stage1 `213d5ede62fc714c028f0f009730ef060f424bf8`，本包只实现 Core/Proto 与 Embedded。服务器的持久控制账本、原 Delivery owner、语义路由、生成/TTS 背压及端到端集成仍未完成，因此这不是自然全双工端到端恢复的验收。用户要求额度收口后停止新增功能，后续从持久交接继续。

## 成对升级的契约

`alia.playback.v3` 与 `alia.voice.v2` 必须成对升级。没有旧协议兼容双栈、额外 feature 开关或新的 registration UUID。

- `OutputBinding` 固定 exact `VoiceScope`、规范 `response_id`、`context_id`、`server_instance_id`。
- `RegisterOutput` 只登记当前本地 Ready scope；精确重复返回原 backend 与能力。同 scope 更换任一 binding 字段拒绝。Start 必须携带此前登记的精确 binding，不能创建登记或替换已消失的 native owner。
- `BoundControlCommand` 包含 binding、Stage1 `ControlCommand` 与完整预期 `SubmissionBackend`。使用锁定的 serde_json 序列化固定类型，加 `alia.playback.bound-control.v3\0` 前缀后计算 SHA-256；命令身份、预算及完整 backend 都参与摘要。
- `Control` 与 `ObserveControl` 独立于媒体队列。后者只查询精确原命令；未知返回 NotFound，同 ID 改意图冲突，不能执行未知请求、创建 owner 或以传输错误伪造 Rejected。
- `BoundControlReceipt` 同时携带原 immutable outcome 与当前 snapshot。`observation_sequence` 在原控制锁下采样并递增，非零；它不是命令执行次数或续期。服务器只能用更高观察序号推进当前快照，不能把旧命令的 Applied 当作当前 Running。
- 回执包含原 backend、SoftwareGate 模式、真实 native write quiescence 与最多 32 个原 PlaybackProgress。Held 不宣称物理静音、Presented 或硬件 pause 保留。

本包把 Core 工程上限从 Stage1 历史 15 秒改为 **120 秒**。Open/Ready 显式协商 `software_hold` 与 `max_hold_ms`，不接受缺失/零/超过本地上限的值。服务器默认 60 秒和临时语义 3 秒由后续 Server 政策负责；60 秒是主代理等待用户时长回复后声明的实现假设，并非用户明确指定。重复请求不能改变首次 hold 截止时间。

## 捕获、授权与生命周期

`CaptureControlBarrier` 保存真实连续 sequence 和最新 speech sequence。实际 `Session.enqueue` 在 `try_send` 前、在与 Resume 相同的 PlaybackSession 锁内更新水位；不再只依赖网络 `sent` Atomic。Ready 前已排队的语音同样进入屏障。新 generation 从同一实际捕获 owner 的保留水位初始化；队列满、缺口、关闭会退休捕获/输出。旧 Resume 不能越过更新的本地语音。

登记、控制与 Start 遵守 binding authentication read lock → Session → native backend gate。控制 RPC 的原生写收尾等待在锁外，完成后重新认证并读取原命令。丢弃 RPC waiter 不丢原 owner。Builder 的绑定撤销 hook 现在同时同步退休 PlaybackSession 与 Motion，覆盖 Start 尚未到达时的登记。

合法 Held 只冻结 5 秒媒体 input/EOF idle 的剩余时间。进度、鉴权、捕获和原 120 秒绝对 observation deadline 继续运行；断流、撤权、永久取消、过期及替换不能通过 Resume 复活旧媒体。

voice v2 的 `TurnEvidence.output_decision` 区分 Unchanged、PauseRequested、ResumeRequested、Cancelled。这些是语义请求，不是设备回执。仅 Complete + Unchanged + 严格增加 epoch 允许 `response_enqueued=true`；Pause/ResumeRequested 必须同 epoch 且不入队；Cancelled 必须增加 epoch 且不入队；Unchanged 且不入队必须同 epoch。只有严格增加 epoch 才 fence。相同 epoch 的 Evidence 不操作本地控制门，ResumeRequested 本身不能恢复 Held。

## 已验证与边界

本工作树 macOS ARM64 的离线锁定验证：Core **27** 项、Proto **3** 项通过；Embedded 默认与无默认 feature 的 all-targets check 均通过；两种配置的完整 lib 测试各 **145** 项通过。默认配置真实链接本地 Sherpa v1.13.1 arm64 静态库，归档 SHA-256 为 `ca7baf61f331ea6b848e3630e566647302c26960c7303932bc33125074871eeb`，没有用 DOCS_RS 跳过链接。

新增覆盖包含真实签名 playback loopback：满媒体队列独立控制、Pause-before-Start、End 尾部、Held 跨 idle、原 token 到期、断连、丢失 ACK 后 Observe、binding/backend 错配、未知 Observe、捕获新语音/断续/队列失败、关闭与替换。voice loopback 用受控延迟 Ready 验证真实预排队捕获，以及新 generation 水位继承；另有新语义 epoch 组合矩阵和 Evidence 不能释放 Held 的断言。120 秒边界通过纯 Core 时钟或 Tokio 虚拟时间验证，没有真实等待 120 秒。

完整命令、退出码、耗时在本工作树 `run/duplex-control-validation/results.json`，各目标日志同目录；详细恢复顺序、源码摘要、未完成事项在 `run/duplex-control-handoff/HANDOFF.md`。初期编译失败仅为新协议测试字段/语法接线，后续均已修复并保留原日志。最终整库测试之后仅对 Core `lib.rs` 的两个 `pub mod` 声明排序，另复跑 Core/Proto；测试前后 manifest 均保存。

尚未执行本包的 Server 编译/集成、完整 workspace、Linux ARM64、真实声卡/Pi 5、AEC、声学连续性或 native pause 验收。必须由后续包把原 Response/Context 来源、政策/epoch 撤权、持久命令身份与原 Delivery dispatch owner 接通后，才能声明端到端恢复。
