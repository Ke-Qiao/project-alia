# R03-A — Embedded 动作资产与准备所有权 HANDOFF

日期：2026-10-14；Owner：`settings_auth_reproduce`；基线 `2b30dd6`。本分工负责 Embedded motion/config/settings manager/回归和包文档。Core/Server/Transport 由 contract owner 负责，提示音上传边界由 Root 负责。本 agent 不运行 Cargo、数据库、Docker、硬件或 Git 提交。

## Checklist 与进度

- [x] 01 追踪真实签名请求、prepare、文件读取、执行和清理所有权，先区分缺口与已有隔离。
- [x] 02 在旧生产实现上添加同 canonical 请求换内容 RED；Root 实际运行、归档，无源码漂移。
- [x] 03 对齐用户确认的两端本地受信清单与 Core 共同 `BodyAnimationAsset` 契约。
- [x] 04 完成目录 fd 锚定、有界同次 bytes 验证/解析、默认空 catalog、远程清单保护及全部 Embedded 调用迁移。
- [x] 05 补充文件替换、非 regular、上限、RPC 回执和精确取消/准备 owner 回归；独立 reviewer 最终静态读回无阻断。
- [x] 06 Root 对最终冻结输入运行 GREEN，回填结果和当前平台限制。
- [x] 07 与 Root 的远程写入边界处理整合，完成最终证据/文档读回。
- [x] 08 Root 更新总 checklist、恢复检查点并正常提交。

当前 **8/8**；实现、独立静态复核和最终 Embedded GREEN 已完成，生产/测试和包文档已停写。Root 已完成实现提交、总文档与证据核对；身份见文末。R03-A 软件内容身份不等于机械安全、校准或实机验收。

## 旧路径与真实 RED

原路径为：`BodyService` 签名认证 → 2 秒 RPC prepare 等待 → `MotionRuntime` 最多 4 个持有 JoinHandle 的 blocking worker → `plan::prepare` → `read_animation`。原签名只覆盖文件名/fps/loops；`canonicalize` 后再 `File::open`，有路径替换窗口。原读取已限制 65537 bytes，并检查 64 KiB/600 source frames/有限角度，但未绑定批准内容。

`unchanged_signed_animation_cannot_select_replaced_file_contents` 使用临时目录中两份同样合法、不同轨迹的内容：首先固定 canonical 请求并准备 1° fixture，再把同名文件换成 7° fixture；原实现第二次成功，违反新要求。Root 的 `red-embedded.log/json` 为真实断言失败，退出 101，10.50 秒。共同 RED 659 输入 manifest SHA-256：`aff613f253280c38dc44b35f420533153eb04c07f802d00b16d6f39de3a31771`，`source_drift=[]`。当前 GREEN 测试使用原 fixture 的明确内容 pin，再复用完全相同 signed command 检查替换被拒绝。

准备后的轨迹原本已保存在 `PreparedBodyCommand.actions`，writer 不再读文件。这不是旧漏洞；新增 `prepared_animation_is_immutable_after_the_file_is_replaced` 保留已有行为，未把它伪装为 RED。

## 最终实现范围

- `motion/assets.rs` 是唯一新私有文件：要求 signed asset 与本地 `approved_animations` 的完整 entry 相等，然后打开 asset。macOS/Linux 的配置目录最后一段拒绝 symlink，目录 fd 取得后用 `openat` 打开 canonical 单段 `.anim` leaf，设置 `O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC`，立即将 fd 交给 RAII File，验证 regular file。
- 在该 fd 上最多读 `asset.bytes + 1`；Core 将 declared bytes 限定为 1..65536。这个 buffer 先验证准确 byte count 和 SHA-256，再被 `plan.rs` UTF-8/frames parser 使用，没有第二次打开或重新 hash 磁盘。打开后的路径替换不会重定向 fd；原 inode 并发改写若不匹配批准 bytes 就拒绝。
- 目录 fd 之前的中间路径组件仍属于本地受信配置/权限范围，不声称密码学验证消除了所有本地管理员路径行为。非 Unix 平台动画安全打开 fail closed，没有增加适配器或不安全 fallback。O_NONBLOCK 防止 FIFO open 等待；普通文件系统 I/O 的真实系统调用仍不承诺可立即中断。
- `MotionConfig.approved_animations` 默认空，使用共同 Core schema/容量与重复文件校验。`load_config`、library path normalization、保存和 runtime 建造校验清单；配置 disabled 也不会使无效列表被 runtime 构造接受。运行中不扫描并自动相信现有目录。
- `EmbeddedSettingsManager` 合并 remote patch 后、保存前，要求清单与此前 snapshot 完全相等。新增/换版本或摘要/清空/重排等改变都拒绝；相同完整 snapshot 的 round-trip 保持可用。此处不新建远程管理员角色或批准入口。
- 用户随后明确授权关闭提示音上传绕过；本 agent 在限定范围内为 settings manager 保存启动时已解析的 `direct_response_defaults.audio_dir` 不可变 clone，公开只读 `direct_response_audio_dir() -> &Path` 供 Root 的 service 写入 owner 使用。远程 settings 合并后必须与原 snapshot 的 raw 目录 `as_os_str()` 相等，绝对/相对别名、`./`、清空或整体替换也不能改变本地目录；拒绝发生在保存前。上传不再从可变、相对的持久化 JSON snapshot 选目录。实际 fd 锚定写入由 Root 负责。
- Body JSON/version/signing domain 已迁移到 Core 的版本 2；service route 保持 `alia.body.v1`。所有 Embedded 构造使用新 asset 引用。`hardware_enabled=false`、`calibration_confirmed=false` 默认值和实际校准门槛不变；没有批准历史动画，也没有校准或驱动设备。
- 本地 discovery 只按 enabled/calibration/mapper 发布整体 `motion.native`，没有逐动作 `play_animation` 能力声明。空批准清单使 Animation prepare 拒绝，但不关闭内建动作或整体 writer 可用性；Server 的逐动作能力另外按其本地清单非空 gate。冻结期间只读核对此边界，未新增能力字段。
- prepare 的 4 个 owned worker、timeout 后仍持有 job、stop join、submit 的再次鉴权和逐 frame 取消保持原路径。Prepared actions 不在 submit 时重读。Core 对 canonical asset 的字段校验不会要求实际文件存在，因此原命令的 Observe/Cancel 仍能在资产丢失时使用。

