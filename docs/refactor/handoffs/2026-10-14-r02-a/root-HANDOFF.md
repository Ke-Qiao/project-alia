# Root — R02-A 双流证据与离线重放交接

日期：2026-10-14。分支 `codex/architecture-refactor`；起点 `f372ad3b4a7fc87de2526840fcada461080c20dc`；实现提交 **`e557ceb962610004287f38e66b50abca3e44b661`**。实现与最终交接记录分别提交；三个 subagent 已停写，验证进程已全部退出。

## 范围与分工

本轮只实现有界软件证据与合成 PCM 离线重放；没有真实录音、播放、硬件连接、AEC、依赖变更或部署。三个 subagent 分别负责 native 双流与单消费者、显式会话与导出重放、独立审查。Root 负责旧配置/调用方清理、统一验证、文档和提交。

现有 post-mix reference、native timing、固定块队列和精确取消继续复用。旧 AudioDebugRecorder 的 capture/wake 接口没有生产调用，唯一 local-cue 写入发生在 native 接受之前；源 PCM WAV 不能作为最终播放证据。旧逐文件 spawn_blocking 不保留 join，缺少会话字节上限和完整关闭，因此整条旧路径已删除，包括配置、调用参数、fixtures 和文档入口。

## 交付行为

- CPAL input callback 和 ALSA 成功 readi prefix 在转换/增益之前进入既有 64 × 1024 样本队列，与最终 native playback reference 共用有界证据通道。整批 capture 时钟只观察一次，拆块不会重复推进 native cursor，也不改变现有 speech CaptureTiming。
- bus 不再公开 try_read；不可 Clone 的 AudioReferenceReader 以原子 claim 强制单消费者。finish、取消或 Drop 只释放读取资格，不能关闭 native stream、取消真实播放或清空队列。
- 成功发布在 queue try_lock 内先观察 loss、再分配 sequence，保留记录顺序与 loss 前缀约束；失败尝试仍计数，不等待消费者。独立 error callback 可以穿插先前观察的 PCM，不重写原 timing；重放将歧义保留为 reset/gap/unknown。
- 显式 EvidenceSession 提供 start/pump/finish，最多 512 records、每条 1024 samples、总计 524288 samples 和 16 MiB JSON，调用方只能收紧。导入先限 bytes，再限数组，最后验证身份、格式、序列、时钟与样本；没有自动会话、文件写入或分离后台任务。
- 重放逐条返回实际记录与分方向 reset 原因，不播放、不补零、不假定采播对齐；调用方仍须检查 footer 中的尾部 loss 和最新流状态。合成示例执行导出、真实读回和 replay，并以 persist_noclobber 拒绝覆盖现有文件。
- 实际 load_config/Settings 路径拒绝旧 audio_debug TOML/JSON，即使 enabled=false。底层通用 EmbeddedConfig serde 仍宽松，不宣称所有直接反序列化入口都拒绝未知键。忽略跟踪的本地 config/embedded.toml 未修改；下次启动前需删除旧表。

## 复现与验证

RED 源码清单冻结 657 个输入，SHA-256 `e520b96fb716df80ba7c5d5c4c2e9a7975b987495785d95c7bab2d55a96a4a78`。native capture 仅等价抽取原 observe 调用以让非零合成样本经过实际生产入口，不先补 PCM 发布行为。旧 API 的 clone 竞争和旧配置接受分别由独立用例复现。原始证据从 run/validation-20261014-r02-a/ 按原字节归档至 [evidence](evidence/evidence-index.json)，不删除失败日志。

GREEN 冻结 659 个输入，manifest SHA-256 **`676d2e76fccd23ac78003a6d6cbc8258a1c1faeba18e8c8cf1d02da8b246bda3`**。运行结果均 source_drift=[]，提交后的 659 个 Git blob 与 GREEN 清单逐项相同，详见 [提交核验](evidence/commit-verification.json)。

| 检查 | 实际结果 |
| --- | --- |
| RED 旧配置、native capture、clone 偷读 | 4 个预期业务断言失败；不是编译失败 |
| 首次 all-targets 沙箱运行 | lib176通过/42失败，均为本地 loopback fixture bind EPERM；原日志保留 |
| 同源、同命令允许 loopback 后重跑 | cargo test -p alia-embedded --all-targets --offline --locked，58.43秒，lib218 + integration66 = **284通过/0失败/0忽略** |
| 关键软件回归 | evidence13、reader/queue10、native clock5、PCA14 均通过，原播放精确取消与资源清理测试包含在 all-targets 中 |
| 合成导出→读取→重放 | 7 records、16 samples、9 resets；latest playback closed、尾部 loss 各2、latest capture running、被替换 capture 最终未知 |
| 再次导出同一路径 | 预期退出1/File exists；5137 bytes 原文件哈希不变，无临时文件残留 |
| 格式 | Embedded fmt通过；workspace仅报未改 Server tests/server_runtime/delivery.rs 的历史 import 顺序 |
| 独立审查 | reviewer 9/9，无阻断项；native/evidence owner 各8/8 |

命令、环境、时长、退出码与限制见 [验证摘要](evidence/validation-summary.json)。合成文件 SHA-256 为 `374d84a56124ab88aecf40aaaa8225c2df6f813b3c860e9061efa953a38d1b49`。全仓格式差异的原文件哈希与基线核对记录在 preexisting-format.json；没有为此扩展 Server 修改范围。

当前宿主 macOS 26.6.2 ARM64、Rust 1.95.0；只读 `orbctl status` 仍为 Stopped，未启动其他机器或容器。Linux 不能以历史通过代替当前验证。

本轮未启动外部数据库、SSH、容器、录放音或硬件；测试仅用本轮临时文件/SQLite、loopback 和模拟后端。Cargo 与示例均已结束，没有需要继续等待或清理的验证进程。

## 保留边界

采集记录是后端实际交付的 native 格式数据，可能已被设备固件处理，不宣称原始 ADC、未处理麦克风或真实声学对齐。归一化 f64 保留其自身 bits，不承诺全部 i64/u64 原始精度。采集/播放各自独立时钟；未知 queue delay 保持 None，缺失参考不补零。

观测会话只持有唯一读取权，不取得音频取消或设备关闭权限。流快照只代表最新流；旧流最终快照未观察到时必须保持未知。操作数量和内存有界不代表底层同步文件 I/O 的墙钟耗时有界。

没有自动文件录音不等于运行采集时没有内存 PCM；native 队列会保留有限数据。ObservedNative/SyntheticFixture 是来源声明，不是认证。活动快照和 loss counters 是独立观察，不是原子事务；只有 closed 快照具有对应的最终计数可见性。

## 进度与恢复

[本包 checklist](../../checklists/r02-a-2026-10-14.md) **10/10**；R03-0 仍10/10，L09—L12仍4/4，R01—R06仍 **2/6**。本包不关闭 R02 声学或 R03 物理验收。

下一软件项为 **R03-A 可信动作资产内容身份**，从 [最新恢复检查点](../../resume-checkpoint-r02-a-2026-10-14.md) 继续。R1 尚未接入、扬声器/功放和电气参考未定；执行器校准、供电/OE/限位与保持结果仍需现场资料。Settings 剩余等待/取消实证、V01、R05长时负载、额外平台及未来部署形态继续保留，不以本轮软件 GREEN 关闭。
