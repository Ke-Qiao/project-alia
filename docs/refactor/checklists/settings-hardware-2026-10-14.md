# Settings 有界审计与硬件主线切换

日期：2026-10-14。基线 `66ae27ec1f19a69bbac2765938fdb18efdd35deb`，分支 `codex/architecture-refactor`。本包只关闭审计与准备项，不以软件测试代替 H01—H05、AEC、机械反馈或 R05 整机验收。

- [x] 01 读取上一轮检查点和实际源码，继续由三个 subagent 分工。
- [x] 02 固定 GET/PATCH、共享 privacy gate、Registry/模块/SQL/文件的锁和取消所有者。
- [x] 03 记录具体等待风险和现有预算；保留权限、政策、部分成功和显式重试，不增加整请求 timeout。
- [x] 04 两项确定性 Registry 排队/持锁取消回归通过，并保留失败清理。
- [x] 05 执行相关 Settings 回归和当前 Embedded 软件基线，明确平台与硬件边界。
- [x] 06 整理硬件/部署事实、未知项与只读核对步骤，集中向用户获取 H01—H05。
- [x] 07 把主线切为 R02/R03，固定首个交付包及 R05 进入条件；不继续自动衍生 R04 轮次。
- [x] 08 完成独立审查、证据、四份 HANDOFF、恢复检查点和正常提交。

本轮审计与准备 **8/8**；H01—H05 尚未逐项完成实际部署确认。R01—R06 仍 **2/6**（R01/R04），L09—L12 **4/4**；均不是项目完成百分比。

资料入口：[硬件与部署清单](../../operations/hardware-deployment-readiness.md)、[锁与取消审计](../handoffs/2026-10-14-settings-hardware/flow-HANDOFF.md)、[回归边界](../handoffs/2026-10-14-settings-hardware/regression-HANDOFF.md)。

Settings 剩余可用性风险：慢模型持配置读锁可阻挡 Settings apply 及后续读取/隐私修改；SoVITS 权重设置前还有音频额度等待；同步文件保存不可被 Tokio 抢占。已列入 R05 前故障验证，不将“完成审计”解释为“等待全部有界”。

测试源码 `cffbca531cec2478c17b5bcacfd191e7f00a56f9`。当前验证：Settings原回归5、新取消2、存储7、Embedded默认lib177，全为0失败/0忽略。见 [最终交接](../handoffs/2026-10-14-settings-hardware/root-HANDOFF.md#root-最终收口)。
