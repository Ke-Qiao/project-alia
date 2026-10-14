# R02 Mac 离线模拟 — Audio evidence HANDOFF

日期：2026-10-14；Owner：`settings_auth_reproduce`；基线 `ea3f0e4`。本轮仅推进用户已允许的 Mac 合成 PCM 离线模拟，R03 暂停；不新增依赖，不打开真实采播设备或执行器。Root 统一运行 Cargo、保存验证证据和提交，本 agent 未运行 Cargo/Git/SSH/Docker 或设备调用。

## Checklist 与当前进度

- [x] A01 读取 field-prep 与 R02-A 交接，审阅现有 session/export/import/replay、native clock/queue/owner 及 13 项回归。
- [x] A02 汇报具体 closed footer 校验反例及与实际生产者计数顺序相符的最小修复方案。
- [x] A03 按 Root 授权只增加现有 API 可编译的 RED 测试，生产保持未改，停写交 Root 执行。
- [x] A04 Root 归档实际 RED 后，修复 closed 最终统计约束并补必要正例。
- [x] A05 Root 统一验证，独立读回实际结果及源码冻结信息。
- [x] A06 独立审查通过，分工交付停写；全包 checklist、恢复入口与提交交 Root。

当前 **6/6（本分工完成）**；真实 RED、默认完整回归与无默认 features 回归已独立读回，独立审查无阻断。源码、测试及本交接停写；全包 CLI smoke、checklist、恢复入口与提交以 Root 交接为准，不预写提交 hash。

## 已完成能力与本轮分工

现有 R02-A 已提供唯一 reader、调用者所有的同步 `EvidenceSession::start/pump/finish`、512 records / 524288 samples / 16 MiB 诊断预算、严格有界 JSON 导入导出和按流重放。观察取消、finish/drop 不关闭 native owner；丢失的样本不补零；reopen 后无旧 close marker/快照的 final 保持 unknown；活动快照各 atomics 不是同一时刻事务。已有 13 项测试覆盖这些边界，本轮不重写、不重复造 recorder、后台任务或消费入口。

Root 负责把原生成固定 fixture 的示例改为显式 `generate` / `replay`，由实际输入生成摘要；另一 implementation agent 负责实际 capture helper / postmix output / EvidenceSession 的合成链路。我的文件归属仅 `module/audio_evidence.rs`、`module/audio_evidence/tests.rs` 与本 HANDOFF；不修改 native 后端、example 或顶层文档。

## 具体缺口与真实不变量

既有 `validate` 对 closed 快照只验证 `lost_blocks <= attempted_blocks`、最终计数不早于每条记录携带的历史 loss、cursor/continuity 不早于已观察 timing。它未把已观察成功块/PCM 与最终 loss 总量同时校验。

现有 `fixture()` 使用真实 reference bus 生成每方向 Started + PCM + Stopped，并取得 closed 快照。把 capture 的 `lost_blocks` 改为 `attempted_blocks` 后，旧校验仍接受“所有发布尝试均丢失”与文件中的成功记录同时成立；把 `lost_frames` 改为 `cursor` 也能把已观察 PCM 帧同时算作丢失。该反例不是声学质量判断，而是同一 closed 流的证据自相矛盾。

生产顺序确认：capture 在完整 native batch 上只调用一次 `observe` 推进 cursor，再按完整帧切块各 publish 一次；playback 对每块调用一次 `observe` 后 publish；marker 为零帧。每次 publish 仅成功入队或失败计 loss 其中一种。原生 owner 先停止/析构，再发布最终 close 尝试和 Release Closed。因而对 closed 最终值，可安全要求：

- `observed_record_count <= attempted_blocks - lost_blocks`；减法应检查下溢。
- `observed_PCM_frames <= cursor - lost_frames`；汇总及减法应检查溢出/下溢。

文件可能只包含成功发布记录的子集；缺少前缀、未读取尾部或中途结束观察都允许严格小于。并发 marker 与 PCM timing 的交错不改变成功/丢失总量。此约束只适用于 closed，不能扩到 running/closing/preparing 的非原子快照，也不要求文件完整或补回未知帧。

原合成 example 的 closed playback 具有 2 个已观察块 + 2 个 lost = 4 attempted、2 个已观察帧 + 2 个 lost = cursor 4，满足上述关系；不需要为迎合修复篡改既有合法 fixture。

## 真实 RED

