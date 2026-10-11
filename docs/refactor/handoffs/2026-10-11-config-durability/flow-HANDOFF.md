# 2026-10-11 Config durability — Setup flow HANDOFF

## 范围与状态

基线 `7165ad6`。本分工负责 `oobe/bootstrap.rs` 的模板与完成流程、`oobe/routes.rs` 的 HTTP 接线、`bootstrap/setup.rs` 的 CLI／HTTP 生命周期。配置发布和恢复实现归 `config/publication.rs`，Root 负责配置加载／发现与重启入口；独立 regression agent 负责迁移 `oobe/routes/tests.rs` 和新增文件系统／HTTP 回归。

- [x] 审计旧完成门、保存 JSON、CLI 错误及重启加载入口。
- [x] 明确已发布但同步未确认不能当成发布前失败、成功或已回滚。
- [x] 使用单一 watch 状态承载 Idle / Pending / Ready / Cancelled。
- [x] HTTP、CLI、取消迁移到同一个完成 owner，删除旧发布旁路。
- [x] 保持 HTTP 200 `{success,path,error?}` 和现有来源／媒体类型限制。
- [x] CLI 未知结果留在输入循环；取消保留磁盘发布与恢复标记。
- [x] 补完整 setup 生命周期的 unknown → 同请求恢复 → Ready 响应排空回归。
- [x] 持久化实现落盘后交叉核对类型、错误和同意图恢复。
- [x] 审阅 Linux V2 既有播放测试失败，只修正该测试的本地 gate 观察等待，保持共同 3 秒期限。
- [x] Root 冻结并完成双平台、必要 feature 和资源清理验证。
- [x] Root 更新总 checklist、恢复检查点与正常提交。

本分工仅执行指定文件 rustfmt，不运行 Cargo、数据库、Docker、Git，不操作真实外部服务或硬件。Root 已报告 V2 本轮 14 项新增测试在 macOS 与 Linux 通过；Linux 全 Server 的既有播放测试失败及后续测试纠正单独记录如下，最终 V3 验证待 Root 填入。

## 旧缺口及新契约

旧 `finish_setup` 的 watch 写锁覆盖准备／发布，保证 HTTP、CLI 和取消只有一个赢家；但只接受 Ready / Cancelled，任何错误都回到未完成状态。若直接在原写入末尾新增目录同步，发布后的同步失败会被当成完全未发布，允许第二个目标或内容重写；CLI 的旧通用错误清理还会删去已经发布 base 依赖的 fragments。

`SetupCompletion` 现在持有唯一的 `watch::Sender<SetupState>`：

- Idle：尚无受约束发布。确定的发布前失败保持 Idle，可修正输入。
- Pending：保存 `PendingPublication`，不发送完成信号，不开放第二次发布。相同目标／内容／HTTP 或 CLI 安装方式通过 `matches_request` 后，只调用原 descriptor 的 `retry`。
- Ready：发布及所需同步已经确认，只把路径交给启动流程。
- Cancelled：取消封闭本次 setup；不删除可能已经发布的文件或磁盘恢复标记。

发布与恢复均在同一个 watch 写门内调用。没有第二个写锁、后台发布任务、重试队列或缓存。Pending 重试即使返回普通错误，也保持原 Pending，并明确表示持久性仍未确认；只有成功才能进入 Ready。Ready 与 Cancelled 不再执行任何保存闭包，迟到的 CLI、HTTP 或取消不能改写赢家。

`finish_setup`、`SetupResult`、OOBE 的 `release_example_config`、`save_config`、`write_validated_config` 及本地清理辅助函数均删除。CLI 与 HTTP 直接写入旧入口不再保留为兼容旁路。完整模板仍由原嵌入 base 与 fragments 按 loader 顺序合并；CLI 仍请求发布原 base 加独立 fragments。

## HTTP 与 CLI

`OobeState` 改为持有 `Arc<SetupCompletion>` 与原 cancellation token。保存成功仍为 HTTP 200、`success=true`、原目标 path；验证失败、未知结果、恢复失败及已完成拒绝仍为 HTTP 200、`success=false` 与 error。没有新增端点、字段、CLI flag、授权或来源规则。Host / Origin / JSON Content-Type 的既有保护保持。

`run_setup_on` 仅等待 `SetupState::is_terminal`，Pending 的 watch 更新不会触发正常启动或关闭 setup。CLI `c` 遇 Pending 会输出未知结果及原请求重试说明、清空输入行并继续轮询；再次 `c` 对应原 CLI 意图时只确认原发布。HTTP 已占用 Pending 时，CLI 不能改用默认模板覆盖，必须由原 HTTP 请求恢复。`q`、Ctrl-C 和外部取消封门，磁盘状态留给重启恢复。

取消检查仍在完成门内、实际写入之前。已经进入同步文件系统操作的发布不能被请求 future 的取消伪装为撤销；本轮没有声称强制中断同步系统调用。CLI 在外部取消与 `c` 同时发生时按取消退出，避免将正常取消误报为输入 worker 故障。

Ready 之后仍保持原服务器 future，使用原 2 秒 drain 返回成功 JSON，再关闭停滞连接并 join 终端 worker。listener witness、真正 socket 关闭与原 PTY 释放测试保持。

## 重启边界

