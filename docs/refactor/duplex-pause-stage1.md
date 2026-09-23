# 可恢复播放暂停：Core 与 Embedded 本地基础

本文保留 2026-09-20 的 Stage 1 历史范围与当时的 15 秒上限；当前成对网络协议、120 秒工程上限及 Server 接线见[控制协议](duplex-control-protocol.md)和[语音协议](voice-protocol.md)，最终验收见[整合检查表](checklists/duplex-integration.md)。下文的“当前/未修改”均指第一阶段当时的状态。

状态：2026-09-20，第一阶段本地机制已实现，补齐公开回执重试契约后完成受影响检查与 v3 Linux ARM 完整补验，源码已冻结待主代理审查与提交。服务器、Proto 和当前 WAIT 网络行为未修改。本阶段不能作为端到端自然全双工恢复、物理静音、Presented 或 AEC 的完成证据。

用户已经确认两类行为：临时疑似插话避让可在新鲜语义确认只是附和后恢复；明确等待须明确继续。本包将这种区别落实为控制类型和本地执行规则，语义推断和外部权限验证由后续服务器接入。

## 本地接口

纯契约位于 [`playback_control.rs`](../../crates/alia-core/src/playback_control.rs)，不依赖网络、数据库、设备或系统时钟。`PlaybackControlState` 接收调用者提供的单调已用毫秒和实际在途写状态。

```text
ControlCommand { scope, hold_id, command_id, sequence, action }
Pause { reason: AcousticCandidate | ExplicitWait, through_sequence, budget_ms }
ConfirmExplicitWait { detection_id, through_sequence }
Resume { authorization: { kind: FreshBackchannel | ExplicitContinue,
                          detection_id, through_sequence } }
```

`ConfirmExplicitWait` 只把同一 AcousticCandidate hold 提升为 ExplicitWait，不释放门、不重建媒体、不刷新首次截止时间；以后 FreshBackchannel 必须拒绝。服务器可为初始门设置最多 15 秒的绝对保留预算，并自行管理较短的 3 秒语义阶段期限。确认明确等待只能结束那个临时阶段，不能延长设备最初预算。设备不推断这份语义结果是否真实；调用方必须提交经过验证的控制输入。

相同 command ID 必须逐字段等于原命令；同一 scope 的合法下一条命令被接纳后占用序号，动作被证明拒绝也消耗序号。精确重试返回原固定 outcome，receipt 中的 snapshot 是当前事实；不能把较旧命令的 Applied 当作当前门已开放。乱序、错误 scope、非法 ID 不推进水位。每个 scope 最多记录 32 条命令，达到上限永久退休，取消/关闭不受此限制。

`PlaybackSession.apply_control` 在本地 scope 注册仍然有效、仍持有原 PlaybackControl 时，先查询该 owner 已记录的回执；到期、采集断续或原 native generation 退休不会抹掉它。原 Applied 与原 proved rejection 都保持固定，当前 snapshot 则如实显示 Retired。同 ID 的不同内容仍冲突；未知新命令仍经过原 native admission/退休门并拒绝。查询不建立 owner、不推进序号、不执行 Resume；读取时钟只可能追加到期关门。`PlaybackSender` 的原 owner 查询遵守同样规则。scope 已关闭或被替换后，公开 Session 入口仍返回 stale；本包不新增已关闭作用域的历史 owner 档案。外层绑定授权也仍是必需条件，历史 Applied 不能代替当前权限。

Resume/Confirm 必须匹配原 hold，带非空、未被成功消费的 detection ID，覆盖触发 Pause 和最新近端 speech 的采集 sequence，且不超过实际已观察的 capture。连续静音不会使合法结果过期；新的近端语音会拒绝旧结果。原始 Backchannel 标签本身不构成权限。采集缺口或重复 sequence 使该控制 scope 退休，不伪造缺口中的静音。

[`PlaybackSession`](../../packages/alia-embedded/src/module/playback_session.rs) 增加以下实际本地入口，均由现有 `CpalAudioPlayback`、`PlaybackSender` 和 native worker 执行：

- `observe_capture_for_control(scope, sequence, speech)`：登记真实连续采集证据；不自行抑制或恢复音频。
- `apply_control(command)`：应用独立于媒体队列的控制，返回固定命令 outcome 与当前 `Running/Pausing/Held/Retired`。
- `observe_control(scope)`：观察原 owner，不创建新 owner 或续期。
- `wait_held(scope, hold_id)`：最多等待 1.5 秒原生在途写收尾；超时意味着缺少 Held ACK，丢弃 waiter 不丢弃原门或媒体。
- `control_evidence(scope)`：在控制锁内读取原 backend、实际在途写是否静止、当前控制状态及最多 32 个原 PlaybackProgress。它证明软件提交边界；Retired 自身不是原生写已结束的证明。

