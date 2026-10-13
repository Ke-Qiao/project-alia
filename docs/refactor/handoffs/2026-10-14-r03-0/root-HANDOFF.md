# Root — R03-0 最终交接

日期：2026-10-14。分支 `codex/architecture-refactor`；起点 `125ea5844284efc39ea80c70c543add74ff5e895`；已测试源码提交 **`b5100d778a53dda9f3adc55fd0309a5eb5c853e9`**。本包正常提交实现，随后单独提交证据与恢复记录。三个 subagent 已交回并停写，无待恢复测试任务。

## 完成的行为

停止不再创建 local reset permit 或追加归中/disable 帧；进入中的 syscall 仍归原 writer，丢弃等待者不能丢失 join。lease Close、binding revoke、到期、observer drop 各自保持原权限语义。

新增本地 `calibration_confirmed=false`：未确认 mock 零输出/不加载矩阵/不可用，native 未确认在构造前拒绝；确认后仍验证校准文件、维度与中心范围。启动只完整提交一次中心目标后建立 previous，初始位置保持未知，后续软件目标继续限速。四个旧 reset/soft-start 键拒绝解析，JSON 更新失败不改变 RAM/文件。默认 hardware=false，无真实配置改写。

PCA 保存原 MODE1/PRE_SCALE，开启 AI 并保存地址模式位；未知 EXTCLK 在数据写前拒绝。首个可能进入 SLEEP 的写开始即承担恢复责任，包括短写/错误结果不确定。取消不打断必要收尾，原 awake 尝试恢复运行，原 asleep 恢复后不擅自唤醒。多板构造未提交时后板失败会逆序恢复此前板，单板恢复失败不丢掉其他清理，原错误和 recovery unknown 都保留。恢复限制是事务数量，不能保证阻塞 syscall 的物理时间或真实输出恢复。

生产修改只在 Embedded Motion/配置；真实 gRPC fixture、配置示例、使用文档同步。没有协议、工具政策、权限、依赖或额外平台变更。精确目标取消、部分/不确定通道与 position_confirmed_actions=None 保留。

## 实际验证

| 阶段 | 结果 | 证据 |
| --- | --- | --- |
| RED | 5 项预期失败；stop 3 vs2 帧、未确认启动1 vs0、PCA burst/取消/写失败 | `red-source.patch`、`red-source-manifest.json`、3组red日志及json |
| macOS 默认 Embedded 全 targets | **199 lib +64 integration =263/0/0**，exit0，89.4秒；PCA14项均通过 | [GREEN](evidence/green-embedded.json)、[原日志](evidence/green-embedded.log) |
| 源码身份 | 657输入测试前后无漂移；提交 blob 的 SHA-256 657/657匹配 | [提交核对](evidence/commit-verification.json) |
| 格式 | Embedded通过；workspace失败只因未改基线Server import顺序 | [基线核对](evidence/preexisting-format.json) |
| 独立审查 | lifecycle、PCA及实际日志/哈希已读，无阻断项 | [Review](review-HANDOFF.md) |
| Linux | Docker daemon/OrbStack停止，未启动其他机器或容器，未执行 | [环境](evidence/environment.json) |

RED 模型只为最初AI/取消/错误见证；GREEN补活动PWM与RESTART W1C状态，避免将状态位当普通可恢复配置。没有通过调整生产来迎合错误模型。复现与GREEN使用各自固定清单，不把两者混为同一源码。

Cargo阶段已退出。没有外部PG、Docker、SSH、真实设备或部署进程；Embedded回归用临时SQLite、loopback服务及模拟驱动，关闭/取消回归通过。没有真实断电、首段物理速度、持续力矩、实际音频、长稳或本轮Linux证据。未使用reset卡。

## 下一轮与未完成

本包 [checklist](../../checklists/r03-0-2026-10-14.md) **10/10**；R01—R06仍 **2/6**，L09—L12 **4/4**。下一条软件主线是 **R02-A 双流有界证据与合成PCM离线重放**，可与 **R03-A 受信资产内容身份** 独立推进。继续三个subagent，先检查既有实现/测试，只补实际缺口。

R02-A沿已有单consumer reference、capture timing/continuity，callback不写文件；先合成数据，设备未接入不阻塞离线工作。真实录音需明确采集和保留范围；R1的功放输出参考与USB采集不同，未确认电气和声学前不叠加新AEC。

R03-A仍需可信资产清单/摘要来源、版本拒绝、签名绑定与同次读取校验/解析；不能把模型自报hash或设备当前文件当信任根。现场校准、供电/OE/限位及物理保持仍需取得证据，当前不得为了验证而开启hardware或校准确认开关。

Linux可用时补同源Embedded回归；R02/R03现场出口完成后进入R05，并处理Settings已记录的等待/取消故障验证。V01和其它延期项不在本包扩大，详见 [延期台账](../../deferred.md)。
