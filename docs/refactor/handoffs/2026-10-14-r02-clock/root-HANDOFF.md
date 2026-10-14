# R02 独立音频时钟与 consumer 契约 Root HANDOFF

日期2026-10-14；基线`38fb689`，分支`codex/architecture-refactor`。用户要求先审计和明确漂移/对齐契约后补缺口，继续三个subagent；R03暂停，不新增依赖、不启用真实录放音/执行器。

## 范围与先有证据

读取上一轮四份交接、R02-A设计/当前参考总线、时钟、replay及实际speech路径。原单reader、512条/16MiB会话、真实PCM/静音分类、closed统计、观察取消与owner生命周期均已实现；上轮默认327/0/0是历史依据，不计成本轮结果。基线clean。

复用settings_auth_reproduce审计并修复clock与native分块；settings_auth_fix审计并修复reference/replay；settings_auth_review核对AEC consumer帧需求并独立审查。Root汇总契约、统一Cargo、证据、文档与Git。没有同时运行多组Cargo或让agent自行启动资源。

## 明确的契约

[设计契约](../../audio-reference-clock-design.md)先于生产修复记录。采播独立域与native/converted游标不混用；每个native batch只观察一次锚点，拆块sample时间为nominal offset，不能当独立漂移观测。CPAL callback/sample均为后端估计，负sample offset与callback相等合法；ALSA只观测status/delay，不造sample timestamp。主机观察时间不等于样本时间。

未来映射`p=a*c+b`与ppm仅为条件定义；当前没有可校准的跨流锚点/精度界或实测延迟，因此不输出猜测系数/阈值。帧、缺口、时间变化与stream更换应使相关consumer状态失效，恢复需重新建证据；真实静音保留，缺失PCM不补零。

实际仓库没有AEC processor/feature或自动reference worker。1024 interleaved样本块、最多1600样本的16k mono上传块、VAD窗口、rubato约10ms内部块是不同用途；未选AEC帧长。硬件AEC是否仍需host算法待R1参考/格式确认，取得reference reader也不会自动改变converted speech→VAD/KWS/upload路径。本轮无需重复询问未变的硬件资料，不建立未来框架。

## 实际 RED

在生产保持基线时新增3项用例并冻结666输入，RED manifest SHA-256 `f492372c783673beac85b356b3e0d01fdd1e4b165ceed22d679a77d5d9816272`。Root执行无默认features的lib三filter，exit101，11.08s，**0通过/3失败/0忽略/247 filtered**，source_drift=[]；实际业务断言失败，不是编译或环境失败。

1. callback100→200而sample−10→−20，旧clock未分段。
2. 回退播放batch含2052个stereo样本，拆成3块后quality为`[Unknown, BackendEstimate, BackendEstimate]`，同一已拒时间在后两块误恢复。
3. 历史合法PCM记录八种quality/可用性/回退变化经read_json保留后，replay全部缺少reset。

原始日志、RED patch和独立清单保留；最终结果不覆盖RED。

## 实施

native clock独立检查callback/sample锚点的严格回退，整批降Unknown并发布原ClockRegression连续性标记；actual cursor不回退，真实PCM不删除。capture/playback共用私有已观察批次分块helper，不再让playback每存储块重新observe。helper不能修改clock，保留边界、content、样本归一化、尾部完整帧规则及空playback原行为。

回放在原FrameGap的前一PCM查找中新增`ClockDiscontinuity`：同stream相邻PCM的quality/可用性变化及严格时间回退才提示；忽略marker的空时钟，不从queued delay、host time或名义频率推断漂移。保留历史输入、schema、原时间与PCM；现有example按真实reset动态汇总，没有硬编码分支。

取消、单consumer、精确输出身份、原owner关闭/资源清理路径均复用，不修改R03、Server、依赖/配置/协议或默认政策。

## 最终验证

本包 **10/10**，实现提交`28920fb92e1785f0e8880309f1b4c7b1193905fa`。最终666项源码/构建输入manifest SHA-256为`fb4a641051ef77125d788d7013487dcc7fda488e43c74044a0690c5767144fe7`，与提交blob逐项匹配，各轮无源码漂移。

首次完整默认验证在沙箱内exit101、51.80s，lib212通过/43失败/0忽略；43个panic全部来自现有测试夹具绑定`127.0.0.1:0`被EPERM拒绝（playback21、body9、duplex13），新增8项均通过。保留失败日志和定位，没有改源、跳过测试或调整并行度；随后通过工具批准允许本机临时回环端口，以同一manifest重跑。

| 最终实际命令 | 结果 | 边界 |
| --- | --- | --- |
| `cargo test -p alia-embedded --all-targets --offline --locked` | **335/0/0**，68.75s | 255lib+75integration+5example，原八项新回归与四项组合模拟均逐项通过 |
| `cargo test -p alia-embedded --no-default-features --lib --example audio_evidence_replay --offline --locked` | **260/0/0**，39.55s | 重复255lib+5example，不相加为独立测试 |
| `cargo build -p alia-embedded --no-default-features --example audio_evidence_replay --offline --locked` | exit0，1.49s | 最终CLI二进制 |
| `python3 run/validation-20261014-r02-clock/replay-smoke.py` | **2/2**，0.60s | 新时钟变化证据及上一轮历史fixture只读重放 |

新SyntheticFixture为3条PCM记录/12样本，报告FirstObservation1次、ClockDiscontinuity2次；来源未认证、物理完整性=false、没有matching latest快照。旧fixture仍7records/16samples/9resets，输入字节不变。此纯文件演练不播放音频；新增fixture作为证据保留。

变更Rust格式与diff检查通过；全仓fmt仍只报告未改动Server `tests/server_runtime/delivery.rs`的历史import排序。环境为macOS26.6.2 arm64，rustc/cargo1.95.0。所有测试wrapper已退出收集，测试自身临时文件/SQLite和回环RPC随其生命周期结束；保留ignored构建缓存与验证原件，未启动部署服务/外部数据库/设备/模型，也未SSH。Server、Linux/Pi/ALSA runtime、staging正常启停及长稳本轮未运行。

三个subagent完成clock6/6、reference8/8、review7/7后停写，无阻断项。原始日志、RED补丁、两份manifest、合成fixture、静态结果与提交核对按字节归档，见[验证摘要](evidence/validation-summary.json)及[证据索引](evidence/evidence-index.json)。

## 接续与未验证

本轮审计不选择AEC consumer，不实现声学对齐/漂移补偿；R1未接入、参考电气/固件/扬声器/功放、真实录放音窗口/PCM保留规则仍待定。下一步先确认实际硬件AEC路径与目标consumer及其帧/延迟契约，再决定软件对齐/组帧接入；条件未齐时只能继续明确范围的离线证据工作。

R03继续暂停，既有校准/保持/信任决策保留；部署账户/目录/散热/模型未定，R05现场与长稳未通过。V01、Settings剩余故障、前端/其他平台/端侧模型/同机部署继续按台账延期。