配置与包文档位置：`config/embedded.example.toml`、`packages/alia-embedded/docs/motion-assets.md`、`packages/alia-embedded/docs/configuration.md`。文档明确先停受影响进程、编辑两端清单再重启；运行中的 settings manager 持有启动 snapshot，可能覆盖未重启的人工磁盘编辑。没有修改实际部署配置。

## 已实现回归及验证入口

本轮新增/扩展的测试使用明确的临时 synthetic fixture，不以运行时自动计算哈希作为生产信任根。

- `module::motion::assets::tests`：目录 rename 后原 fd 仍读原目录；leaf symlink 替换拒绝；`fcntl` 直接检查 NONBLOCK/CLOEXEC；临时 FIFO 由非阻塞双向 guard 持有避免测试挂起并验证 non-regular 拒绝；打开后的文件名替换仍读原 fd，原 inode 改写则 hash 拒绝。
- `module::motion::plan::tests`：同签名 pin 的同名替换拒绝；准备后内存轨迹不变；本地 entry 的 file/version/hash/bytes 任意不一致或空清单拒绝；准确 65536 bytes 可以验证，追加/截断拒绝；批准但非法 UTF-8 仍拒绝。原所有帧验证、有限角度、source count/loops/duration、symlink、cadence 和完整尾帧继续覆盖。
- `prepared_approved_animation_cannot_revive_a_retired_identity_or_revoked_lease`：晚提交已退役身份返回原拒绝回执，已准备资产在 lease 撤销后不能进入 native writer，只有启动目标计数。
- 原 `preparation_capacity_and_stop_ownership_survive_dropped_rpc_waiters` 改为真实批准动画 fixture：四个受控阻塞 prepare waiter 被 abort 后仍占 owner 容量，stop 观察取消不丢 job，释放后 stop join 完成；未创建新后台 owner 模型。
- `approved_animation_tampering_rejects_new_work_and_preserves_original_receipt`：真实 loopback、签名请求/回复及 mock motion。先完成批准动画，再改内容；重复原 Submit 的 prepare 失败保留原 Submitted receipt，新的 sequence 返回零接纳/零 frame Rejected。删除文件后原 Submit/精确 Cancel/Observe 仍返回原执行事实，不误称零写、不重放。
- `tests/config.rs`：默认空清单、本地 load/save round-trip、重复/超量/无效版本在保存前拒绝且原 bytes 不变；remote 从空清单新增、换 pin、清空、整体替换拒绝，RAM/file 不变；同值完整 snapshot 与普通 fps patch 可继续使用。
- `cue_upload_directory_uses_the_resolved_startup_snapshot` 与 `remote_cue_directory_changes_preserve_memory_disk_and_startup_owner`：区分启动绝对路径与持久化相对值，构造后修改原 config 不影响 owner；同值完整 JSON/单字段回传和无关 fps 更新可用；变目录/等价别名/绝对路径/null/整体替换拒绝后 RAM、disk、getter 均不变，首次拒绝不创建配置文件。

Root 的提示音上传越界 RED 和 fd 写入实现不在本文件 owner 内；用户已确认本轮修复，最终整合已实测通过。只读确认 service 已调用启动目录 getter；Root 的 `cue_files` 对所有上传 leaf 先校验、以目录 fd 逐段打开并拒绝 symlink，创建新文件使用 `O_EXCL | O_NOFOLLOW`，拒绝覆盖现有 regular/symlink/hardlink；同批文件写入失败由 RAII 回收该批已创建文件。数据库保存结果未知时不盲删已完成文件，也没有把整个文件/数据库操作声称为原子事务。上述保证仍不覆盖下述已获宿主代码执行权的插件部署主体。

