# R02 Mac 离线模拟 — 独立审查交接

日期：2026-10-14。Root 指定基线 `ea3f0e4` clean；分工 `settings_auth_review`。本轮仅审查 Mac 离线模拟、证据导入与重放操作，不开展真实录音/播放、AEC、模型、Pi/Linux 或外部服务。R03 依用户要求暂停。仅本文由本分工编辑，生产源码、测试、包文档由各 owner 管理；本分工不运行 Cargo、Git、SSH、Docker 或设备。

## Checklist

- [x] 读取 R02-A 与 field-prep 的交接、验证摘要和现有音频说明，沿用已完成证据。
- [x] 独立追踪 EvidenceSession、AudioReferenceReader、取消/关闭和限额资源归属。
- [x] 审查 read_json/replay 的输入与信任边界，以及当前 example 参数与输出行为。
- [x] 向 Root 和两位 owner 提供有界缺口、通用摘要与最少回归建议。
- [x] 独立审阅完整候选及必要修订，不代替 owner 编辑。
- [x] 读取 Root 实际验证结果、清单和失败证据，不将历史结果计为本轮通过。
- [x] 汇总最终结论与剩余边界并停写，提交由 Root 收口。

当前 **7/7**，属于本分工进度，不是项目或 R02 硬件完成比例。本文已停写；最终提交与提交 blob 对照由 Root 完成。

## 沿用证据，避免重复改造

R02-A 已在同一固定内存队列接入 native capture 和最终 native playback，reader 排他、会话显式有界、f64 bits 导出/导入及 gap/reset replay 已实现。当轮 Mac 默认 Embedded 为 284/0/0，evidence 13、native clock 5、reader/queue 10 项通过；固定 synthetic example 实际输出 7 records / 16 samples / 9 resets，并保留已知丢失和旧采集流 final unknown。该轮拒绝覆盖复执行保持原文件字节且无临时残留。以上来自已读归档 `2026-10-14-r02-a/evidence/validation-summary.json`，不作为本轮新测试。

field-prep 已交付纯文件 `--check-config` 和无设备 staging，默认 Embedded 316/0/0、no-default CLI 5/0/0；新增 5 项 CLI 是相同测试的另一 feature 运行，不累加独立用例。用户后续允许 Mac 模拟，已执行无默认 features 的旧 synthetic example，结果仍为同一固定 fixture。该事实允许本轮继续离线模拟，不代表已授权真实采播或打开 staging runtime。R1 上次仍未连接，R03 后续暂停。

## 现有取消与单消费者保证

`AudioReferenceBus::try_claim_reader` 用 atomic compare_exchange 取得非 Clone reader；只有 reader 的 `try_read(&mut self)` 可 pop，共用 bus clone 不能偷取已有 owner 的 PCM。reader Drop 仅释放 claim，不清空队列，也没有 native close/cancel 能力。消费者取队列使用普通锁；producer 仍只 try_lock，争用与容量满通过 attempt/loss 记录，不等待消费者。新工具应复用这些所有权，不增加第二个自动 reader 或后台任务。

EvidenceSession 持有 reader、观察取消 token 和有界数据；pump 在块间检查取消，最多拉取调用者限定的记录。cancel/limit/invalid 会停止继续观察，但 claim 直到 finish 或 Drop 才释放；这不等于停止设备。finish 消费会话，附上两个方向当前最新 snapshot，正常返回或错误返回均释放 reader。已 pop 而被样本/字节上限拒绝的一个块另记 discard。现有回归已证明取消/drop 后 native 仍 running、剩余 PCM 未被清空且后续 reader 可重建，不应为本轮 CLI 重写这些行为。

write_json 先验证并在内存构造有界 JSON，再分块写 caller-owned sink；取消和失败保留已写前缀，调用方负责目的文件。read_json 先限制总字节，再限制 records/每块 samples 数组长度并进行合同验证；取消检查发生于同步操作之间，不能中断已进入的 Read/Write。replay 自身是至多 512 records 的有界同步迭代，没有后台 worker 或设备生命周期。字节/操作界限不构成文件系统墙钟时限。

## 输入与 replay 信任边界

现有验证覆盖 version/非空 boot/stream、方向/格式/clock 身份、帧与样本长度、有限 normalized 样本、当前 CPAL/ALSA 边界与容器、真实零样本静音、sequence/loss 单调、同 stream PCM 不重叠及表示不变。严格拒绝未知嵌套字段、截断和解析期数组超限。marker 与 PCM 并发交错可使 continuity/marker cursor 回看，现有代码保留实际 metadata 并输出 reset，不应将合法交错一律拒绝。

