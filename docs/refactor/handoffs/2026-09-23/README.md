# 2026-09-23 组件定向证据归档

本目录保存本轮 Voice、Output lifecycle、Playback 三份组件交接与机器结果。六个归档文件在组件收口时逐字节复制自下列 `run/` 目录；保留原失败、复跑、源码变动时间及资源清理边界，不改写为最终整合结果。最终同一冻结源码的 macOS workspace、Linux ARM 与补充 live / PTY 验证由 Root 另行记录。

| 组件 | 交接与结果 | 原始目录（相对工作树根） |
| --- | --- | --- |
| Voice | [交接](voice.md)、[结果](voice-result.json) | `run/duplex-voice-validation-20260923` |
| Output lifecycle | [交接](lifecycle.md)、[结果](lifecycle-result.json) | `run/output-lifecycle-handoff-20260923` |
| Root Runtime | [交接](runtime.md) | `run/validation-20260923` |
| Playback / RetireOutput | [交接](playback.md)、[结果](playback-result.json) | `run/duplex-playback-validation-20260923` |

归档中的 `本目录`、相对日志名、`RESULT.json`、源码 manifest 和外部证据路径仍按其原始目录解释。本目录只复制 HANDOFF 与 RESULT；原始日志、原始 manifest 与诊断记录继续保留在对应 `run/` 目录。各组件自己的源码摘要只适用于其记录的运行阶段，不能代替 Root 最终冻结 manifest。

## 定向结果及限制

- Voice：Actor 23、配置 4、真实 Easy-Turn + gRPC + PG live 1，共 28 个不同定向用例。保留首次 Actor 20 pass / 3 fail，以及首次 live 超过未改变的 8 秒模型预算；后续 live 通过不证明首次超时的根因。模型冷缓存或负载只是推断。
- Lifecycle：Pipeline 21、DeliveryService 12、OutputLifecycle 11 通过，但跨阶段 Core 源码摘要发生变化，随后还有错误记录上限与格式调整；这 44 项不是一次冻结源码验收。保留三个 Bind 取消真实红测、错误通知生成额外 Context 的红测及栈帧诊断。
- Playback：真实 PG + 两端 loopback 的六项全部通过，最后两个受影响用例各复跑通过；Core 27、Proto 4 与 Embedded 相关定向组也有实际日志。8 项 retirement 过滤组与此前 72 项有交集，两次配对复跑属于原六项，不能相加制造新测试数。保留原五项首跑 1 pass / 4 fail、replacement 初跑尚未到达目标竞争、首播放序号为 0 的生产诊断。

上述软件 backend、受控 native 写入与软件 quiescence 不能证明物理 Presented、扬声器即时静音或真机暂停保留。Easy-Turn 仅在 Server；Pi 5 麦克风 / 声卡、声学连续性、真实双讲与 AEC 边界保持未验收。

## 首次冻结后的独立跟进

Root 首次冻结包含 605 个构建输入，manifest SHA-256 为 `4e6eadd8d28aeda542e8de0e6dbfbc855cd60d0c83c8c7cdc910702cc6002166`。其 macOS workspace 运行出现两项 Embedded binding-admission fixture 失败和一项 `server_runtime` fixture 失败；Linux 同样出现两项 binding-admission 失败。首次结果保留，不能用此前组件绿色覆盖。

两项 binding-admission fixture 在撤销旧绑定后重用了同一 boot / session / epoch 1。新增退休记录拒绝重新激活该 exact scope，是不可复活契约的一部分。修正仅改 fixture：先断言旧 scope 被拒绝且没有 active scope，再以新 session 激活，强断言旧 Evidence 因原 binding 已失效而拒绝，并检查旧 guard / 断连清理不动新 owner。独立 no-default `binding_admission` 组实跑 8 pass / 0 fail / 0 ignored，测试 0.02 秒；文件 SHA-256 为 `eb5cc1b31092575050191d54881abdaccb0322bc9fbf3ca11cf05e07d64001ed`，运行前后相同，未使用 PG。

这次跟进的日志与结果独立保存为 `run/duplex-playback-validation-20260923/binding-admission-retired-scope.log` 和 `binding-admission-retired-scope-result.json`，不覆盖原组件 HANDOFF / RESULT 或首次冻结结果。该定向修复仍需纳入 Root 新冻结版本的完整验收；本归档不宣称最终全矩阵已通过。

## V2 最终补充证据

组件归档保留收口时的原件；随后原始 run 交接可能追加 V2 结果，不反向覆盖这些旧阶段记录。最新验收见[2026-09-23 检查点](../../resume-checkpoint-2026-09-23.md)、[V2 输入清单](source-manifest-v2.json)、[PTY/模型补测](pty-model-v2-result.json)及 Root 交接。V1 失败结果独立保存为 `workspace-v1-result.json` / `workspace-v1-counts.json`；最终 workspace 与 Linux 摘要另存，不以组件计数替代。

最终 V2 已完成软件验收：[汇总](validation-summary.json)、[macOS 逐目标计数](workspace-v2-counts.json)、[macOS 完整命令](workspace-v2-result.json)、[Linux 各阶段与清理](linux-v2-result.json)。具体合入状态以检查点为准。

## 额度暂停时的最新交接

`voice.md` / `lifecycle.md` 及对应 result 已同步包含 V2 补充结果的最终原件；原组件阶段副本另存为 `voice-component*`、`lifecycle-component*`，仍可追溯先前失败和定向限制。Playback 原件未再修改；Root Runtime 汇总本次暂停。用户在完整 V2 验证通过后要求暂停：当前 10/11，剩余 DU11 合入；三名 agent 与全部测试 exec 已结束。
