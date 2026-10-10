# L11 Root HANDOFF

基线 `f15db66d9bbf99552e61572587d15dbeb8fa1535`；分支 `codex/architecture-refactor`。三个 subagent 的实施与交叉审查已交回，Root 持有后续修改与唯一统一验证窗口。本轮 checklist 已完成 10/10；最终同源验证与任务资源关闭完成，源码和文档分别正常提交。

## 结构与原子性

Interaction `ResponseCommitter` 拥有规范 Response/history 用例；Bootstrap 创建一个 Arc 并注入 ModuleRegistry、InteractionServices、Companion LiveOutputs。Conversation 普通历史端口删除跨域提交方法。生产事务体除显式数据库绑定和 Delivery 关联调用外与基线一致。两秒 ACK 不确定性、不可重新创建已擦除历史的重试语义不变。

Companion 的 Shadow 和 Live repository 继续分别提交原事务；Live 只有成功 commit 后构造一次性 Fresh permit。Context 提供 appraisal authority 与规范证据投影，Companion 构造 observation/erasure roots 并校验 private grant 与 Companion 政策。Context 私有 lease 签发、证据合并及引用适配保持；没有公开构造器或新增事务框架。

Root 删除无人调用的 Delivery `set_response_status_in_transaction`，收窄内部 response 查询；两个有效事务参与函数使用传入 tx，无多余 repository/pool 实例。全调用者和外部 fixtures 已迁移。守卫增加旧路径、Conversation 越界提交、Context 越界政策/投影的负例，并允许必要的证据适配。

## 验证过程的修正

- V1 的静态审查发现 `private_readable` 有三处 Shadow/Live 消费，恢复原 pub 字段范围；ContextLease 字段封装从未改变。V1 未运行编译或测试。
- V2 集成编译发现 DeliveryRepository 测试导入遗漏和一处不再使用的测试实例。Root 修复时曾误删另一处仍使用的实例，第二份编译日志记录该错误；最终恢复正确实例后冻结 V3。V2→V3 仅两份测试文件变化，生产源码保持。
- 交叉审查发现端到端故障测试可能提前失败而空过，冻结前已给 SQL 触发器加入非事务 sequence，并断言 `is_called`，证明真实命中故障点。

V3 两端全量均仅新增故障回归失败：async move 未捕获模型观察 receiver，mock 发送时遇到 SendError。V4 显式读取并校验模型输入，使 receiver 保持存活；SQL sequence 的故障到达断言保留。V3→V4 只改一份测试文件，639/640 输入不变，生产代码没有变化。

最终 V4 输入 640，manifest `520ec684c2619e775389330adbe837113edcb675e2f7d62fc7f5d65758fe274f`。原始失败、修正差异、静态比较及最终运行记录按实际字节保留。最终计数、源码身份与资源关闭见下方 Root 最终验收。

## 下一轮恢复步骤

1. 先读本目录四份 HANDOFF、L11 checklist、恢复检查点及实际 Git 状态，不重做已完成的 L11。
2. 继续 [L12](../../checklists/layering-next.md)：检查 `delivery/service.rs` 与 `service/media.rs` 对 `channels::adapter` 的真实类型和运行依赖，追所有实现、调用者、fixtures，再决定契约所有者。
3. 检查 `delivery/service/output.rs` 对 `providers::tts::audio_budget::ResponseAudioBudget` 的绑定、取消与限额，连同实际 producer/consumer 迁移；禁止只改目录或增加空转 trait。
4. 保留 Response/Delivery 身份、精确取消、单次派发、文本/音频投递证据、预算限额和 native owned 关闭；按实际风险选择回归，继续三个互斥 subagent，Root 统一验证。
5. 完成后继续更新 checklist、四份 HANDOFF、同源证据、恢复检查点和正常提交。Skills HTTP CRUD 三处 reload TODO 是独立软件待办，不夹带进出站契约整理。

当前默认 Shadow、全部政策、双 epoch、两类暂停与已派发工具迟到事实保持。R01—R06 仍 2/6，不把 L11 换算为整体完成百分比。前端、真实模型、AEC/R02、执行器反馈/R03、V01、Pi 整机/24h/R05、其他平台/R06 与未来端上部署见延期台账；没有新增硬件或部署授权。用户允许额度不足时使用 reset 卡，本轮未使用。

## Linux V4 旧关闭测试的有界异常

首次完整 Server 验证 777/1/5，唯一失败为旧 `delivery::panicked_playback_fence_retains_original_native_future_and_dependencies` 子进程超过既有 20 秒总期限。日志已出现预期取消 panic，尚无阶段时间戳，不能判定超时发生在启动预算消耗还是后续等待。V3 同生产源码的该项通过；V4 无源码变更定向复查 1/0、16.40 秒通过。同一 V4 默认并行完整复跑 778/0/5，通过且无源码漂移。

独立审查确认该测试及 NativePlayback 之后代码、runtime fixture、Delivery supervision/output、Bootstrap runtime 与基线字节一致。child 启动会在新的进程级 OnceCell 中读取完整测试可执行文件并计算 SHA-256，这项耗时计入父 20 秒期限；它只是有源码支持的候选解释，没有分段测量，不能称为根因。原全量阶段与 macOS 验证有时间重叠，后续 Linux 复跑时 macOS 已结束；没有改 libtest 默认并行度。

后续若复现，应先记录启动、panic、native release、fixture cleanup 的阶段耗时，以及任务 PG 的等待/阻塞证据；不延长期限、不串行规避、不把复跑通过当作已修复。本项挂靠 L12 关闭证据可观测性，独立于原 V01 Companion 配对超时。

原日志还出现 Docker stdout/stderr 次序交错：下一 Cargo heading 先于上一目标的最后一条测试和汇总。旧计数器拒绝该日志；新计数器仅在前一目标未汇总、未出现新 harness、间隔仅含 test 行且汇总与原始数量相符时归回旧目标。新增正反例后 8/8 自测通过；旧计数报告和原始日志均保留，未修改测试输出。

保留测试二进制：`run/validation-20261010-l11/linux-arm64-v4/runtime/server_runtime-v4`，461182688 bytes，SHA-256 `975ee9ae4cd700f58fc3b3eb8a2f52b3b59bea841614bd1e1ea2574bdf0ba5d6`。该忽略目录中的证据文件未部署或纳入 Git；环境镜像和摘要已归档。Idle Linux holder 停止时的 137 不是测试退出码；各验证阶段退出码另有结果文件。

## Root 最终验收

源码 `60f665cf6fce4811b144dfc743b4814c5bdf19d5`，V4 manifest `520ec684c2619e775389330adbe837113edcb675e2f7d62fc7f5d65758fe274f`，640/640 Git blob 一致。macOS workspace **1060/0/5**、真实 PTY **3/0**、无默认 availability **2/0**、提前准入 **1/0**；Linux ARM64 Server 完整默认并行复跑 **778/0/5**，两端无默认全目标及 Linux release/native link 通过。所有最终结果同源且无漂移。

本轮 10/10，L09—L12 3/4，R01—R06 仍 2/6。四个任务容器均停止。停机前 schema/其他连接均为零，无需额外删除 schema。原失败、修正和最终证据见本目录 evidence-index.json。

未运行 Linux 全 workspace 或部署 smoke，未验证真实模型、Pi/声学/机械与其他平台。Linux V4 首次旧 panic-fence 子进程 20 秒超时仍保留为未定位项；定向和完整复跑通过不等于根因修复。