active snapshot 各字段是独立 atomic 观察，不能要求它们同一时点一致；record 的两个 loss 计数同样顺序读取。closed snapshot 才按关闭协议校验 final counters 不早于已有 records。每方向只保留最新流 snapshot；新 stream 不能继承旧流状态或证明旧流已结束。replay 只输出观察到的记录和 reset 信号，不补零、不配准、不播放。

`ObservedNative` / `SyntheticFixture` 是文件内的来源声明，不是签名、认证或真实硬件证明。JSON 格式合法也不意味着内容真实。软件 close marker 与 closed snapshot 仍不是实际扬声器停止或物理完整性。首次审查没有提出重写此库；随后库 owner 找到 closed footer 与已保留记录相矛盾的具体缺口，独立核对如下。

## Closed footer 新缺口的独立判定

库 owner 提出：现有 fixture 每流已保留 Started、PCM、Stopped 三条成功记录，final attempted=3；把 closed lost_blocks 改为 3，旧验证仍可接受，导致“全部尝试都丢失”与已观察成功记录矛盾。把 closed lost_frames 改成全部 cursor，同样可能与已保留 PCM 矛盾。Root 安排先由该 owner 写最小 RED，再统一运行，生产修复须等实际 RED；本分工没有执行复现。

已独立追踪实际发布路径，确认应仅对 closed snapshot 加两个保守下界：同一完整 stream 身份的 observed records 数量不大于 `attempted_blocks - lost_blocks`，observed PCM frames 总和不大于 `cursor - lost_frames`，减法和累加使用 checked 运算。每次 publish 恰增 attempt，失败才增 lost_blocks/lost_frames；marker 为零帧。capture helper 整批 observe 一次，再按不重叠完整帧块发布；CPAL render 每块 observe，ALSA 只对实际成功接受的前缀 record_samples。CPAL native stream 在 owner close 之前被 drop，ALSA 同线程停止后关闭 owner；closed 是原有统计发布边界。

只能用不等式，不能强制总量相等：会话可能只看到后缀、未消费完队列、达到限额或丢弃非法块。marker/PCM 合法交错不改变总量，active snapshot 仍不新增这类约束。原 reference 并发测试故意重复 clone 同一个 PCM 块来测试队列守恒，未相应推进 cursor；该模型不进入 AudioEvidence 验证，也不能拿它作为真实音频证据来反对新的下界。此补丁约束格式自洽，不给来源增加认证能力。

## 已确认的 CLI 操作缺口与范围建议

基线 example 只取 `args_os().nth(1)`，始终生成固定 synthetic fixture；不存在只读检查已有证据的命令，多余参数被忽略。误用类似 `--input /path` 会把 `--input` 当作新的输出文件名，而不是拒绝错误模式。此结论来自源码静态路径，未运行复现或创建文件。现有 summary 也依赖固定 fixture，直接 unwrap 两个方向 snapshot，并写死旧 capture final unknown；不能直接用于一般输入。

Root 选择的有界补包与真实空白吻合：example 显式 `generate <newfile>` / `replay <existingfile>`，严格检查模式/参数数目和帮助；生成保留 temporary + sync + persist_noclobber，replay 使用同一已打开普通 fd 的 nofollow/nonblock/metadata 与原 read_json cap，不隐式生成。参数错误应在任何创建临时文件前拒绝。输入缺失、目录、symlink、FIFO、截断、超限、非法合同必须明确失败；已有目标、hardlink/symlink 冲突不得改写目标。正常返回错误时清理临时文件，与 SIGKILL/断电后是否残留是不同保证，不为本轮增加新平台或信号框架。

通用 summary 应由实际 records、replay resets 和 footer 派生：逐 stream 统计观察到的 PCM frames/samples、reset 原因和 close marker，snapshot 匹配时单独呈现 running/closing/closed；空 records、单方向或只有 footer 均可报告，不使用 unwrap 假定双流存在。不匹配最新 snapshot 的旧流终态保留 unknown。`attempted_blocks - last_record.sequence` 是未观察 attempts，未必全为 producer 丢失；final loss 与 record.lost_before 的差也可能含此前滞后计数，不能一概标为精确尾部丢失。输出应保留终止原因、discarded、unobserved_history_possible，并明确 source 未认证、独立 clocks 未配准和 physical completeness=false。

