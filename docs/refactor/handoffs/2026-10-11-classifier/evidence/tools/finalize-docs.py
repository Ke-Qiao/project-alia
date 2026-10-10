from pathlib import Path
import json
r=Path(__file__).resolve().parent
root=r.parents[1]
d=root/'docs/refactor'
h=d/'handoffs/2026-10-11-classifier'
v=json.loads((r/'commit-verification.json').read_text())
stages=json.loads((r/'final-stage-summary.json').read_text())['stages']
closure=json.loads((r/'resource-closure.json').read_text())
assert closure['all_task_containers_stopped'] and closure['other_database_connections']==0 and not closure['remaining_test_schemas']
source=v['source_commit']; manifest=v['manifest_sha256']
identity=f'源码 `{source}`，V2 manifest `{manifest}`，644/644 Git blob 一致。'
results='macOS workspace **1080/0/5**；Linux ARM64 Server 默认并行完整回归 **798/0/5**。两端无默认 feature 全目标检查、边界正反自测和变更文件格式检查全部通过。计数依次为 passed/failed/ignored，嵌套子进程摘要不重复累计；所有最终阶段同源、无源码漂移。'
resources='所有 Cargo 阶段结束；关闭前其他数据库连接与测试 schema 均为零，任务 Linux holder 与 PostgreSQL 已停止，用户 5433 未访问。空闲 holder 的 sleep 进程在 docker stop 后 exit 137，验证命令均 exit 0；未把容器停止码当作测试结果。缓存与只读快照保留在 ignored `run/validation-20261011-classifier/`。'
limits='未运行 Linux 全 workspace、release/native release link、真实 PTY、部署 smoke、Skills 网络 COMMIT ACK 丢失注入、真实模型或硬件验证。目录写锁取消注入覆盖 finish 阶段；exact/semantic 目录等待未分别重复同样锁注入，已审阅包装并运行既有缓存取消出口回归。多实例／外部 SQL 仍需显式 reload。'
next_task='下一软件任务：从已记录的 R04 整体 health 限额开始，审计 `api/health.rs`、ModuleRegistry 顺序聚合、Router 重复 minor probe 及 Provider 诊断；先检查现有实现，再收敛整体预算、同 profile 重复探测和并发边界。保持 API、默认 probe=false 和当前政策，不自行启用外部或付费探测。本轮未实施。'
p=d/'classifier-2026-10-11.md';s=p.read_text();s=s.replace('最终结果待统一验证填写。',identity+'\n\n'+results+'\n\n');s+='\n## 最终验证与证据\n\n| 平台／检查 | 命令 | 结果 | 耗时 |\n| --- | --- | --- | --- |\n'
rows=[('macOS 默认 workspace','cargo test --workspace --locked --offline --no-fail-fast','1080/0/5','macos/workspace-v2'),('macOS 无默认全目标','cargo check --workspace --all-targets --no-default-features --locked --offline','exit 0','macos/no-default-v2'),('Linux ARM64 默认 Server','cargo test -p alia-server --locked --offline --no-fail-fast','798/0/5','linux/server-tests'),('Linux ARM64 无默认全目标','cargo check -p alia-server --all-targets --no-default-features --locked --offline','exit 0','linux/server-no-default')]
for platform,command,result,key in rows:s+=f'| {platform} | `{command}` | {result} | {stages[key]["elapsed_seconds"]} 秒 |\n'
s+='\nmacOS 26.6.2 ARM64 与 Debian 12 ARM64 容器，Rust/Cargo 1.95.0；两端 CARGO_BUILD_JOBS=2，RUST_TEST_THREADS/RUST_MIN_STACK/DOCS_RS 未设置，沿用原生归档和离线缓存。5 项忽略为 3 项真实 PTY 与 2 项真实 Easy-Turn 音频验证，未静默启用或计为通过。V2 首次完整运行两端均通过，没有延长期限或降低并行度。\n\n'+resources+'\n\n'+next_task+'\n\n'+limits+'\n';p.write_text(s)
p=d/'checklists/classifier-2026-10-11.md';s=p.read_text().replace('本轮实施中；','本轮 **10/10 完成**；').replace('- [ ]','- [x]');s+='\n'+identity+'\n\n'+results+'\n\n'+resources+'\n';p.write_text(s)
p=h/'README.md';s=p.read_text();s=s.replace('Root 正在运行冻结 V2 的 macOS workspace 和 Linux ARM64 Server 完整默认并行测试。','本轮 **10/10 完成**，L09—L12 4/4，六轮仍 2/6，不代表项目完成百分比。\n\n'+identity+'\n\n'+results+'\n\n');p.write_text(s+'\n'+resources+'\n')
p=h/'root-HANDOFF.md';s=p.read_text().replace('最终运行结果、源码身份和资源关闭状态待统一验收填写，不把实现完成计为测试通过。','本轮 **10/10 完成**。\n\n'+identity+'\n\n'+results+'\n\n'+resources);s+='\n## 恢复步骤\n\n1. 读取 [当前检查点](../../resume-checkpoint-classifier-2026-10-11.md)、checklist 与其余三份 HANDOFF，核对 branch/status/log 和证据索引。\n2. 本轮没有待恢复作业或未完成实现。V1 为静态候选，V2 为最终首次同源验收；新变更另冻清单，不能覆盖原证据。\n3. 按上述 health 审计继续，先给出具体范围与步骤，保持三个互斥 subagent 和 Root 唯一 Cargo/PG 验证窗口。\n4. 原 V01、R02/R03/R05/R06 与 D/H 门槛不变；不访问用户数据库或自行发起硬件动作。\n\n未推送/未建 PR；本轮未使用 reset 卡。\n\n'+limits+'\n';p.write_text(s)
for name in ['registry-HANDOFF.md','router-HANDOFF.md','regression-HANDOFF.md']:
 p=h/name;s=p.read_text().replace('- [ ] Root','- [x] Root')
 s=s.replace('Root 在 V2 冻结后运行新增定向测试及完整验证','Root 在 V2 冻结后的完整验证中运行新增锁取消回归')
 if name=='regression-HANDOFF.md':s=s.replace('当前本分工进度 **6/8**。只做过静态阅读和格式化；上述是已经写入的断言，不是尚未运行测试的通过报告。','本分工最终 **8/8**。Subagent 只做静态阅读和格式化；Root 的实际运行结果见下方验收补充。')
 s+='\n## Root 最终验收补充\n\n'+identity+'\n\n'+results+'\n\n以上实施时的“待验证”描述保留时序语境，以本段为最终状态。新增回归随完整默认测试实际运行；没有单独把循环场景或嵌套子进程当作额外测试累计。四份 HANDOFF、checklist 和恢复点完成，资源关闭见 Root HANDOFF。\n';p.write_text(s)
