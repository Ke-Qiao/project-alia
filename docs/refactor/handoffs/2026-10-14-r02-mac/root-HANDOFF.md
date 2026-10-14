# R02 Mac 离线模拟 Root HANDOFF

日期2026-10-14，基线`ea3f0e4`，分支`codex/architecture-refactor`。用户要求三个subagent先检查再补缺口，保留单consumer、精确取消及清理；R03继续暂停，无新依赖或真实录放音/执行器。

## 分工与先有实现

继续复用settings_auth_reproduce（音频证据库）、settings_auth_fix（组合模拟测试）、settings_auth_review（独立审查）。Root负责example CLI、统一Cargo/证据/文档与Git。既有队列claim、cancel/finish/drop、不补零、512records/1024单块/16MiBJSON限额已成立，没有再造框架或consumer。

旧example仅手填7条fixture，单参数必生成新文件，多余参数忽略；不能检查已有证据，输出硬编码最新playback与旧capture状态。局部capture/mixer/evidence单测存在，但没有将实际软件边界连到同一证据会话和读回重放。

## 复现与改动

基线no-default example实际构建后，在临时目录执行--help，exit0并生成名为--help的文件，真实行为已保存；未在仓库或设备目录试写。新的generate/replay严格检查arity，旧隐式路径拒绝，help只输出usage；generate仍临时文件+sync+persist_noclobber，replay读同一个已打开普通文件，经原有read_json限额/契约。末级symlink/目录/FIFO拒绝，父目录仍可信本地输入；同步文件I/O不保证墙钟上限。正常错误清理未发布temp，但发布后读回失败保留已发布文件，kill/断电不承诺无残留。

通用摘要按stream呈现记录、帧、样本、reset计数、最后序号、observed close marker与匹配latest snapshot。空记录、单方向、仅footer正常；旧stream没有自己的close或对应closed snapshot时final未知。source_authenticated=false，不从ObservedNative声明建立信任，不把队列sequence差额统称producer loss；physicalcomplete恒false、采播独立时钟未配准。

Closed摘要此前只要求loss<=attempted与每record的前缀计数，因此可同时宣称所有attempt/PCM均丢失和已观察成功记录。新回归真实失败：0/1/0、245filtered，16.85s exit101，错误同时列records/frames，666项RED输入无漂移；非编译失败。修复仅对closed按checked运算要求observed_records<=attempted-lost_blocks与observed_frames<=cursor-lost_frames；允许部分/零记录观察，不强化active快照。现有物理owner关闭协议保证最终计数，其证明和反例见audio/review交接。

4项新组合测试以合成PCM走现有capture helper（跨1024样本块）、voice+cue最终callback、单EvidenceSession、JSON往返和replay；验证cue精确取消不伤voice、明确暂停保留voice且cue不重放、观测取消/drop不关闭输出owner、真实队列满/丢close/reopen。检查PCM、submitted账本、未证明presented、buffer/renderer/source_bytes/claim清理，不开启设备、不公开测试专用生产API。5项example测试覆盖参数、动态摘要、只读/不覆盖、取消与特殊文件。

## 验证与资源

本包 **10/10**，实现提交 `160704578cd79d23898e6c8225d18c8511895d39`。最终冻结666项输入，manifest SHA-256 `ef33fcd714968f0236f42a065783c9967f23f7b0ac22deee6f120b376efef7f8`，逐项与提交blob匹配且无漂移。

| 实际命令（均offline/locked） | 结果 | 计数边界 |
| --- | --- | --- |
| `cargo test -p alia-embedded --all-targets --offline --locked` | 327/0/0，84.00s | 247 lib +75 integration +5 example |
| `cargo test -p alia-embedded --no-default-features --lib --example audio_evidence_replay --offline --locked` | 252/0/0，35.63s | 重复247 lib +5 example，不累加独立数量 |
| `cargo build -p alia-embedded --no-default-features --example audio_evidence_replay --offline --locked` | exit0，1.43s | 供实际CLI验证的最终二进制 |
| `python3 run/validation-20261014-r02-mac/cli-smoke.py` | 九场景符合预期，0.70s | 成功与预期拒绝都核对退出码、文件字节和无临时泄漏 |

CLI覆盖help无写入、旧隐式参数/额外参数拒绝、缺失文件、新生成/重放、不覆盖、历史fixture只读重放和矛盾closed拒绝。报告7records、16samples、9resets及3streams；source未认证和physicalcomplete=false。输入fixture SHA-256为`374d84a56124ab88aecf40aaaa8225c2df6f813b3c860e9061efa953a38d1b49`。四项组合回归与五项example测试均有逐项通过记录，库共15项。

变更Rust的rustfmt与diff检查通过；全仓fmt只在未改动`packages/alia-server/tests/server_runtime/delivery.rs`的历史import排序失败，不据此宣称全仓fmt通过。macOS26.6.2 arm64，rustc/cargo1.95.0；所有运行wrapper已退出收集，CLI临时目录已删除，正常错误无未发布temp残留。保留ignored构建缓存和验证原件，按字节复制到[evidence](evidence/evidence-index.json)。

三个subagent完成音频6/6、模拟8/8、独立审查7/7后停写，无阻断项。R03 source未改，无Server修改/数据库服务、SSH、容器或模型；整包测试仅使用临时文件、SQLite、loopback及合成软件音频。本轮未跑Server、Linux/Pi/ALSA、staging runtime、真实模型/硬件或长稳，不将默认模型feature编译视为模型推理验证。

## 接续

下一项可先有界审计现有独立时钟、gap/reset与实际AEC consumer的帧需求，明确待测漂移/对齐契约后再决定软件缺口；不重复固定fixture演练，也不在尚未确认实际consumer时新增AEC、重采样或迁移框架。


R02后续仍需真实输出链/参考电气、用户录放音窗口和保留规则，再开展AEC/双讲/尾音；本包不模拟实际房间声学、Easy-Turn推理或生产AEC。R1仍未接Pi；R03按用户决定暂停，既有软件保留，待恢复后处理板卡/通道/供电/OE/校准。部署账户/目录/模型仍待定，Pi SSH许可保持但本轮不用。R05等待实际R02/R03出口；V01、Settings剩余故障和其他延期沿用台账。
