# V01 有界复现与诊断收口 — Root HANDOFF

日期：2026-10-15（Asia/Shanghai）；分支 `codex/architecture-refactor`；基线 `c685e5a`；测试修复提交 `f81762395de0ad75efa9c1e7afe60f46b620cac1`。本调查包10/10完成，**V01历史根因仍open**，R01—R06仍2/6。默认shadow、当前政策、执行批准、原期限/并发保持；R03继续暂停，无新增依赖或真实硬件动作。

## 目标、已有工作与分工

本包先核对R04 V3首次三例25秒超时、V4/10月4日后续记录和当前实现，再在限定三批内尝试复现。本次续接时工作区已保留本包WIP及完成的测试日志；Root与三个subagent重新核对全部身份后沿用结果，没有另起第四批。初始检查表3/10未及时收口，不等于运行没有发生；最终以原日志、冻结输入与独立复核为准。

三个分工及其当前进度：flow7/7、fixture6/6、review6/6，均停止源码编辑。Root负责冻结/原始证据、进程和数据库回收、提交核对及恢复记录。现有源码复核、失败日志审查与测试fixture复审分工互不覆盖写入。

## 调查结论与最小修复

旧V3原始日志确有三例超时，但没有phase/ScanProgress/retirement证据；本轮10个明确候选中未找到原失败binary，现有V4及10月4日artifact身份不能冒充V3。历史15份证据hash匹配，当前与历史源码/链接/负载不同；不能把重新编译旧代码称为恢复原失败环境。详见[历史审计](review-history-audit.json)。

有界运行没有复现旧超时，所以没有依据修改生产Companion。确认并修复的是测试观测缺口，仅改 `packages/alia-server/tests/duplex_playback/companion.rs`：

- 原filter屏蔽`service`已有固定安全warn；现在只增该namespace，继续默认off，不开放请求/身份/SQL原始错误日志。
- 原`ScanProgress`只描述shadow，却容易被解释成live进展；改标为`shadow_scan`，同一次SQL加入source claims按mode分组及rejected marker数量。计数不等于全部拒绝次数，也不等于admission成功。
- 内层25秒timeout和catch_unwind panic都在cleanup前获取并打印原最多1秒脱敏摘要；成功路径不新增查询，原panic仍在cleanup后恢复。
- 一项真实隔离PG回归证明shadow计数为零时live claim可见、owner/source/appraisal/generator不泄露，以及SQL出错仅返回固定`unavailable`。

原业务25秒、fixture45秒、失败诊断1秒、CASES=3、默认libtest并发和原机会/commit期限完全保留；没有refresh机会、降低断言或新增ignore。`CARGO_BUILD_JOBS=2`是编译并行，不是测试线程数。原输出身份、一次TTS/Response/Admission与精确取消、heartbeat/Runtime/RPC owner退出顺序保持。

## 实际验证与计数

| 执行 | 源码/身份 | 实际结果 |
| --- | --- | --- |
| workspace基线构建 | 669输入baseline manifest `2258c3ec…`；offline/locked、默认features | --no-run成功；固定baseline binary `f1c4756d…`，159.44秒 |
| 原基线完整duplex两批 | 同一个保留binary、默认libtest、独立任务PG | 各9/0/0，14.40与13.28秒；18次执行仍为9个unique |
| 诊断版完整workspace一次 | final manifest `27d16d5b…`；`cargo test --workspace --offline --locked --no-fail-fast` | **1245通过/0失败/5既有ignored/0 filtered**，436.72秒；67 Running + 5 Doc-tests目标 |
| 其中duplex | 诊断版binary `c26bf833…`，不同于baseline | 10/0/0，13.77秒；含原9项和新增1项，已包含在1245内 |
| 续接静态复核 | 当前669输入、两份保留binary、原日志 | 无漂移；rustfmt限定改动文件与源码/新写文档diff check通过（原样证据空白例外见README）；没有重跑测试 |

五项既有ignored为真实PTY三项与真实Easy-Turn两项；没有新增忽略，也没有把它们算通过。它们本轮未单独运行。Linux应用/Pi、真实模型、声学/执行器、release部署及长期负载均未运行。现有原短期限COMMIT拒绝回归随workspace通过，但人为过期机制不能回推V01历史原因。

最终manifest SHA-256为 `27d16d5b28bc4b0dcc60a92b2522d72a330fb56e2da9a1ace2483835ad769093`。baseline到final的669输入只有上述测试文件变化；最终669个提交blob逐项匹配，无额外/缺失。见[提交核对](evidence/commit-verification.json)、[验证摘要](evidence/validation-summary.json)、[续接复核](evidence/recovery-verification.json)和[证据索引](evidence-index.json)。原件按字节归档，两个大binary继续保存在ignored run目录且有哈希收据。

## 资源关闭及其证据边界

测试使用任务独立PG容器 `alia-v01-20261015-db`，loopback临时端口，未访问用户5433。两批baseline结束后的真实SQL快照为schema0/其他client0。恢复收口时OrbStack已停止、socket缺失；Root启动现有runtime以清理任务容器，精确ID/label核对后发现该容器仍已停止，没有重启PG或补跑测试。

最终全量测试结束当刻的schema/连接快照未保留，所以[资源记录](evidence/final-resources.json)明确为null，不用重启后零连接替代。当前相关测试进程为空，任务容器与临时卷已移除并确认不存在，见[cleanup](evidence/cleanup.json)。共享astrbot随runtime启动而运行，未修改其配置或停止它，OrbStack保留运行；`other_containers_modified=false`仅描述cleanup阶段，不表示runtime启动前后运行状态没有变化。

## 恢复与未决项

从[恢复检查点](../../resume-checkpoint-v01-2026-10-15.md)和[checklist](../../checklists/v01-2026-10-15.md)接续。三批上限已用完，不为取得更多绿色重复相同场景。V01只在出现新的失败/原现场artifact时继续按phase、shadow/live计数、pending/admission reason、原预算及PG等待分类定位；捕获明确原因并验证修复前不关闭根因。

新增诊断没有覆盖初始化、外层45秒直接drop和cleanup自身中断；本轮也未动态强制触发整条paired失败清理分支，顺序正确性由源码复核与原成功清理证明分别支持。无需据此预建全局追踪框架或改变原期限。R02沿用已确认硬件AEC优先并等待资料，R03暂停，R05继续等待现场出口；其它部署/Settings边界见[台账](../../deferred.md)。