checkpoint=f'''# 当前检查点：2026-10-11 Classifier 目录衔接完成

分支 `codex/architecture-refactor`；本轮 **10/10**，L09—L12 **4/4**，R01—R06 仍 **2/6**，不代表整个项目完成百分比。三个 subagent 已交接，没有待恢复的 Cargo/PG 作业。

依次读取 [技术说明](classifier-2026-10-11.md)、[checklist](checklists/classifier-2026-10-11.md)、[四份 HANDOFF](handoffs/2026-10-11-classifier/README.md) 和 [证据索引](handoffs/2026-10-11-classifier/evidence-index.json)，然后核对 Git branch/status/log。

{identity}

静态技能表和专名捷径已删除。每轮从运行 Registry 获取一次目录，候选只有五个描述字段；模型返回名称必须属于此次候选。解析前核对选中技能 ID 与完整内容，变更、禁用、删除和同名重建按当前模式回退；无关目录变更不阻止有效选择。无二次分类，major-only/minor-only/both 的政策保持。

缓存 exact/semantic direct/reference 绑定整目录指纹，继续要求原批准与安全条件；审核不能改写或补造原绑定。目录不再当前时不写新缓存。目录等待可取消，真实 COMMIT 占写锁下 finish 取消已实测及时退出且无 Response/assistant 持久化。

{results}

V1 仅静态审阅，类型导入、三处取消遗漏和 major-only 夹具替身接线在首次编译前修正。V2 两端第一次完整执行均通过。5 项 ignored 单列；不宣称真实 PTY/模型验证已通过。默认 Shadow、工具全局可见性与 Execution 批准保持，无新增依赖、feature、配置、迁移、平台或硬件动作。

**{next_task}**

目录消费的线性化点在 resolve/is_current 成功处；之后 CRUD 不撤销已经准入的文本，排队缓存仍携原指纹。外部 SQL/其他实例仍需显式 reload，未引入广播依赖。原 V01、R02 AEC、R03 反馈、Pi/24h/R05、其他平台/R06 和用户暂缓项见 [延期台账](deferred.md)。

{resources}

未推送/未建 PR，本轮未使用 reset 卡。{limits}
'''
(d/'resume-checkpoint-classifier-2026-10-11.md').write_text(checkpoint)
p=d/'resume-checkpoint-skills-2026-10-11.md';s=p.read_text();first,rest=s.split('\n',1);p.write_text(first+'\n\n后续更新：静态 classifier 与运行目录衔接已完成，最新恢复从 [Classifier 检查点](resume-checkpoint-classifier-2026-10-11.md) 开始；下文保留 Skills 轮当时的状态。\n'+rest)
p=d/'checklists/layering-next.md';s=p.read_text().replace('后续更新：Skills HTTP CRUD reload 清理已完成（本轮 10/10），下一项软件审计见 [2026-10-11 检查点](../resume-checkpoint-skills-2026-10-11.md)。','后续更新：Skills HTTP CRUD reload 与 classifier 运行目录衔接均已完成（各轮 10/10），下一项 health 软件审计见 [2026-10-11 最新检查点](../resume-checkpoint-classifier-2026-10-11.md)。');p.write_text(s)
p=d/'checklists/next-round.md';s=p.read_text().replace('## R01 ·',f'2026-10-11 Classifier 运行目录衔接 **10/10**，源码 `{source[:7]}`；六轮仍 **2/6**。动态候选、在途定义核对、缓存绑定与取消已同源双平台验收。最新恢复从 [Classifier 检查点](../resume-checkpoint-classifier-2026-10-11.md) 开始，下一软件项为 R04 整体 health 限额审计。\n\n## R01 ·',1);p.write_text(s)
p=d/'deferred.md';s=p.read_text();old='- [ ] 下一项软件任务：独立审计静态 classifier';s=s.replace(old,'- [x] 后续已在 [Classifier 轮](resume-checkpoint-classifier-2026-10-11.md) 完成，以下为此前任务定义：独立审计静态 classifier',1);s+='\n## 2026-10-11：Classifier 运行目录衔接后的续接\n\n- [x] 本轮 10/10，动态候选、未知名回退、选中定义版本核对、整目录缓存绑定和可取消读锁均完成；'+identity+'\n- [ ] '+next_task+'\n- [ ] 多实例/外部 SQL 自动通知仍待明确需求，当前显式 reload；未新增广播依赖。\n- [ ] V01、R02/R03/R05/R06 与 D/H 原状态不变。已准入文本不会由后续技能修改自动撤销，Execution 仍独立检查权限与批准。\n\n'+results+'\n';p.write_text(s)
p=d/'platform-validation.md';s=p.read_text();first,rest=s.split('\n',1);p.write_text(first+'\n\n**2026-10-11 Classifier：10/10，L09—L12 4/4，六轮仍 2/6。** '+identity+' '+results+' macOS 26.6.2 ARM64 / Debian 12 ARM64，Rust 1.95.0；原期限与默认测试并行度保留。当前结果见 [技术说明](classifier-2026-10-11.md#最终验证与证据)，本轮未运行 release/部署/PTY/真实模型和硬件，以下为各旧源码的历史证据。\n'+rest)
print(json.dumps({'source_commit':source,'docs_finalized':True,'checklist':'10/10'}))
