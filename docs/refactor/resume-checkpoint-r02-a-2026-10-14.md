# 当前恢复检查点：R02-A 完成 → R03-A

日期：2026-10-14。分支 `codex/architecture-refactor`，实现提交 **`e557ceb962610004287f38e66b50abca3e44b661`**；最终交接记录另行提交。本包 **10/10**，R03-0仍10/10，L09—L12仍4/4，R01—R06仍 **2/6**；不换算成项目完成百分比。

先读 [checklist](checklists/r02-a-2026-10-14.md)、[四份 HANDOFF](handoffs/2026-10-14-r02-a/README.md)、[验证摘要](handoffs/2026-10-14-r02-a/evidence/validation-summary.json)、[音频证据契约与用法](audio-reference-clock-design.md)及[硬件部署资料](../operations/hardware-deployment-readiness.md)，再核对 branch/status/log。三个 subagent 已交回并停写，验证进程均已退出，没有待恢复的 Cargo、数据库或容器任务。

## 不再重复实施

- native capture 在转换/gain之前与最终 native playback reference 进入既有固定块队列，整批 capture timing 只观察一次。独立时钟和 unknown queue delay 保持原义，不做声学对齐或补零。
- 不可 Clone 的独占 AudioReferenceReader 取代 bus.try_read；观察者取消/finish/drop只释放claim，不关闭设备、不取消播放、不清空队列。成功发布的sequence在queue锁内分配，loss先观察；marker/PCM并发交错保留原时序并在重放显式reset。
- EvidenceSession显式启动/pump/finish；512 records、1024 samples/record、524288 total samples、16MiB JSON硬上限，调用方可收紧。严格导入验证、f64 bits保留、按方向离线重放和footer尾部loss已完成。
- 旧source-WAV recorder、audio_debug配置及调用参数已删除，实际load_config/Settings入口拒绝旧表。忽略跟踪的本地config/embedded.toml没有替用户修改；下次启动前删除其中旧[audio_debug]，不要恢复旧recorder来绕过配置错误。
- R03-0仍保持hardware=false、calibration_confirmed=false默认。用户允许校准后归中不等于已经校准；没有实际校准资料或硬件动作。

## 实际验证和限制

旧实现4个预期RED已复现。当前macOS ARM64上默认features/默认并行 `cargo test -p alia-embedded --all-targets --offline --locked`：**284/0/0**（lib218、integration66），其中evidence13、reader/queue10、clock5、PCA14。659个冻结输入无漂移，全部提交blob与清单相同。

合成示例实际导出、读取并重放7records/16samples/9resets；再次写同路径预期失败，原文件哈希不变且无临时文件残留。第一次all-targets沙箱运行的42项loopback EPERM失败被完整保留；同源同命令允许本地回环后全过。Embedded fmt通过；全仓fmt仅有未改的历史Server导入排序差异。

Linux/OrbStack仍Stopped，本轮Linux/ALSA未运行，没有启动平台、SSH、真实录放音、I²C/UART、模型或外部数据库，没有新AEC/依赖。固定内存tap不等于自动文件录音；ObservedNative/SyntheticFixture不是真实性认证，normalized f64并不保留所有原i64/u64低位。活动快照非原子事务，重开后旧流最终状态可未知；同步Read/Write的阻塞时间不受操作/字节预算保证。未验证真实声学对齐、AEC、漂移、物理取消尾音、长期负载或部署。

## 下一条可执行软件任务：R03-A

**可信动作资产内容身份。** 先检查已有命令签名、plan解析、有界读取、资产目录和配置；先复现同名文件替换未绑定内容的缺口，再补接口与调用方。先确定可信清单/摘要的来源、版本和签名绑定，让同一次有界读取所得bytes同时用于验证与解析，避免校验后重开造成内容替换。拒绝未知/不匹配摘要及未绑定内容的旧命令，不把模型自报hash、文件路径/mtime或临时对当前文件求hash作为信任根。

继续三个subagent按文件所有权分工：Core/Server内容签名契约；Embedded有界读取、prepare和取消；独立复现与审查。Root先协调共同契约，再组合验证、更新checklist/四份HANDOFF/恢复入口并正常提交。若审核者或可信摘要分发方式无法从现有配置和用户授权确定，在完成不依赖该决定的检查/复现后集中确认。保持当前政策、工具可见性/执行批准、精确取消和原native owner清理；不启用hardware、不伪造校准、不自动驱动机构，不新增依赖或扩大平台。

## 现场前置与延期项

已确认资料沿用，不重复询问：当前Mac运行Server；Pi5/8GB、Ubuntu24.04.4、5V5A，地址ke-qiao@192.168.10.221；R1尚未接入，后续USB接Pi；PCA9685A+MG996R，停止/断连保持、校准后允许启动归中。模型服务待后续确认。

扬声器/功放、R1实际固件/格式/音频权限/参考电气与布局仍待落实；执行器供电/OE/限位/校准/保持实证仍缺。PCA9685不因此拥有位置读回、MCU boot或ACK，不能用软件提交冒充到位。R02/R03现场出口满足后才进入R05整机通过，包括至少24小时混合负载和Settings剩余等待/取消故障实证。V01、前端、通用Embedded适配、额外Server平台与端侧/同机部署继续在[延期台账](deferred.md)保留。