不支持这些能力的后端默认明确拒绝。外部接入必须持有原绑定授权的读锁，再进入 PlaybackSession；本地方法不能代替服务器对规范 Response、原 Context/privacy/relationship epoch 和实际根的复核。

## 输出、所有权与有界保留

每个 scope 的所有原有与后续 AudioSubmission 共用原生 owner 的原子门和在途计数。Pause/Resume 保留 boot/session/generation、playback UUID/sequence、backend/stream 身份、原 resampler、未提交队列和渲染游标。已取消或终结的 submission 不会因 scope Resume 复活。原生实例消失、scope 关闭、expiry 或永久 fence 均禁止同 scope 换一个 native generation 恢复。

`AudioSubmission::try_begin_output()` 返回同一次原子判定的 `Writable / Held / Retired` 事实（Rust 中 Writable 为 `Ok(SubmissionWrite)`）。渲染器不再通过之后读取的 held 位补判原因。Held 被拒后，Resume 即使立即到达，也只会在完整静音声道帧结束后重新尝试；因此既不误删 front block，也不会将左声道数据写进右声道位置。已开始的 CPAL 完整帧和 ALSA writei 的真实成功前缀仍归原身份，计数收尾后才承诺 Held。

控制不进入媒体满队列等待。后台继续持有 renderer、stream 和取消任务；worker 自行检查到期，空媒体、丢弃 waiter 或没有后续 PCM 都不能阻止 expiry。真实 condition-variable 等待比较控制状态变化；新 Hold 能唤醒已经空闲的 worker。活动/暂停进度使用最多 2 毫秒等待，空闲受控 scope 用最多 20 毫秒周期处理采集侧退休；控制及新媒体通知可提前唤醒。

新增明确的 native 保留限制：单个输入 PCM payload 最多 1 MiB，共享源 PCM 保留最多 2 MiB，渲染 PCM 保留最多 2 MiB。源块的所有分片共享一份字节 reservation，最后一片转换或丢弃才释放额度；源字节与队列条数都在复制前检查。渲染容量不足明确报错，不偷丢尾部。metadata、重采样器状态和有界转换暂存不属于这 4 MiB PCM 留存值；它不是整个进程 RSS 上限。服务器编码 payload、LLM/TTS 工作和传输各层总预算尚待后续接入，不能由此声称全链路已受同一配额限制。

同 generation 一旦接纳 identityless legacy media，就不能再建立声称覆盖全部媒体的 scope 控制；需要合法的新 generation。反过来，已控制的 scope 拒绝混入 identityless media，防止未携带原门的块绕过暂停。

## CPAL、ALSA 与参考真相

CPAL 回调继续写实际零值，保留原媒体。ALSA 第一阶段也采用软件门：在需要保持运行时向同一 PCM 提交真实零帧，保留原媒体；只记录 writei 成功接纳的前缀。没有调用 native pause 后再声称设备保留了驱动队列。

真实软件 null 查询到 `can_pause=true`，本阶段测试模式仍明确为 SoftwareGate。尚未验证或启用 `pause(true)/pause(false)` 的 native 保留契约，CPAL 的 pause/play 也未被当作无损暂停接口。受控 ALSA 出现无法证明连续性的 underrun/suspend 时终止该 owner，不通过 prepare/resume 悄悄重放原数据。

实际参考流增加 `HeldSilence` 和 `MixedHeld` 内容标识，区分有意保留媒体而输出的零值。它们由此次 native 写入/拒绝结果产生，不事后重读当前 gate 来猜测。后端 native cursor 可以因零帧增加，原媒体 submitted cursor 保持冻结；两个游标不能混用。丢块/关闭统计、独立采集时钟、gain/重采样/最终格式量化和原有 SubmissionBoundary 保持真实。已排入硬件的尾音仍可能发声，Held 不等于扬声器即时静音。

## 验证与尚未接入范围

最终 source manifest v3 包含 608 个文件，SHA-256 为 `74e181a9c02b8582a5355f8ae68675370b5bef0979dc7265dc1c4daf00793813`；本说明在源码冻结后更新，不在构建清单中。Cargo.lock 未修改，SHA-256 为 `0736653b5c57a936dc71d2b18a82b927536acb7bb8c49341f845717c87fde2a2`。

