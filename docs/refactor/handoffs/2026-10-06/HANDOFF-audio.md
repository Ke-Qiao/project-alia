# Audio handoff — 2026-10-06

负责人：`audio_readiness_1004`。本轮负责原生混音、scoped 本地提示及对应清理；共享 PlaybackSession 契约由 Root 整合，持续语音入口由 speech agent 整合。

总体进度、冻结版本、测试命令和最终数字以 [本轮 checklist](../../checklists/cleanup-2026-10-06.md) 为准，由 Root 更新；本文件不声明整轮验证已通过。

## 已完成

- [x] 新增实际进入 CPAL / ALSA 输出路径的两路 mixer：Voice 增益 `1.0`，LocalCue 增益 `0.2`，最终整数限幅。配置输出音量先作用于各路，再混音和限幅；没有新增背景音乐或独立 mixer 插槽。
- [x] 两路分别保留原始 `AudioSubmission`、重采样器、队列水位和帧计数；按源取消，不把旧 PCM 改标为新 owner。渲染 worker 交替服务两路，避免长语音队列阻塞短提示。
- [x] 删除生产路径的裸 `play` / `play_if_current` / `stop_playback` 入口；旧测试改用带 scope 的 fixture，保留必要的 scope 生命周期入口。
- [x] 本地提示接入 `PlaybackSession` cue admission。WAV 完成读取后仍须通过原始 Ready scope 校验；cue 使用独立身份，不消耗远端序号，也不计入 canonical Response progress。cue-only scope 仍能证明原 native owner 已收敛。
- [x] 本地 WAV 限制为普通 PCM16 文件、最多 4 MiB / 5 秒、合法采样率与完整声道帧；多声道取均值。读取之后，提交、End 与 native 终止观察共用 7 秒 deadline；超时精确取消并等待 producer 退出，包括队列已满的情况。
- [x] 删除已无调用的 direct-response 包装函数和 `pcm_duration`。删除三个空壳 features，默认直接启用 `sherpa-onnx`；下载脚本移除 SenseVoice，仅保留 KWS / VAD。同步约定的四份当前架构、模块和运维文档，历史证据保留。

主要代码：[mixer](../../../../packages/alia-embedded/src/module/mixer.rs)、[native playback](../../../../packages/alia-embedded/src/module/cpal_audio/playback.rs)、[local cue worker](../../../../packages/alia-embedded/src/module/direct_response_playback.rs)。

## 混音、暂停与 reference 边界

- 两路共享原 scope 的输出 gate。接受 Pause 时同步取消 cue 的剩余部分，因此快速 Pause / Resume 也不会重播过期提示；Voice 保留原 owner 和可恢复尾音。撤权与 scope retirement 必须等待两路写入收敛。
- CPAL 在完整交错声道帧边界结算两路写入；取消一个来源不破坏已进入写入的完整帧，也不取消另一个来源。ALSA 按实际接受的短写前缀结算两路；失败写入不增加已提交帧数。
- Reference 来自最终混合后的 native PCM。CPAL 对应回调实际填写的输出；ALSA 仅记录实际接受的前缀。它们证明软件/native 提交边界，不能据此宣称扬声器已发声、用户已听见或声学回声已消除。
- Reference 保留 native frame / clock 信息；capture 与 render 的不同 clock domain 不能直接相减。两路 mixer 不是 AEC，也没有实现自适应声学延迟或时钟漂移补偿。

## 本代理实际检查

- [x] 只读核对完整 native render / reference / hold 路径及旧入口调用图；新增覆盖两路混合、限幅、精确取消、立体声帧、快速暂停恢复、ALSA 短写、独立重采样历史、cue 先于 Response、cue-only retirement 和满队列超时的离线回归。
- [x] 读取 Root 的 `run/validation-20261006-cleanup/workspace-v1.log`，定位 reference fixture 的 `sample_ns` 错值，并恢复为 `1000`。新增测试的最终整轮结果仍由 Root checklist 记录。
- [x] 对下载脚本执行 `bash -n` 与 `zsh -n`，均通过；对指定修改执行 `git diff --check`，通过。检索指定当前文档、Cargo 和脚本，旧 feature / SenseVoice / `server_turn` 描述已清理。
- [x] 未自行运行 Cargo / PG，未执行下载脚本，未连接设备或执行真实录音、播放、执行器动作。

## 后续 checklist

- [x] Root 已完成最终 macOS workspace、当前 Linux 和所选 feature 验证，在总 checklist 记录冻结输入、结果及未覆盖范围。
- [ ] R02：确认实际麦克风、扬声器和音频设备配置，取得真实 capture / render 时钟、缓冲、欠载与重连证据；当前离线 fixture 不替代硬件验证。
- [ ] R02：基于最终 post-mix reference 实现真实 AEC，明确帧适配、声道与采样率转换、延迟估计、时钟漂移和 discontinuity / reset 策略，再验证双讲与误打断表现。不在 mixer 内假称已有回声抵消。
- [ ] 硬件声学验证后再评估固定增益和限幅的听感；本轮没有新增动态 ducking、更多来源、背景音乐或端上 Easy-Turn 部署。