新测试：`module::audio_evidence::tests::closed_snapshot_cannot_count_observed_records_or_frames_as_lost`。

它先确认原 fixture 已 closed、零 loss、有 PCM，并通过原 JSON 导入；随后分别修改 block loss 和 frame loss，通过真正 `AudioEvidence::read_json` 检查两类输入。失败信息只列接受的类型 `["records", "frames"]`，不输出样本或完整文件。

本 agent 已独立只读核对 `run/validation-20261014-r02-mac/red-closed-footer.log/json`：Root 使用无默认 features 的 lib 定向命令，退出 101，16.85 秒；**0 passed / 1 failed / 0 ignored / 245 filtered**，实际失败为 `contradictory closed snapshots accepted: ["records", "frames"]`。这是编译通过后的业务反例，两种矛盾均被旧实现错误接受。冻结 666 个输入，manifest SHA-256 为 `1f7c36444fe03f3c8b6fc3a60747c3446b8cb5f50a9e227244e0be6470c17e4e`，`source_drift=[]`；原日志与 RED patch 保留。

## GREEN 候选实施

Root 确认真实 RED 后授权实现。`AudioEvidence::validate` 在原身份、格式、逐条 counter/timing 校验基础上，仅为 closed 快照 checked-add 汇总同一完整 stream 身份的已观察块数与 PCM 帧数；使用 checked-sub 求发布成功总量，再要求观察数不超过成功总量。没有把部分记录要求成等值，没有为 running/closing/preparing 快照添加跨字段一致性约束。

原 RED 断言保留，新增 `closed_snapshot_allows_partial_observation_but_rejects_loss_underflow`：缺少 marker 的部分观察和完全没有记录的观察仍有效；即使没有可逐条比较的 records，`u64::MAX` block/frame loss 引起的减法下溢也被拒绝。原 active 非原子快照回归追加 `lost_frames=u64::MAX`，仍须被允许；转换成 closed 后才拒绝。库测试总数由 13 增为 **15**，没有重复已有单 consumer、取消、预算、重开或尾丢测试。

两份 owned 源码已运行 `rustfmt --edition 2024 --config skip_children=true`，退出 0；未运行 Cargo。生产与测试已交 Root 冻结，后续仅修改本交接并只读验证日志。

## 最终验证读回

Root 冻结 666 个输入，final manifest SHA-256 为 `ef33fcd714968f0236f42a065783c9967f23f7b0ac22deee6f120b376efef7f8`。本 agent 只读核对 `run/validation-20261014-r02-mac/` 中下列日志与元数据，未另跑测试：

- `final-embedded.log/json`：默认 features `--all-targets --offline --locked`，退出 0，84.00 秒；lib 247 + integration 75 + example 5 = **327 passed / 0 failed / 0 ignored / 0 filtered**，`source_drift=[]`。本模块 15 项均通过，原 RED 两反例转 GREEN，原取消/claim/loss/reopen/marker/严格导入回归未跳过。
- `final-no-default.log/json`：`--no-default-features --lib --example audio_evidence_replay --offline --locked`，退出 0，35.63 秒；lib 247 + example 5 = **252 passed / 0 failed / 0 ignored / 0 filtered**，同 final manifest 且 `source_drift=[]`。本模块 15 项同样全部通过。
- `static.json`：本轮 changed source rustfmt 与 diff check 均退出 0。全 workspace format 退出 1，仅发现 `packages/alia-server/tests/server_runtime/delivery.rs` 未改基线的 import 顺序差异；Root 已确认该文件相对 HEAD 未变，本轮没有为此扩张修改。

独立 reviewer 完整读回生产/测试后确认：先检查同一身份/格式，closed 才累加并检查总量；checked 运算、保守不等式、partial/empty 观察、两类下溢和 active 独立快照均符合实际 producer 契约，没有阻断。Root 负责其他 agent 的 native 模拟整合、CLI 实际执行证据与最终提交；本分工完成不等于 R02 声学验收完成。

## 明确保留的限制

来源枚举是声明，不是文件认证。即使内容格式和最终计数一致，也不证明硬件来源、真实采播时钟配准、R1 AEC、声学抑制、模型表现或物理取消尾音。同步 Read/Write 的取消只能在有界调用之间检查，不能中断已进入的操作系统阻塞调用。R1 仍未接入，功放参考规格与现场部署值尚待补齐；本轮不查询或操作设备，R03 后续保持暂停。