## 只读入口复核与信任边界

Settings RPC 的配置变更统一经过 `update_from_json` → `update_from_value` → catalog/目录相等检查 → `save_runtime_settings`，绑定管理和 plugin metadata 刷新只读 settings。`ensure_device_id` 只在未设置时保存本地 device ID，不改变 catalog。load/save/library builder/runtime/prepare 均沿共同 catalog 校验。资产 canonical 字段校验先于打开，descriptor 的固定 bytes 上限先于单次读取，digest 与 UTF-8/parser 使用同一 buffer；文件 fd 的失败路径由 RAII 关闭。

取消 RPC waiter 不代表阻塞系统调用立即取消。四个 prepare worker 的 JoinHandle 继续由 MotionRuntime 持有，stop 重入保持待 join 的同一个句柄；调用者取消只丢失 observation，不能放出重复 worker 容量。最终提交再次鉴权/检查 lease 和已退役 sequence，writer 只消费已准备内存轨迹并沿原 permit 逐 frame 取消。没有把任务 abort 当作物理设备已停止证明，也不承诺一般文件系统调用的墙钟完成时间。

上述保证针对 Body/Settings 的数据入口；具备插件安装与启动权限属于另一种、既有的软件部署能力。`PluginManager` 对绑定服务器的授权检查之后可安装插件，`PluginRegistry::start` 以宿主相同用户启动脚本/可执行文件，未提供 OS 文件隔离。本轮不承诺隔离已获同用户代码执行权的操作者或插件，不把这些主体等同于普通动作或提示音数据请求。只读还观察到既有安装路径/name 与解压写入检查需单独审查；本轮不修复插件路径写入、不部署/启动插件、不扩大该权限模型。

## 验证结果与待续

已完成 scoped rustfmt、独立静态读回和最终 Embedded GREEN。此前 `red-cue-boundary.json` 记录真实业务断言失败（退出 101，19.11 秒），660 输入 manifest 为 `412ba53ed2f412768299a0c47c704f175919f692e6e7de02f96b69609a0d11c0`，`source_drift=[]`。该 RED 保留为旧实现缺口证据。

用户确认附带修复之前，Root 在同一 660 输入候选运行 Embedded `--all-targets --offline --locked -- --skip grpc::settings_service::cue_tests::cue_upload_cannot_overwrite_a_config_outside_its_audio_directory`：退出 0，108.12 秒，无漂移，lib 229 + integration 68 = **297 passed / 0 failed / 0 ignored，1 filtered**。该命令明确跳过已知 cue RED，仅为中间候选，不是最终 GREEN，也不涵盖随后新增的目录固定实现和两项配置回归。

最终 `green-embedded.log/json`：`cargo test -p alia-embedded --all-targets --offline --locked`，默认 features/测试并发、build jobs 2，退出 **0**，**98.74 秒**。独立逐段计数为 **lib 236 + integration 70 = 306 passed / 0 failed / 0 ignored / 0 filtered**；661 输入 manifest SHA-256 为 `76a90d0416ae60e8d316a27ff06a0a0457c785659feb8d5235b2d121dfcdf4b0`，`source_drift=[]`。

已在最终日志逐名核对：原同 canonical 换内容 RED、新的同 fd/目录替换/非 regular/最大 bytes/UTF-8 回归、4 owned prepare/late admission/原 receipt 精确取消、两个新目录 settings 用例均为 `ok`。原 cue 越界 RED 已通过，Root 新增的目录/leaf symlink 与 hardlink、批次部分创建回收、无数据引用不写盘及固定目录下合法上传用例均为 `ok`。已有 waiter drop、prepare join error 仍 join native writer、取消超时保留原 owner、已完成 JoinHandle 不重复 poll 的清理回归亦通过。

最终源引用只读复核未发现遗漏：service 使用 getter，plan 唯一调用 assets 同次读取，catalog 的 load/save/library/runtime/prepare 校验完整，旧 `command-v1` 注释已修正。日志均保存于 `run/validation-20261014-r03-a/`。本次已核对的 Embedded 运行环境为 macOS arm64；此处没有当前 Linux Embedded 测试证据，不能以 Linux 容器运行数据库或历史测试替代。源码、测试及包文档保持冻结。

尚未进行部署、真实设备打开、录放音、PCA/serial 输出或机械动作。历史 neck calibration/animation 文件仍是参考资产；内容 pin 与本地 catalog 不能替代实际机制审核和校准。


## Root最终收口

实现提交`e7265f0d0e9be8d6e663bfcabd53739b32206e27`已正常完成；661个Git blob与最终green manifest逐项一致。Core/transport41、Embedded306、Server860，合计1207/0/5、无filtered、无漂移。本分工最终 **8/8**。本轮PG已移除；共享astrbot后来重启，Root保留它与OrbStack并记录cleanup断言/只读收口。最终总文档与原始证据见本目录README。
