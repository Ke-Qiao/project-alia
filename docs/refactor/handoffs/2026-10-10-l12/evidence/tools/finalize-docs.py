from pathlib import Path
import json
r=Path(__file__).resolve().parent
root=r.parents[1]
d=root/'docs/refactor'
h=d/'handoffs/2026-10-10-l12'
v=json.loads((r/'commit-verification.json').read_text())
c=json.loads((r/'resource-closure.json').read_text())
assert c['all_task_containers_stopped'] and not c['remaining_test_schemas'] and c['other_database_connections']==0
commit=v['source_commit']; manifest=v['manifest_sha256']
def counts(name,linux=False):
 p=(r/'linux-arm64-v4/evidence'/f'{name}.counts.json') if linux else (r/f'{name}.counts.json')
 x=json.loads(p.read_text());assert x['assessment']=='PASSED_RECORDED_RUN',x['assessment']
 t=x['totals'];return f"{t['passed']}/{t['failed']}/{t['ignored']}"
mac=counts('workspace-v4');linux=counts('server-tests',True)
pty=counts('pty-v4');availability=counts('availability-no-default-v4');admission=counts('admission-no-default-v4')
for p in [r/'no-default-v4-result.json',r/'static-v4-result.json',r/'linux-arm64-v4/evidence/server-no-default.result.json',r/'linux-arm64-v4/evidence/server-release.result.json',r/'linux-arm64-v4/evidence/server-binary.result.json']:
 x=json.loads(p.read_text());assert x['exit_code']==0 and not x['source_drift']
