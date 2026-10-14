# 当前恢复检查点：R02／R03 现场前置

日期：2026-10-14；分支 `codex/architecture-refactor`；基线 `283f38f`。本包 **10/10**；实现提交 `e409dc944a589104d0e2d2c2977f642ad8aaef28`，最终交接另行提交；R01—R06仍 **2/6**，不是项目百分比。

恢复先读 [checklist](checklists/field-prep-2026-10-14.md)、[四份 HANDOFF](handoffs/2026-10-14-field-prep/README.md)、[硬件部署资料](../operations/hardware-deployment-readiness.md)，再核对 branch/status/log。R03-0、R02-A、R03-A已完成，不重新实施。

本轮补齐 `alia-embedded --check-config <path>`：只读显式配置与所指校准/已批准动作文件，复用现有类型、speech、motion和内容身份/帧解析，不构造 runtime/backend/worker，不创建身份/状态、不打开设备或服务。帮助/错误参数不落入普通启动；无参数行为保持。配置有1MiB输入界限，missing/旧字段/错误拒绝，不用默认配置掩盖缺失。选择 PCA 的地址规则可在硬件关闭时检查，运行时原行为不变。

新增 `config/embedded.staging.example.toml` 为七模块noop、设备发现关闭、绑定门槛保留、独立路径与loopback临时端口的示例。仅完成离线检查，未正常启动；正常启动仍写盘和监听。清理旧 speech.enabled、自动录音目录与不存在的校准工具说明。

检查成功不证明模块/模型可实例化、Server清单一致、音频设备权限或AEC/物理校准；输出 hardware_verified=false 和 physical_state=not_observed。同步本地文件I/O无墙钟期限，本地父目录/操作者与插件代码部署权仍是信任前提；不生成或自动批准摘要。默认 shadow、执行批准、精确取消和清理保持，hardware/calibration仍false，清单仍空。

下一步先做R02 Mac合成PCM离线模拟并补现场资料；R03后续暂停、已完成软件不回退：R1当前接入/版本、扬声器功放和厂商AEC参考规格；PCA板数/地址、舵机通道/供电/OE/限位/校准；Mac/Pi服务账户目录、Pi散热、模型端点版本和审核者。本轮已收到集中回复：R1仍未接入，允许Mac模拟，AEC参考接法/电气规格未拿到；R03全部机构资料未确定，用户要求后续暂停；部署资料待定，Pi可SSH登录。不要重复询问已确认的Mac、Pi5/8GB/Ubuntu24.04.4/5V5A、USB归属、停止保持与校准后归中选择。

Mac模拟沿用已有合成证据工具，不打开真实麦克风/扬声器，也不把其结果称为模型或AEC验收。资料齐备后依现场记录先做指定目标只读核对，再确定真实录放音窗口、参数与PCM保留规则；R03须用户恢复后才推进。R02声学和R03实际保持/恢复证据完成后进入R05独立安装/配对/故障/长稳；不能用当前离线通过代替。V01、Settings剩余等待/取消验证、前端、额外平台和端侧/同机部署按[延期台账](deferred.md)继续保留。

验证：macOS26.6.2 ARM64 / Rust1.95 默认 Embedded **316/0/0**，45.05s；无默认 CLI **5/0/0**，17.77s；两示例离线报告通过、输入hash不变。665项最终输入与实现提交全部blob一致，manifest `dd888cec2f84c06a213cadc236911d088c89015b144025284e8eab72070d3573`。首轮fixture缺identity必填字段exit101原件保留，不计业务RED；变更格式通过，全仓fmt仍只有历史Server import排序。见[验证汇总](handoffs/2026-10-14-field-prep/evidence/validation-summary.json)。所有测试进程退出，未启动任务服务/容器/设备；本轮未SSH、未改共享服务、未测Linux/模型/真实硬件/长稳。构建目录当前binary由最后一次无默认CLI构建产生，实际默认smoke二进制hash另已归档；后续部署按选定features重新构建，不把target目录当发布身份。

用户允许Mac模拟后，本轮另实际运行现有无默认features合成PCM示例，exit0、4.66s：7条记录、16个样本值、9个重置提示，保留2个丢失播放块、独立时钟未对齐和旧采集流结束状态unknown。文件标记SyntheticFixture，physical_stream_complete=false；只是离线证据演练，不是真实录放音/AEC/模型验收。原始fixture、输出与hash见本包evidence。
