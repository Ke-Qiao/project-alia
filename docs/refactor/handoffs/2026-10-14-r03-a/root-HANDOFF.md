# R03-A Root HANDOFF

日期2026-10-14，基线2b30dd6，分支codex/architecture-refactor。本包10/10；实现提交`e7265f0d0e9be8d6e663bfcabd53739b32206e27`。661个提交blob与最终冻结输入一致；总包进度见[checklist](../../checklists/r03-a-2026-10-14.md)。

## 用户决定与分工

用户确认双端本地受信清单：受信操作者审核文件后，在Server与Embedded受控配置中固定同一文件/version/SHA-256/bytes；默认空，历史资产不自动信任，模型不能自报摘要建立信任。随后确认同轮关闭提示音上传任意路径绕过、禁止远端改audio_dir与清单。现有政策、执行批准、校准门槛和硬件禁用保持；没有新增依赖、部署或真实录放音。

三个subagent：settings_auth_fix负责Core/Server/transport；settings_auth_reproduce负责Embedded同次读取/解析及配置；settings_auth_review独立审查。Root负责共同契约、附带cue写入修复、统一测试、文档与提交。额度中断后沿用原分工恢复，未替换为未经复核的并行实现。

## 实现

BodyAnimationAsset(file/version/sha256/bytes)由双方本地approved_animations提供；最多64项、文件为单叶.anim名称、非零version、小写64hex digest、1..65536bytes。Body JSON version及request/reply域分离签名升2，proto外层namespace保持alia.body.v1；expression-command-v1不机械改版。内建动作不依赖动画清单，空清单只关闭Server的play_animation可见性。

Server首次body admission固定tuple到Entry、持久账本、签名；模型多余pin字段拒绝。已存body命令按原tuple恢复和same_intent核验，不按新清单重解释；Entry保留已读取事实，账本行随后消失也不转为新Open/Submit。owner/Response/channel检查和新dispatch事务政策保持。尚无durable body意图的ExpressionPlan文件名没有历史内容绑定，本轮不前移到Response生成或改变批准体系。

Embedded唯一生产读取经prepare的canonical/catalog校验，dirfd/openat/no-follow/nonblocking/regular验证后最多读bytes+1，对同buffer长度/hash校验及UTF8/帧解析。已经存在的prepare后内存隔离继续保留；没有重开文件，原回执、精确取消、准备并发4和shutdown join保持。配置祖先与同用户文件权限仍由操作者控制。

Settings merge后不能改变原本地清单和audio_dir，拒绝不碰磁盘/内存。Cue实际用启动resolved root；所有上传名称先验证，逐目录fd/no-follow，新文件O_EXCL，不覆盖普通文件/symlink/hardlink。可信配置的..按fd走，请求仅leaf；已有无data引用不写文件。失败批次RAII清本次新文件，可留空目录；成功文件后DB可能已commit，未知/取消时保留，不声称跨文件/SQLite原子。无新后台任务或垃圾回收器。

插件Install/Start是既有同用户宿主代码部署权能，无OS sandbox。本轮不隔离这种部署主体，也不修插件路径漏洞；仅收口Body/Settings/提示音数据入口，不能宣称完全对抗可执行任意宿主代码的Server或本地进程。

## 已复现与验证

- 659个RED输入manifest aff613f253280c38dc44b35f420533153eb04c07f802d00b16d6f39de3a31771。Core缺内容身份、Server空清单、Embedded同签名同名替换分别预期断言失败，exit101，16.33/55.09/10.50秒，全部source_drift=[]，非编译失败。
- 660个候选输入manifest412ba53ed2f412768299a0c47c704f175919f692e6e7de02f96b69609a0d11c0。Cue越界RED 19.11秒真实断言失败；Core/transport41/0/0；Server同源恢复DB后860/0/5；Embedded候选297/0/0但过滤1项cueRED，仅历史候选。
- 最终661输入manifest76a90d0416ae60e8d316a27ff06a0a0457c785659feb8d5235b2d121dfcdf4b0。Core/transport41/0/0（2.42s），Embedded306/0/0（98.74s，无过滤）；Server860/0/5（410.01s），合计1207/0/5。默认features/默认并行，原始日志与退出码保留，已完项source_drift=[]。
- 变更Rust格式与git diff --check通过；全仓fmt仅未改的packages/alia-server/tests/server_runtime/delivery.rs历史import排序差异，已对HEAD字节核对。

## 环境与资源

使用当前macOS26.6.2 ARM64、Rust1.95；所有动作测试仅mock/临时文件。没有Pi/SSH、ALSA、I²C/UART、真实PCM、模型或新依赖。Linux只运行独立测试PG，不算Linux应用验证；真实PTY、模型及5个ignored没有重跑。

为必要SQL回归启动既有OrbStack，自动恢复旧Home Assistant/alia-postgres/astrbot三容器。Root按精确ID停止它们恢复原Stopped状态，没有向其应用发送查询/修改；其自身启动副作用不能保证为零。独立task PG来自本机已有镜像，不拉取新镜像、不使用旧DB。

独立task PG在08:48 UTC clean-stop，发起方未知；首次Server08:55才开测，连接超时。Root在505.86s只SIGTERM唯一lib测试子进程，父wrapper记录exit101且无完整统计；不虚构失败数。重启该task PG并SELECT1后同源candidate重试通过。原记录保存于database-interruption.json和candidate-server原件。

本轮task PG已停止并docker rm -v，验证进程退出。cleanup脚本在第三个旧容器状态断言失败：astrbot于08:48:26UTC被重新启动，发起方未知。后续只读核对确认task DB不存在、HA/旧PG停止，保留astrbot与OrbStack以免干扰其他工作；不声称全环境Stopped。首次清理在断言前的stdout仅在内存未落盘，cleanup.json如实区分断言顺序事实与随后实际读回。实现提交已逐blob核对，最终交接另行提交。

## 接续

[恢复检查点](../../resume-checkpoint-r03-a-2026-10-14.md)列出R02/R03现场前置与R05出口。R1未接入、功放/扬声器/参考电气、执行器校准/供电/OE及保持实证仍缺。R03-A纯软件通过不增加六轮完成数（仍2/6），不启用hardware或calibration确认，不冒充位置/停止/声学验收。