identity=f"源码 `{commit}`，V4 manifest `{manifest}`，{v['matching_git_blobs']}/{v['input_count']} Git blob 一致。"
results=f"macOS workspace **{mac}**、真实 PTY **{pty}**、无默认 availability **{availability}**、提前准入 **{admission}**；Linux ARM64 Server 默认并行完整测试 **{linux}**。两端无默认全目标检查及 Linux release/native link 均通过。计数依次为 passed/failed/ignored，子进程摘要不重复计入。"
measure=json.loads((r/'playback-measurement.json').read_text())
phase=measure['phases']['v4_full_default_parallel']
phase_line=f"最终 V4 默认并行 child {phase['child_ms']/1000:.3f} 秒，其中 build {phase['build_ms']/1000:.3f} 秒、native release {phase['native_release_ms']}ms、schema DROP {phase['schema_drop_ms']}ms。"
limits='未运行 Linux 全 workspace 或部署 smoke，未验证真实模型、Pi/声学/机械及其他平台。L11 历史超时没有阶段日志，不能追认其全部原因；V3 setup 失败当时连接到谁仍未证实。'
nextstep='下一项软件任务：核对 `packages/alia-server/src/api/skills.rs` 中创建/更新/删除后的三个 reload TODO 与 `modules/interaction/skills` 的实际索引更新路径；先检查既有实现，再补缺口和真实整合回归。保持工具可见性、批准、加载/卸载/热更新和当前政策；本轮未实施该任务。'
root_text=f'''# L12 Root HANDOFF：完成

分支 `codex/architecture-refactor`，基线 `d5481b5cf4a6eeb28f535f08a8b1b9be7b31d5c9`。L12 **10/10**，L09—L12 **4/4**，R01—R06 仍 **2/6**；这些是条目计数，不代表整个项目完成百分比。三个 subagent 已交接，Root 完成集成、验证和正常提交。

## 完成的职责

Delivery 拥有 `DeliveryTransport`、`DeliveryTarget`、音频/文本 payload、observer/report、精确取消及输出控制端口；Channel 保留入口、能力、生命周期与 generation/session fence，具体 CLI/Embedded 直接实现出站端口。原 Arc upcast、原 producer 与完整 Channel 捆绑校验保持真实资源实例；同名替换回归扩展了既有测试。

跨 TTS/Channel/HTTP 的字节预算归 `delivery::audio`，provider stream alias 与 GPT-SoVITS 私有累计上限留在 provider；原限额、等待取消和 RAII 最后引用释放算法不变。旧 `ChannelTarget`、Channel 出站定义及 TTS budget 文件/出口删除，架构守卫防回流，无兼容包装。

三个分工分别是 outbound、budget、playback timeout；Root 独占 Cargo/数据库/容器/提交窗口。最终静态复核无阻断。默认 Shadow、当前政策、双 epoch、Context、事务原子性、两类暂停、精确输出身份、单次 dispatch、Unknown 和 native owned cleanup 均保持。不扩依赖/feature/平台/模型/硬件范围。

## 最终源码与结果

{identity}

{results}

全部最终检查基于相同 V4 输入，阶段前后均无漂移。默认完整测试未设置 RUST_TEST_THREADS；macOS 的 3 个 PTY 用例单独在真实终端运行；另 2 个 ignored 用例需要真实 Easy-Turn 服务，本轮未运行。Linux 未补跑 PTY。两端 CARGO_BUILD_JOBS=2，native Sherpa 使用已有归档，未设置 DOCS_RS。真实 PostgreSQL、loopback HTTP/gRPC、锁与关闭实际执行，模型/外设为原替身。

证据入口：[索引](evidence-index.json)、[macOS 全量](evidence/root/workspace-v4.counts.txt)、[Linux 全量](evidence/linux-arm64-v4/server-tests.counts.txt)、[提交身份](evidence/root/commit-verification.json)、[资源回收](evidence/root/resource-closure.json)。

## 保留的失败与修复链

V1 Linux 因原 registry 只读无法解包本地已有 alsa archive，未运行测试；随后复制现有 registry 到任务目录，全部离线继续。V1 真实 child 16.551 秒，build 16.249 秒。精确实际 sha2 依赖对 462699808 字节保留二进制的相同 64KiB 全文件算法耗时 15.416 秒；digest 与 Python/sha256sum 一致。build 窗口 15 个 PG 样本无活动/等待，不能排除采样间短等待。

仅将 dev profile 的已有 sha2 设为 opt-level=3。同字节优化后 0.834 秒、长度/摘要不变，V2 child 1.280 秒；不改变依赖、算法、输入、OnceCell、release 或原 20 秒期限。{phase_line} 原 V1/V2 测量及误写旧 binary 路径的命令失败均保留。

macOS V3 全量 **1059/1/5**：setup 取消/无文件断言已通过，最后对旧端口新建连接的断言失败。相同 binary 单测 0.15 秒通过不算修复。端口重用是候选原因，原日志无 peer 身份。V4 仅 cfg(test) 对实际 listener owner 包装，先 drop 原 listener 再发送见证；两个 HTTP 测试等待 serving 返回后立即验证见证，并在原 1 秒上限要求原 accepted socket EOF 或明确 ConnectionReset，其他错误失败。原正常响应、100 Continue、None/无文件、关闭期限均保持；非测试行为不变。V4 targeted **4/0/1** 及完整双平台验收通过。

{limits}

## 收尾与下一步

任务 PG 为 host 32770 的 `alia_l12_20261010`；未访问用户 5433。停机前其他连接、schema 均为零，全部任务容器已停止；ignored 缓存/原始 binary 留作诊断，没有活动 Cargo 或服务。未推送、未创建 PR，本轮未使用 reset 卡。

{nextstep}

原 V01、R02 AEC、R03 执行器反馈、Pi/24h/R05、其他平台/R06、前端与未来端上部署仍开放，详见 [延期台账](../../deferred.md)。恢复先读 [当前检查点](../../resume-checkpoint-l12-2026-10-10.md) 与四份 HANDOFF，再检查实际 Git。
'''
(h/'root-HANDOFF.md').write_text(root_text)
(h/'README.md').write_text(f'''# L12 四份 HANDOFF

本轮 **10/10**，L09—L12 **4/4**，六轮仍 **2/6**。{identity}

1. [Root：集成、验收、失败轨迹、资源与续接](root-HANDOFF.md)
2. [出站契约：Delivery / Channel 边界](outbound-HANDOFF.md)
3. [音频预算：所有者与 RAII](budget-HANDOFF.md)
4. [播放清理：阶段测量、瓶颈与 setup 资源见证](playback-HANDOFF.md)

最终运行以 Root HANDOFF 和 [证据索引](evidence-index.json) 为准。其他三份保留各自实施时记录，并由 Root 追加验收闭环。恢复从 [L12 检查点](../../resume-checkpoint-l12-2026-10-10.md) 开始。
''')
for name,old,new,box in [
 ('outbound','**8/9**','**9/9**','Root 在冻结输入上完成编译、相关集成/并发/取消/关闭测试和同源证据归档。'),
 ('budget','**6/7**','**7/7**','Root 在冻结源码上运行完整回归与 feature 检查，归档最终证据并更新本项。')]:
 p=h/f'{name}-HANDOFF.md';s=p.read_text().replace(old,new).replace('- [ ] '+box,'- [x] '+box)
 s=s.replace('本分工实施和静态审查完成 **9/9**；运行验收一项交给 Root，不能把未执行检查计为通过。','本分工实施、静态审查与 Root 最终运行验收完成 **9/9**。').replace('本分工实现与静态交接 **7/7**；最后一项同源编译与运行验证由 Root 接手。','本分工实现、静态交接与 Root 同源运行验收完成 **7/7**。')
 s+='\n## Root 最终验收补记\n\n'+identity+'\n\n'+results+' 相关取消/音频预算/原实例回归已在全量中通过；详见 [Root HANDOFF](root-HANDOFF.md)。先前“未执行”仅指本 subagent 的实施阶段，现已由 Root 完成上述验证。\n'
 p.write_text(s)