模拟 owner 负责将实际 capture helper、post-mix renderer、同一 EvidenceSession、导出/导入/replay 串联。基线 evidence fixture 手工构造 AudioReferenceBlock，example 完全手工构造 EvidenceRecord，因此该组合见证有价值；应使用内部测试入口，不为测试公开新 producer API。重点比较实际产物 bits/postmix、hold 真零与丢失区别、独立 clock、游标，以及观察取消与 native owner/reader 重建边界。Root 统一运行与收口，本分工等待候选完成后独立读回。

## 已独立读取的实际复现

`run/validation-20261014-r02-mac/baseline-cli-reproduction.json` 保留基线 `ea3f0e4b3c6c7e7920fa9c4be2d4480dabeadce2` 的源码/二进制 hash 和独立临时目录执行结果：旧 example 以单参 `--help` 运行，exit 0，但创建了名为 `--help` 的 synthetic 文件并输出 fixture 摘要。这是实际操作缺口，不再仅为静态推演。

`red-closed-footer.json/log` 实际执行 no-default library 定向用例，exit 101，16.85 秒，manifest `1f7c36444fe03f3c8b6fc3a60747c3446b8cb5f50a9e227244e0be6470c17e4e`，`source_drift=[]`。用例先成功载入原 fixture，再分别修改 closed lost_blocks/lost_frames，最终断言明确报告 `contradictory closed snapshots accepted: ["records", "frames"]`；1 项预期断言失败，非编译或环境失败。Root 据此允许库 owner 修补，GREEN 尚待统一验证。

## CLI 与模拟候选读回

已完整读取 Root 的 example 与五项 unit。execute 对模式、参数数目和 help 的判断先于任何文件操作；不再隐式把旧单参当生成路径。generate 保留新临时文件、同步和 persist_noclobber；replay 预检普通文件，并用同一 nofollow/nonblock 已打开 fd 复验 metadata 后交原 read_json。摘要使用已验证 stream ID 聚合，库已先保证同 ID 不跨完整身份/格式冲突；逐流保留真实 observed frames/samples、reset 分布、close marker 与 matching snapshot。没有尾部损失差值推断、固定双流 unwrap 或来源认证声明。空 records、单方向、footer-only 与未认证 ObservedNative 均有用例。

已建议在原文件拒绝测试中增补 hardlink 目标 bytes 不变；现有 noclobber 逻辑本身正确，此项为验证增强。另提醒 Root：persist 成功后的再读或取消失败会保留已经发布的新文件，不能用“命令返回错误”推断“没有发布”。正常错误回收临时文件不涵盖进程被杀或断电；不扩新信号机制。

已完整读取模拟 owner 四项 `playback/tests/offline_evidence.rs` 及 tests.rs 私有模块登记：实际 capture helper 的 2052 个 stereo 样本产生 512/512/2 帧块；实际 Renderer/PlaybackBuffer/CPAL fill 的 1.0 voice +0.2 cue 结果、精确取消后的 voice 尾部，经同一个 EvidenceSession 导出/载入后逐样本核对。ExplicitWait 的真实零输出不推进 source submitted，ExplicitContinue 恢复 voice 而取消 cue；观察 token cancel 与 session drop 后，native owner 和待播 PCM 保持，再 claim reader 后可继续记录。

溢出用例按实际固定64块队列制造旧 capture close marker 丢失，随后 playback 的 start/PCM/stop 全丢而只留下 closed footer；该 footer 为 cursor10、attempted3、lost_blocks3、lost_frames10，且 observed playback records 为零，故新下界必须接受。capture reopen 只保留新 snapshot，旧状态未知，replay 不补丢失样本。四项均显式清理 sender、buffer、renderer，核对 source_bytes=0、media 为空、owner quiescent，且 presented_source_frames 仍为 None。没有新生产接口、真实设备、网络线程或硬件声明。当前静态审查未见阻断；尚待库候选与统一 GREEN。

库 GREEN 随后已完成并独立读回：先沿原逻辑校验匹配 record 的完整 stream 身份/format，再仅在 closed 分支 checked_add 累加成功观察数量，最后 checked_sub 校验两个下界。原 `lost_blocks<=attempted` 被更完整的减法条件包含，不降低已有约束。新增 partial/空 records 合法、`u64::MAX` loss 下溢拒绝；active 用例同时保留 `lost_frames=u64::MAX` 仍可接受，防止误把非事务观察强化成 final。库测试共15项（原13+本轮2）；原限额、取消、reader 和 native 输出代码未改。所有候选静态审查完成，无阻断；等待 Root 冻结与实际 GREEN。

