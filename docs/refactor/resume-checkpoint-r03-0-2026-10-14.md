# 历史检查点：2026-10-14 R03-0 完成 → R02-A / R03-A

R02-A 软件已在后续完成；最新恢复请读 [R02-A 检查点](resume-checkpoint-r02-a-2026-10-14.md)。以下保留 R03-0 当时状态，不再重复已完成的双流软件任务。

分支 `codex/architecture-refactor`，源码 **`b5100d778a53dda9f3adc55fd0309a5eb5c853e9`**。R03-0 软件 **10/10**；R01—R06仍 **2/6**，L09—L12 **4/4**。没有真实硬件校准、AEC或物理保持验收。

先读 [checklist](checklists/r03-0-2026-10-14.md)、[四份 HANDOFF](handoffs/2026-10-14-r03-0/README.md)、[验证摘要](handoffs/2026-10-14-r03-0/evidence/validation-summary.json) 与 [硬件资料](../operations/hardware-deployment-readiness.md)，再核对branch/status/log。三个subagent已交回，没有待恢复Cargo、数据库或容器任务。

## 不再重复实施

- 停止/lease关闭或到期/binding撤销不追加归中/disable；原native写入继续由原owner持有，observer drop不撤销全局lease。
- `calibration_confirmed=false`默认，未确认mock不开放motion且零启动写；native未确认构造/start拒绝。校准确认仍验证矩阵/维度/中心限制。初始previous=None，完整提交一次中心目标后才ready，不虚构实姿态或首段物理限速。
- 四个旧soft-start/stop-reset键移除并拒绝；示例、TOML/JSON保存和mock fixtures已迁移。hardware仍默认false。
- PCA开启AI、保留地址模式、EXTCLK提前拒绝；SLEEP后取消/错误/短写由原owner完成有界必要恢复，多板构造未提交时逆序恢复此前板。恢复失败保留原错+unknown，不重放目标。已成功提交的初始化不在未来命令取消时全局回滚。

## 实际验证与限制

先取得5项预期RED，再运行macOS ARM64 `cargo test -p alia-embedded --all-targets --offline --locked`：**263/0/0**，其中lib199、integration64、PCA14；657输入无漂移并与源码提交逐项匹配。Embedded fmt通过。全仓fmt只报基线已有Server `tests/server_runtime/delivery.rs` import排序，本轮未改。Linux daemon/OrbStack停止，未运行本轮Linux，不用历史通过替代。

没有新增依赖、真实PCM/I²C/UART、Pi连接或部署动作。校准确认是操作者声明，当前实际机构没有校准资料；不能将示例矩阵/通过测试当成允许启用硬件。开环位置仍None，物理保持需要独立测量。

## 下一条可执行任务

**R02-A：双流有界证据与合成PCM离线重放。** 继续三个subagent：capture/reference记录与单consumer边界；有界导出/离线重放与fixture；独立回归/文档审查，Root集成与提交。先复查现有AudioDebugRecorder、capture timing、post-mix reference与队列关闭，再补缺口；callback不做文件I/O，不引入新AEC/依赖或启用真实录音。覆盖boot/stream、continuity/sequence、loss、格式、时钟质量、队列延迟可用性、重开与close；参考缺失不能冒充真实静音。

**R03-A：可信资产内容身份**可独立推进；先确定可信清单/摘要来源、版本与签名绑定，再同步core/server/embedded。同一次有界读取校验并解析，拒绝同名替换/未知摘要/旧未绑定命令；不得由模型自报hash建立信任。

Pi/R1和执行器资料沿用已确认输入，不再重复询问：当前Mac+Pi5/8 GB，Ubuntu24.04.4；R1尚未接入、后续USB；PCA9685A+MG996R，停止保持、校准后允许启动归中。扬声器/功放、参考电气、实际校准/供电/OE/散热、模型服务仍待落实。R02/R03现场出口完成后进入R05，包含Settings剩余等待/取消故障和24小时负载；前端、额外平台和未来端侧/同机组合继续延期。