p=h/'playback-HANDOFF.md';p.write_text(p.read_text().replace('- [ ] Root 默认并发完整 Server 验证（最终统一验收由 Root 更新）。','- [x] Root V4 默认并发完整 Server 验证通过，最终同源证据已归档。')+'\n## Root V4 最终验收补记\n\n'+identity+'\n\n'+results+'\n\n'+phase_line+' setup 定向 4/0/1，完整双平台均通过；原 V3 失败及原二进制定向复查保留。详见 [Root HANDOFF](root-HANDOFF.md)。\n')
p=d/'checklists/l12-2026-10-10.md';s=p.read_text().replace('实施中；L09—L12 暂为 3/4','本轮 **10/10**；L09—L12 **4/4**').replace('- [ ]','- [x]');s+='\n'+identity+'\n\n'+results+' 最终证据、历史失败边界和资源停止见 [Root HANDOFF](../handoffs/2026-10-10-l12/root-HANDOFF.md)。\n';p.write_text(s)
p=d/'layering-l12-2026-10-10.md';s=p.read_text().replace('最终验收结果在完成运行后记录于下文。','本轮 10/10，L09—L12 4/4，六轮仍 2/6。');s=s[:s.index('## 验证与续接')]+f'''## 最终验证与证据

{identity}

{results}

{phase_line} macOS V3 setup 原端口连接断言失败保留，V4 改为 cfg(test) 原 listener 释放见证和原 socket 的有界 EOF/reset 检查。未改变生产 setup 或关闭期限，也未将未证实的端口重用认定为根因；详细轨迹见 [Root HANDOFF](handoffs/2026-10-10-l12/root-HANDOFF.md)。

所有最终验证同源、无漂移；冻结输入、命令、日志、统计、原失败、测量脚本和资源停止均在 [证据索引](handoffs/2026-10-10-l12/evidence-index.json)。任务数据库/容器全部停止，未访问用户 5433。

{nextstep}

参见 [10/10 checklist](checklists/l12-2026-10-10.md)、[四份 HANDOFF](handoffs/2026-10-10-l12/README.md) 和 [恢复检查点](resume-checkpoint-l12-2026-10-10.md)。{limits}
''';p.write_text(s)
(d/'resume-checkpoint-l12-2026-10-10.md').write_text(f'''# 当前检查点：2026-10-10 L12 完成

分支 `codex/architecture-refactor`；本轮 **10/10**，L09—L12 **4/4**，R01—R06 仍 **2/6**，不是整个项目完成百分比。三个 subagent 已交接，任务容器/数据库全部停止，无待恢复 Cargo/PG 作业。

依次读取 [技术说明](layering-l12-2026-10-10.md)、[checklist](checklists/l12-2026-10-10.md)、[四份 HANDOFF](handoffs/2026-10-10-l12/README.md) 和 [证据索引](handoffs/2026-10-10-l12/evidence-index.json)，核对 Git 状态后继续。

Delivery 现在拥有出站 transport、payload/observer/report 和共享音频预算；Channel 保留入口职责，原 adapter 与 producer 绑定不被 registry 替换。旧 Channel/TTS 契约入口删除，无兼容层。默认 Shadow、当前政策、两类暂停、原子性、精确取消、限额、native owned cleanup 保持。

{identity}

{results} 所有最终结果同源，无源码漂移。

Linux 当前主要启动瓶颈已测量：实际 sha2 对同一 462699808 字节文件 15.416→0.834 秒，摘要完全一致；只调整已有 sha2 的 dev 编译优化。{phase_line} 保留 20 秒期限和默认并发。L11 历史失败没有阶段数据，不能宣称所有历史原因被证明；原 V01 仍独立开放。

V3 macOS setup 旧端口新连接断言失败，最终 V4 改为原 listener drop 见证和原连接 EOF/reset，有界关闭、取消拒写和原响应要求全部保留。失败原时刻的 peer 未知，端口重用仅候选；复查通过与最终修复证据分开归档。

**{nextstep}**

继续三个互斥 subagent，Root 统一执行验证和正常提交。后续仍需处理原 V01、R02 AEC、R03 物理反馈、Pi/24h/R05、其他平台/R06；前端/未来设备端部署等暂缓项见 [延期台账](deferred.md)。不扩依赖/feature/平台/模型/硬件动作，不访问用户 5433。

资源关闭前 schema/其他连接均为零；任务缓存保留于 ignored run。未推送/未建 PR，用户允许额度不足使用 reset 卡，本轮未使用。

{limits}
''')
p=d/'resume-checkpoint-l11-2026-10-10.md';s=p.read_text();s=s.replace('# 当前检查点：2026-10-10 L11 完成','# 历史检查点：2026-10-10 L11 完成\n\n最新已完成 L12，恢复请读取 [L12 检查点](resume-checkpoint-l12-2026-10-10.md)。以下保留 L11 当时记录。',1);p.write_text(s)
p=d/'checklists/layering-next.md';s=p.read_text().replace('当前 **3 / 4 完成**','当前 **4 / 4 完成**').replace('- [ ] **L12 出站契约**：核对 Delivery 对 Channel transport 和 TTS audio budget 的依赖；契约位置应反映实际所有者，继续保留精确取消、限额、实际投递与 native 关闭证据。','- [x] **L12 出站契约**：Delivery 拥有 transport 和共享 audio budget，旧 Channel/TTS 出口删除；保留原实例、精确取消、限额和 native 清理，见 [本轮 10/10](l12-2026-10-10.md)。').replace('L10/L11 已验收，下一步从 L12 开始。','L10/L11/L12 均已验收，下一项软件任务为 Skills HTTP CRUD reload 缺口。').replace('L12 仍须按真实契约和关闭证据单独完成。','L12 亦已按真实契约和关闭证据完成。');s=s[:s.index('L11 已完成，下一步核对')]+ 'L12 已完成；Linux 当前 hash 启动瓶颈已测量修复，原 20 秒期限/并发保持，历史证据边界继续记录。'+nextstep+' 恢复见 [L12 检查点](../resume-checkpoint-l12-2026-10-10.md)。\n';p.write_text(s)
p=d/'checklists/next-round.md';s=p.read_text();at=s.index('## R01');s=s[:at]+f'2026-10-10 L12 出站契约清理 **10/10**，源码 `{commit[:7]}`；L09—L12 **4/4**，六轮仍 **2/6**。最新恢复从 [L12 检查点](../resume-checkpoint-l12-2026-10-10.md) 开始；下一软件项为 Skills HTTP CRUD reload。Linux 当前 hash 瓶颈已有同字节测量与修复，历史失败边界保留，原 V01 不因此关闭。\n\n'+s[at:];p.write_text(s)
p=d/'deferred.md';s=p.read_text().replace('- [ ] L10 初始化依赖、L11 跨域事务、L12 出站契约仍开放，按 [分层清单](checklists/layering-next.md) 推进。','- [x] L10 初始化依赖、L11 跨域事务、L12 出站契约后续均已完成，见 [分层清单 4/4](checklists/layering-next.md)。').replace('- [ ] L11 跨域事务所有者：已核实三条恢复入口，未实施。保持来源/epoch/Context/输出原子性和锁顺序。','- [x] L11 跨域事务所有者：已在后续轮次完成，来源/epoch/Context/输出原子性和锁顺序保持。').replace('- [ ] L12 出站契约与 Skills HTTP CRUD 三处 reload TODO 继续开放，未因初始化重构提前完成。','- [x] L12 出站契约已在后续轮次完成；Skills HTTP CRUD 三处 reload TODO 独立保留为下述待办。').replace('- [ ] L12 出站契约：核对 Delivery 的 Channel transport / TTS audio budget 所有者，保持取消、限额和真实关闭证据。','- [x] L12 出站契约已完成：Delivery 拥有 transport / audio budget，取消、限额和真实关闭证据保持。').replace('- [ ] Linux 旧 panic-fence 测试子进程 20 秒超时阶段定位：','- [x] Linux 旧 panic-fence 测试子进程的阶段定位与当前瓶颈修复已在 L12 完成；下面是 L11 当时观察：');s+='\n## 2026-10-10：L12 验收后的软件续接\n\n- [x] L12 出站契约 10/10，L09—L12 4/4，六轮仍 2/6；见 [最新检查点](resume-checkpoint-l12-2026-10-10.md)。\n- [x] 当前 Linux 启动瓶颈由实际 hash 同字节测量确认；仅 sha2 dev 优化，20 秒期限/默认并发保留，最终默认全量通过。L11 历史失败无阶段日志，原因不能全部追认。\n- [ ] '+nextstep+'\n- [ ] 原 V01 仍独立开放，后续复现需保存阶段/候选/准入证据。R02/R03/R05/R06 与用户暂缓项保持原门槛。\n\nmacOS V3 setup 原端口新连接断言失败证据保留；V4 直接检查原 listener 与已接收连接释放，同源双平台通过。未证实原 peer 身份，不把端口重用写为确定根因。\n';p.write_text(s)
p=d/'platform-validation.md';s=p.read_text();first=s.index('\n');s=s[:first]+f'\n\n**2026-10-10 L12：10/10，L09—L12 4/4，六轮仍 2/6。** '+identity+' '+results+' 当前 Linux hash 瓶颈实测修复，20 秒期限及默认并发保持；历史 L11 超时与 V3 setup 失败的证据边界保留。资源全部关闭，见 [最新检查点](resume-checkpoint-l12-2026-10-10.md) 和 [最终验证](layering-l12-2026-10-10.md#最终验证与证据)。以下为历史记录。\n'+s[first:];p.write_text(s)
print(json.dumps({'source_commit':commit,'macos':mac,'linux':linux,'l12':'10/10','layering':'4/4','rounds':'2/6'}))