## 最终运行证据独立复核

已逐项读取 `run/validation-20261014-r02-mac/` 中 final-source-manifest、四次 final wrapper 的 JSON/原始日志、CLI smoke 脚本和九项 stdout/stderr、static 与 environment 记录。独立重算清单原字节 SHA-256 为 **`ef33fcd714968f0236f42a065783c9967f23f7b0ac22deee6f120b376efef7f8`**，666 个输入文件逐项 hash 匹配；四次 wrapper 均记录同一 manifest 且 `source_drift=[]`，本分工再次检查当前输入无漂移。

| 实际执行 | 独立核对结果 | 证据边界 |
| --- | --- | --- |
| 默认 Embedded all-targets | exit 0，84.00 秒；lib247 + integration75 + example5 = **327/0/0**，十四个目标结果行完整 | 库15、组合4、example5 均在原日志逐项通过；没有把 main 的0项或旧轮次计数另加 |
| no-default lib + example | exit 0，35.63 秒；247+5 = **252/0/0** | 是同一批测试在另一 feature 配置下的重复验证，不加为252个新的独立用例 |
| no-default example build | exit 0，1.43 秒 | 真实构建，无测试数量；随后 CLI 使用该二进制 |
| 实际 CLI smoke | exit 0，0.70 秒；九项操作退出码全部符合预期 | help、旧隐式参数拒绝、额外参数拒绝、缺失输入拒绝、generate、replay、拒绝覆盖、历史文件重放、矛盾closed拒绝 |

独立核对 smoke 二进制 SHA-256 与记录一致，为 `75c015c68566eb1817237dd90883883703d7348e3e3c999d86ce311ee62b5fea`。生成 fixture 为 `374d84a56124ab88aecf40aaaa8225c2df6f813b3c860e9061efa953a38d1b49`，与历史 field-prep 合成文件逐字节相同；generate/replay/historical-replay 三份摘要一致，实际 7 records /16 samples /9 resets。旧 capture 4记录且没有 matching snapshot/close marker，playback 2记录而最新 closed snapshot 保留2 lost blocks，最新 capture 1记录且 running；三个流均没有被摘要补造记录或物理完成事实。`source_authenticated=false`、`physical_stream_complete=false` 保持。

smoke 脚本检查了 help/参数失败不创建文件、replay 和历史文件内容不变、同路径 generate 拒绝且无新临时残留、矛盾文件被拒且输入不变。所有预期失败的 stdout 为空；矛盾closed实际报 `final snapshot cannot account for observed records`，覆盖冲突实际报 File exists。记录为 `all_assertions_passed=true`、`temporary_directory_removed=true`，本分工另外检查该确切临时目录已不存在。没有执行 smoke 脚本或再次运行二进制。

`static.json` 和日志显示变更 Rust 格式检查与 diff check exit0；workspace fmt exit1 只报告历史未修改的 Server `tests/server_runtime/delivery.rs` import 顺序，Root 的基线字节比较为 unchanged_from_HEAD=true。此问题保留记录，不作为新代码格式失败，也不顺带修改该文件。现场环境记录为 macOS26.6.2/arm64，Rust/Cargo1.95.0；没有把 Mac 软件结果冒充 Linux/Pi、ALSA实跑或硬件验证。

最终代码未增加专门 hardlink 回归；此前建议仅为非阻断验证增强，现有普通目标及symlink覆盖拒绝、persist_noclobber 实现与实际不覆盖证据均已核对，不残留生产修复待办。文档已准确写明 parent 由本地操作者控制、同步 I/O 无墙钟时限、错误只清理未发布临时文件，以及发布后读取失败不会回滚新文件。

## 最终结论与保留边界

本轮独立审查无阻断项。两项真实缺口都有实际复现与修复后证据，组合模拟补齐生产音频软件路径到证据重放的连接；默认政策、精确取消、单reader、资源归属和默认不自动录音保持。Root 负责后续证据归档、源码提交和提交 blob 与冻结输入对应，不由本文提前声称已经提交。

R03继续暂停。真实R1/扬声器/功放、AEC参考电气与路由、实际录放音许可/保留规则、独立设备时钟/延迟、物理停声和R05长稳仍未验证；本轮没有打开 native stream、访问Pi、调用模型、部署服务或新增依赖。测试与CLI均使用合成PCM、临时本地资源，导出来源是声明而非真实性认证。本分工全部操作仅为只读审查和本文编辑，未运行Cargo/Git/外部服务或硬件。