本阶段 11 个新增 Embedded 控制测试覆盖完整声道帧收尾、后续同 scope 句段、满媒体队列、source/render 字节边界、真实渲染入口的量化参考、ALSA 部分前缀保留、明确等待转换、waiter 丢弃、原 native owner 取消、重复/旧命令和两个确定性并发屏障。一个屏障固定“得到 Held 拒绝 → 并发 Resume → 处理原拒绝”；另一个在真实 condvar 持锁时证明“先进入 idle → 新 Pause → 无媒体及无 waiter 到期”。

公开回执回归先在真实 capture gap 使门退休后失败，错误为 `playback control scope was permanently retired`。修复后的同一公开 Session 测试覆盖 capture gap 与精确 native generation stop 两条无睡眠路径：Applied 和 proved rejection 均可查询、篡改同 ID 冲突、未知命令拒绝，原 generation、媒体留存字节、提交前缀、水位与 owner 身份均不会因查询改变；scope close 后不创建新 owner。v3 修复只改四个跨平台 Rust 文件，受影响检查为 Core 25 项通过、无默认 feature 播放测试 36 项通过、默认 feature Session 测试 5 项通过，以及 macOS 无默认 feature all-targets 检查通过。默认测试实际编译链接 Sherpa 原生库。

v3 冻结快照随后完成 Linux ARM 全量补验：`cargo check -p alia-embedded --no-default-features --all-targets --offline --locked`、`cargo test -p alia-embedded --no-default-features --lib --offline --locked -- --show-output`、`cargo test -p alia-core --no-default-features --lib --offline --locked -- --show-output` 的退出码均为 0；Embedded **142 项**、Core **25 项**通过，包含新增公开回执回归。固定镜像仍为 `rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1`，实际 ARM64 image ID 为 `sha256:7cf1e580ef5539f03b58560753e8ab84c8c360960d99dff714004aa98f203977`。608 文件源码快照与 Cargo registry 只读挂载，target 为容器私有；Cargo 依赖离线，protobuf/ALSA/DBus/pkg-config 原生构建包与 v2 相同，安装于任务容器内。libasound2 实际版本为 `1.2.8-1+b1`。真实 ALSA null 输出仍是 `can_pause=true; tested mode=SoftwareGate`，不是 native pause、声卡、树莓派、Presented 或声学恢复的验收。容器 `alia-pause-stage1-v3-20260920` 退出码为 0，已自动删除并以空容器查询结果复核；未访问 PG。

完整平台验证对应此前 source manifest v2：`e9e419e298213d3454e27708126fb434ae07d19ea41945d5e7bbe6015111ac86`。该快照的 Core 25 项、macOS 默认与无默认 feature 库测试各 134 项、Linux ARM 无默认 feature 库测试 141 项均通过，两端无默认 feature all-targets 检查也通过。默认配置实际链接 Sherpa v1.13.1 arm64 原生库，复用已核验本地归档（SHA-256 `ca7baf61f331ea6b848e3630e566647302c26960c7303932bc33125074871eeb`），未用 DOCS_RS 跳过链接。Linux 镜像固定为 `rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1`，源码/registry 只读，target 为任务容器私有；实际 libasound 为 1.2.8，两只任务容器均已自动删除并核实无残留。上述 141 项保留为 v2 历史结果，v3 完整补验以单独记录的 142 项为准。

日志和冻结清单位于本工作树的 `run/duplex-pause-validation/`。v3 受影响检查以 `run-pause-receipt-{core,playback,default-session,check}.log` 为准，red 保留在 `run-pause-receipt-red.log`。v3 Linux 完整命令、环境、退出码与限制分别记录在 `evidence-v3/{invocation.json,run.sh,result.json}`，原始输出在 `evidence-v3/{linux-check.log,linux-lib.log,linux-core.log,result.txt}`，结果为 `check=0`、`embedded=0`、`core=0`。v2 完整平台日志为 `run-pause-default-lib-final.log`、`run-pause-no-default-lib-final.log`、`run-pause-check-final.log` 和 `evidence-v2/{linux-check.log,linux-lib.log,result.txt}`，Linux result 为 `check=0`、`test=0`。早期沙箱内 loopback 测试因监听权限失败的日志，以及最后 idle 等待修复前 Mac 133 / Linux 140 的结果，仅作过程记录。v3 构建清单文件已逐字节校验，源码未漂移，Cargo.lock 摘要保持相同。

下一阶段必须把 scope 关联原规范 Response/Context，接入真实采集水位、语义决定与双向新协议、持久命令 CAS、输出/鉴权/断连联动以及暂停期间的媒体 idle 分支。当前 WAIT 仍提升 generation 并取消旧回答，没有端到端 Resume。没有 PG 暂停控制账本验收、Windows、真实声卡、树莓派、AEC 或声学无缝恢复结论。
