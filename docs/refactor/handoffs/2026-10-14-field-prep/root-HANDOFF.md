# R02／R03 现场前置 Root HANDOFF

日期：2026-10-14；基线 `283f38f`；分支 `codex/architecture-refactor`。范围为现场前置的离线软件与资料补缺，不新增依赖、真实录放音/执行器或平台。本包10/10，实现提交 `e409dc944a589104d0e2d2c2977f642ad8aaef28`；完成数见[checklist](../../checklists/field-prep-2026-10-14.md)。

## 分工与决策

继续使用三个既有 subagent：settings_auth_reproduce 检查 R02 和音频文档；settings_auth_fix 实现 motion 文件检查并清理资产操作资料；settings_auth_review 准备 staging 与部署文档并独立审查。Root 负责 CLI/preflight、进程级回归、现场记录、测试和提交。仅 Root 运行构建与 Git，无 agent 操作设备或外部服务。

先检查后确认：原 main 未处理 CLI 参数，正常启动会加载/创建身份、保存配置、进入 Builder 并打开选定设备；不能安全地把旧二进制的 `--help` 或“检查”参数当 dry-run 来试。没有为复现而执行旧正常入口。新进程回归提供 noop 保护配置与不可创建的 key 路径，若退回旧路径也会在设备构造前失败。

用户已有 Mac/Pi、R1 USB、PCA9685A/MG996R、停止保持和校准后启动归中的决定不重复询问。集中问题后用户回复：R1尚未接入Pi，允许先用Mac模拟，但尚无AEC参考接法/电气规格；R03机构资料全部未确定，后续先暂停；部署资料待定，Pi可SSH登录。R02据此使用原合成PCM工具，不新开真实录放音或模型；R03已完成的软件保留，但待恢复指令前不推进现场。SSH许可记录，不因许可本身重复盘点或改变Pi。

## 最终行为与限制

`--check-config <path>` 必须指向已存在普通文件，配置最多1MiB；typed schema、路径解析、设备运行界限和 speech 契约复用现有实现。它在 Tokio、logger、KWS环境入口、identity 和 Builder 之前退出；帮助与无效参数同样不进入启动。无参数正常运行保持原行为。

motion inspection 不构造 MotionRuntime/backend/worker，复用原规则。所选 PCA 即使 hardware=false 也静态验证地址覆盖/重复，但不新增电气或频率判断；运行时原触发条件不变。缺失未确认校准报告 not_checked，已存在文件解析并核对 count/center；确认缺文件或错误拒绝。普通文件/no-follow/nonblocking 与1MiB界限阻止常见特殊输入；批准资产复用原有有界同 fd 内容身份及帧解析。报告不建立批准，不变更 calibration_confirmed，不打开配置的 I²C/UART 路径。

输出选择性配置事实，未输出完整配置、密钥或 token。hardware_verified=false、physical_state=not_observed；模块名实际可实例化性、feature与模型可用性、设备权限、Server清单一致性、物理校准/AEC不由该入口证明。同步文件 I/O 没有墙钟期限；本地配置及父目录仍为操作者信任前提，不隔离能修改宿主文件或部署插件代码的主体。

新增 staging 示例明确七槽 noop，设备/发现关闭、默认绑定、独立相对路径与 loopback:0。只检查模板，不启动模板；以后普通启动仍会创建 key/SQLite 并监听。清理 speech.enabled、run/audio-debug 默认目录和不存在的旧校准工具说明；保留现有 synthetic replay，不新增 recorder/consumer/AEC。

## 验证与资源

最终 macOS26.6.2 ARM64 / Rust1.95，默认 features/default parallel Embedded all-targets **316/0/0**（241 lib+75 integration，无过滤，45.05s）；no-default CLI **5/0/0**（17.77s，为同5项重复验证）。两示例实际离线检查exit0，输入hash前后不变。665项最终输入manifest `dd888cec2f84c06a213cadc236911d088c89015b144025284e8eab72070d3573` 与实现提交全部blob相同，source_drift=[]。

首轮candidate（文件保留green命名）80.86s exit101，已完成结果287通过/1失败，后续目标未跑；Root新增fixture只写keypair_path，漏了现有必填device_id/device_name，typed parser正确拒绝。只修fixture后重新冻结最终输入并完整回归，不当生产RED、不丢失败记录。变更Rust和diffcheck通过，全仓fmt仅未改Server delivery.rs历史import排序差异。

四个validation wrapper已分别以101/0/0/0退出，没有启动需要停止的任务服务、容器或设备；临时fixture由测试清理，忽略的构建/证据目录保留。Server源未改，因此本轮不启动PG或重跑Server；Linux/Pi和普通staging runtime未运行。所有新测试采用临时文件与 mock；本轮没有 SSH、Docker/数据库服务、Pi、native音频、执行器、模型或依赖安装动作。先前共享服务状态不由本轮重新检查或改变。

## 后续

下一步优先继续R02 Mac离线模拟及回填[集中现场资料](../../../operations/hardware-deployment-readiness.md)，再按其步骤安排指定目标只读核对和明确现场窗口。R03后续已由用户暂停，实际动作审核、双端清单部署和机构校准待恢复；R02声学证据仍开放；没有把报告 checked 变成 calibrated。R02/R03 出口满足后才进入 R05；V01、Settings剩余故障、前端、额外平台/通用嵌入端/端侧模型及同机部署继续延期。

用户允许Mac模拟后，本轮另实际运行现有无默认features合成PCM示例，exit0、4.66s：7条记录、16个样本值、9个重置提示，保留2个丢失播放块、独立时钟未对齐和旧采集流结束状态unknown。文件标记SyntheticFixture，physical_stream_complete=false；只是离线证据演练，不是真实录放音/AEC/模型验收。原始fixture、输出与hash见本包evidence。