Root 与持久化分工采用配置域的 pending descriptor：正常无标记配置保持原只读加载路径；只有存在标记时才先核对及确认现存发布。descriptor 的创建、清除、目录同步和恢复字节／身份检查归持久化实现，本文件不把流程 owner 的内存状态误当成跨进程凭证。

审计确认，仅重新启动并成功解析配置，不能证明前一轮同步成功。每次无差别同步所有配置及祖先目录会给旧读取路径增加目录可读／可同步要求，因此本轮未采用该方案。Linux 的 fsync 不要求写 FD，但打开目录和文件系统同步能力仍是独立约束；macOS 的 Rust `File::sync_all` 平台映射及目录同步方式由 Root／持久化分工验证。

资料依据：[Linux fsync 手册](https://man7.org/linux/man-pages/man2/fsync.2.html)、[Apple fsync 手册](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fsync.2.html)。本轮软件故障注入、重启和同步调用证据不等同真实断电试验。

## 新增流程回归与交叉审阅

本分工新增 `bootstrap::setup::tests::pending_publication_keeps_setup_available_until_confirmed_response_drains` 一项：真实 `run_setup_on` 和 HTTP POST 在 `Stage::Published` 动作后注入一次错误，断言 HTTP 200、`success=false`、unknown 诊断，base 已可见且 marker 保留；随后真实 GET setup/status 仍能响应。发送相同 POST 后完整读取成功 JSON，再等待 setup 返回原路径、listener witness 关闭及另一条停滞请求的实际 socket 释放。失败路径捕获断言 panic 后先 cancel / join setup，再清理临时目录；不使用进程 kill 或真实断电。

旧 routes 测试迁移及四项持久性／HTTP 回归由 regression agent 完成；本分工没有重复统计它们。旧真实 PTY 释放测试改用新 completion owner，未把其 ignored 状态视作通过。

交叉审阅确认了存储边界：PendingPublication 经 anyhow 类型保留到单 watch 状态；marker 移除后的最后目录同步错误仍由内存 descriptor 继续确认，不能另发 publication；若重启时 marker 已消失，其删除前 base 和依赖已完成同步，普通加载并未据“仅存在”伪造原保存成功。已有合法磁盘 marker 与新请求意图不符的分支必须返回该 marker 的 typed Pending，让新 owner 保留原意图；此项已反馈存储作者并获确认修正。没有发现需要新增端点、字段或 CLI flag 的理由。

## V2 既有播放测试失败与 V3 有界纠正

Root 报告 Linux V2 首次全 Server 运行在既有 `duplex_playback::control_ack_loss_is_observed_from_original_device_command` 的最终 `assert!(!producer.output().is_held())` 失败；日志保留。随后同一二进制以默认并行运行该测试文件的 9 项，结果为 9 通过、0 失败（13.445 秒）。这次复查不能消除或改写首次失败，也不代表已经修复竞态。

只读审计明确了测试等待条件的缺口：`tests/duplex_playback.rs::Scene::command_count` 只从独立数据库连接观察 `output_control_commands.status='receipted'`；`modules/delivery/service/output.rs::apply_control_receipt` 先 `await repository.record_control`，随后才核对当前权限并调用本地 `gate.resume`。因此数据库提交已经可见时，控制任务仍可能没有回到本地 gate 更新；原测试直接把数据库见证当成生产者已经解除暂停的见证。ACK 丢失仍按原 command 执行 Observe，不通过重发获得通过条件。本轮未改 Playback 生产实现。

经 Root 授权，V3 仅修改上述一项测试：将既有 `command_count(..., 2)` 与 `eventually(|| !producer.output().is_held())` 放进同一个 3 秒外层 timeout。两个观察共同消耗原 3 秒预算，保留原最终断言、32 秒场景期限、producer drop 和失败 cleanup；未修改通用 helper、其余播放测试、生产代码或加入生产测试 hook。scoped rustfmt 后逐字比较确认该文件仅此处变化；V3 源码已经停止修改，编译和执行交由 Root 统一完成。

## Root 最终验收补记

源码 `095499676496fba79853532b2ad4f570b368d37c`，V3 manifest `11373f598d31148c27b15bfa1fc5bb015e3b2726f877347560b23bbcc1b9343f`，650/650 Git blob 一致。

macOS workspace **1108/0/5**；Linux ARM64 Server 默认并行完整回归 **826/0/5**。两端无默认 feature 全目标检查、架构边界正反自测、变更文件格式检查、计数器 8 例自测通过。数字为 passed/failed/ignored；嵌套子进程摘要不重复累计，最终阶段同源、无源码漂移。

全部 Cargo 阶段结束；关闭前其他数据库连接为零，最终测试 schema 为零，遗留 schema 清理数 0。本轮 V1/V2/V3 Linux holder 与独立 PostgreSQL 已停止；用户 5433 未访问。空闲 holder 的停止码不作为 Cargo 测试结果。缓存、只读快照保留在 ignored `run/validation-20261011-config-durability/`。

上文 subagent 实施阶段的进度数字与未运行说明保留历史时点；对应 Root 验收事项已勾选，现已完成同源整合验收、资源关闭与源码提交，本轮总 checklist 为 10/10。

未运行真实断电、独立进程 kill/restart、Linux 全 workspace、release/native release link、部署 smoke、真实 PTY、外部或付费模型、Pi/声学/执行器及长稳验证。故障注入与丢弃旧 setup owner 后加载证明软件恢复语义；不代表任意存储设备兑现 flush，也不提供其他本机进程的文件系统锁。
