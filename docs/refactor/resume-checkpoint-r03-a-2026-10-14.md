# 当前恢复检查点：R03-A 可信动作资产内容身份

日期：2026-10-14。分支 `codex/architecture-refactor`；基线 `2b30dd6`。实现提交 **`e7265f0d0e9be8d6e663bfcabd53739b32206e27`**；本包 **10/10**。内容身份、提示音写入边界、最终组合回归、资源与提交内容核对均完成，最终交接记录另行提交。R01—R06 仍 **2/6**，不是项目完成百分比。

先读 [本包 checklist](checklists/r03-a-2026-10-14.md)、[四份 HANDOFF](handoffs/2026-10-14-r03-a/README.md)、[Body 设计](body-command-design.md)、[动作资产操作说明](../../packages/alia-embedded/docs/motion-assets.md)及[硬件部署资料](../operations/hardware-deployment-readiness.md)，再核对 Git branch/status/log。已有 R03-0 和 R02-A 软件包不重复实施。

## 本轮决定与实现

- 用户明确选择双端本地受信清单。操作者审核后，分别在 Server channel 和 Embedded motion 配置固定相同 file/version/SHA-256/bytes。默认空，最多 64 项，单项最多 64 KiB；历史资产不自动受信，模型不能自报摘要建立信任。
- Body JSON 契约与请求/响应签名域升级为 v2；外层 `alia.body.v1.BodyControl/Exchange` RPC namespace 保持。Server/Embedded 需同步升级，旧 name-only 命令拒绝，无补 hash 兼容入口。内部 expression-command-v1 保留原义。
- 首次 Body admission 固定 tuple 到原意图、持久账本与签名。恢复/观察/取消使用原 tuple，清单更新不重绑定或重发；查询后账本行消失也不转为首次 Submit。更早尚无持久 body intent 的 ExpressionPlan 文件名没有历史版本绑定。
- Embedded 在原 prepare owner 中匹配本地清单；目录 fd + openat/no-follow/nonblocking/regular-file 校验后最多读 bytes+1，同 buffer 校验长度/hash 并解析。准备后不重开文件。四个准备任务上限、迟到结果退休、精确取消、原回执和 stop join 均保留。
- 用户明确同意关闭提示音上传绕过。远端 Settings 不得改变本地资产清单或 audio_dir；上传根为启动时解析快照。上传仅空名生成新文件或单 leaf，逐目录 fd 拒绝 symlink，O_EXCL 禁止覆盖已有普通文件、symlink/hardlink。更新提示音使用新名称；无 data 的旧引用不写文件。
- 失败上传批次清除自己创建的文件，可能留下空目录。完整文件写入后数据库结果可能未知，因此保留文件，避免删除已提交引用；这不是文件与数据库原子事务，不做后台自动删除。上传同步文件 I/O 不承诺墙钟上限。

清单保护针对 Body/Settings/提示音数据入口；现有插件 Install/Start 属于同用户宿主代码部署权限，不具有 OS sandbox。本包没有宣称隔离可执行任意宿主代码的部署主体，也没有修复整个插件管理器。将部署权限隔离成独立信任域需要另外决策与实施，不能把这项限制藏进“完全独立本地 pin”的承诺。

## 验证与证据

旧实现的 Core name-only、Server 空清单、Embedded 同名替换三个 RED 均真实断言失败；附带 cue 越界写入第四个 RED 亦真实失败。[原始证据与汇总](handoffs/2026-10-14-r03-a/evidence/validation-summary.json)已归档。最终输入 661 项，manifest `76a90d0416ae60e8d316a27ff06a0a0457c785659feb8d5235b2d121dfcdf4b0`。

最终 macOS ARM64 默认 features/默认并行：Core+transport **41/0/0**（2.42s），Embedded **306/0/0**（98.74s，236 lib+70 integration，无 filtered）；Server **860/0/5**（410.01s）；合计 **1207/0/5**。三个命令均 exit0、无输入漂移，661 个提交 blob 与最终清单完全一致。仅使用临时 fixture、loopback、独立测试 PostgreSQL 和 SQLite，不触发真实硬件、录放音或模型。

Server 初次候选 full 因独立测试数据库在开测前已 clean-stop 发生连接超时，Root 有界终止唯一测试子进程并保留失败原件。停止发起方未知；重启同一任务数据库、SELECT 1 后同源重试 **860/0/5**。候选 Embedded 曾跳过已知 cue RED，这份 **297/0/0、1 filtered** 不是最终验收。

启动既有 OrbStack 时自动恢复了 Home Assistant、旧 alia-postgres、astrbot 三个原有容器，Root 随即按精确 ID 停止它们，未向其应用发送查询/修改；不能据此推断其自动启动没有自身副作用。本轮独立测试数据库及其匿名卷已移除，验证进程已退出。清理时发现 astrbot 在08:48:26 UTC被再次启动（发起方未知），所以保留它和 OrbStack；Home Assistant 与旧 alia-postgres 保持停止。首次清理脚本因共享服务不再全为停止状态断言退出1，只读跟进确认本轮数据库已不存在，原失败和最终状态均归档。没有安装依赖或拉取新镜像。Linux 容器仅运行测试 PostgreSQL，本轮未执行 Linux 应用回归、Pi、ALSA、I²C/UART、真实 PTY、付费探测、真实模型或长稳验证。变更 Rust 格式通过；全仓 fmt 只有未改的历史 Server import 排序差异。

## 下一阶段

主要投入继续 R02/R03 的受控现场前置和验证，再进入 R05，当前不自动启用硬件：

1. R02：R1 尚未 USB 接入；落实扬声器/功放、实际固件/格式、音频权限、AEC 参考电气兼容与布局，再确定真实音频的采集/保留规则并做双讲和尾音基线。已有 R02-A 双流离线重放不是 AEC 或声学验收。
2. R03：操作者实际审核资产并同步双端清单；补齐 PCA9685A/MG996R 供电/OE、通道、限位、校准与保持的实证。继续默认 hardware=false、calibration_confirmed=false，不用历史矩阵或可解析资产代替校准。当前硬件没有主机可读位置反馈，Submitted 不等于机构到位。
3. R05：独立安装/配对/恢复、Settings 剩余等待/取消故障、至少 24 小时混合负载与关闭证据；须等 R02/R03 现场出口，不能用纯软件通过代替整机验收。

当前 Mac 为 Server；Pi5/8GB、Ubuntu24.04.4、5V5A、ke-qiao@192.168.10.221；停止/断连保持、校准后允许启动归中等已确认资料不重复询问。模型服务仍待用户确认。V01、前端、额外平台、通用 Embedded 适配及未来端侧/同机部署继续在[延期台账](deferred.md)保留。
