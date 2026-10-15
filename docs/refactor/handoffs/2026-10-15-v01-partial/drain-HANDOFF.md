# V01 paired late-drain 审计 — 2026-10-15

分工 `/root/v01_flow`；基线 `af5b35b`。先只读当前实现和已有测试，随后按 Root 授权仅新增 `tests/duplex_playback/late_drain.rs` 回归；未运行 Cargo、PG、SSH、模型或硬件，未修改生产源码。本轮保留默认 shadow、当前政策、原 10 / 25 / 45 秒期限与默认并发；R03 暂停，不新增依赖。Root 统一主文件模块引用、实跑、输入身份与最终归档。

## 当前行为

`tests/duplex_playback.rs::Runtime::close` 先释放测试注册 / resume 屏障、revoke 原 session，然后调用原 `DeliveryService::shutdown`。后者从本次调用起有 10 秒报告预算，超时会保留 `self.task` 内原 JoinHandle，并返回 incomplete；`shutdown_pending()` 读取 supervisor 的 running 标志。

paired close 保存第一次结果及 pending 值，继续调用 Embedded session stop、channel stop 和 RPC close。若最初 pending 为 true，则轮询 running，直到 false 后再调用 shutdown 领取原 join，最后依然断言第一次 shutdown 失败。因而 late drain 不能把初次失败变成成功，数据库 cleanup 由外层 context helper 放在这整个 close 之后。

生产 `DeliveryService::supervise_output` 的原 scope retirement 有独立 2 秒报告预算；到期后记录 sticky failure 并继续 await 同一个 `retire_voice_output` future。精确 media cancellation 也保留尚未结束的 adapter / fence future。不能为了测试成功而扩大这些预算、重建 output 或伪造已结束状态。

## 已确认与尚未确认

- **确认的覆盖缺口**：已有 RPC 与外层 45 秒回归没有直接证明 paired `Runtime::close` 的“第一次 Delivery incomplete → pending → 实际结束 → 保留失败”分支。`delivery_service::cancellation_timeout_persists_unknown_but_retains_actual_adapter_owner` 与 `output_lifecycle::scope_fence_timeout_retains_original_future_until_real_join` 验证生产所有权，但前者使用较短外层等待，后者也没有进入 paired close 的原 10 秒 late-drain 分支。
- **需要通过实跑核对的顺序**：paired 当前在 late-drain 轮询前已执行 session / channel / RPC stop。静态代码确实如此，但这不能单独证明原在途 RPC 或 SQL 已被破坏；现有 Arc / client clone 与生产 stop 所有权会影响结果。先用真实 pending 验证，不先修改生产。
- **保留已知限制**：Delivery running 只在 `run(...).await` 正常返回后清 false；supervisor 自身 panic 可能留下 stale true。不能用轮询超时或 watchdog 判定 owner 已结束。当前可控正常 late-drain 回归不证明任意 supervisor panic 可恢复，也不扩大到 poisoned lock 注入。
- **没有历史根因证据**：上述 fixture 覆盖问题不能倒推为旧 Companion V3 25 秒超时的实际原因，历史 V01 仍 open。

## 优先复用的真实屏障

采用现有 `AudioSubmission::try_begin_output()` 返回的 `SubmissionWrite`。仓库 `packages/alia-embedded/src/grpc/playback_service/tests/retirement.rs` 已用该 guard 验证“原 native write 未结束时，原 scope retirement 不可返回 settled”。guard 持有真实 software output gate；`finish(1)` 记录一帧实际模拟提交并释放，意外 drop 则只 settle 零帧，不编造完成音频。

paired 场景可用一帧合成 PCM 取得原 submission，等待 End accepted，但不 drain 原 queue，然后取得此 guard。启动 close 后，session revoke 会关闭原 scope，但原 retirement RPC 仍须等待此 write 结束，Delivery 的 output supervisor 因此实际保持 pending。保持 guard 超过原 10 秒 shutdown 报告预算，再释放；应能覆盖实际 `pending → false`，无须新增假 running 标志、生产钩子或硬件动作。

没有选择现有其他屏障的原因：`release_registration` / `release_resume` 会在 close 开始自动释放；`hold_end` 被 native cancel 或 generation retirement 影响，可能只阻塞 Embedded worker，不能单凭它证明 Delivery 的原 scope fence 仍 pending。真实 PG 与 Tokio 时间必须同步流逝，本例不能用 `advance` 跳过真实数据库等待。

## 最小受控验证方案

1. 使用 `with_db_context` 保留 Runtime、原 write guard、dispatch JoinHandle；body 只完成已有输出创建与一帧 End accepted，不启用 Companion 或其他模块。
2. cleanup 内 pin **同一个** `AssertUnwindSafe(runtime.close()).catch_unwind()` future。借用该 future 观察约 11 秒：这是测试观察窗，生产原 shutdown 仍为 10 秒，外层成功判定仍为原 45 秒，没有延长任何既有期限。
3. 观察窗结束先保存结果而不立即 assert：close 尚未返回、`shutdown_pending()` 为 true、原数据库可 ping、测试目录仍被保留。避免失败断言在持有 native write 时丢弃唯一 close owner。
4. 对原 write 执行 `finish(1)`，await 同一 close future 与原 dispatch handle。然后统一断言：close 仍报告最初 `paired Delivery cleanup failed`，Delivery pending 已为 false，原任务 / 目录 / listener 按可观测契约清理，数据库仍由外层 helper 最后回收。
5. 若当前实现通过，仅补覆盖与明确证据；若捕获确定的依赖提前释放，再修改相应 fixture 顺序并保留 RED / GREEN。不得因为缺少根因就更改生产期限、取消政策或并发。

该负向测试应把“预期原 shutdown 失败，但后来清理全部结束”作为成功条件，不能吞掉任意其他 cleanup panic。整个 cleanup 保持原 context helper 的同一 45 秒成功截止；超过时仍失败并继续持有原 owner 排空。

## 已提交给 Root 的回归

新增 `paired_late_drain_joins_original_write_before_database_cleanup_and_keeps_failure`，使用外层 `LateDrain` context 保存 Runtime、原 producer、原 write 和 dispatch handle。cleanup pin 一次 close，11 秒观察只记录值；释放原一帧、等待同一 close 和 dispatch 后保存独立 `DrainEvidence`，最后恢复 close 原 panic。最外层在原失败 catch 之外检查 pending 观察、数据库关闭前后、目录保留与删除、listener task 实际结束、native quiescent 和 supervisor 停止，再要求原 `paired Delivery cleanup failed` 仍在。

没有改动生产 Delivery 或期限。测试文件 scoped rustfmt 与 diff whitespace 检查通过，源码已通知 Root 冻结；Root 已执行该定向回归，本分工只读复核通过。当前正常 late-drain 条件下没有暴露需修改生产的缺口。

## 定向 GREEN 独立核对

已复核 `run/validation-20261015-v01-partial/boundary-green.log` 与 JSON：命令退出 0，默认测试并发，**6 passed / 0 failed / 0 ignored / 21 filtered**，其中本分工的 `paired_late_drain_joins_original_write_before_database_cleanup_and_keeps_failure` 明确为 `ok`。六项 target 总用时 **11.53 秒**；包含编译等工作的命令用时 **21.88 秒**，不把 target 总用时写成本用例单独计时。

该通过结果具体证明：使用原 **11 秒真实观察窗**时 close 和 supervisor 均仍 pending，数据库、目录和 listener 仍保留；释放原 `SubmissionWrite` 后，同一个 close future 与原 dispatch 被排空，native write 已 quiescent、supervisor / listener 已结束、目录已移除；数据库在 owner 清理后仍可读，随后才被 fixture 关闭。最外层仍取得 **`paired Delivery cleanup failed`**，未把第一次 shutdown incomplete 改成成功。没有推进真实 PG 或 Tokio 时钟，也没有修改原 10 / 25 / 45 秒期限。

GREEN 清单 SHA-256 为 `25bfee8a75f0782b5668194ac9a34c740ff0429ec6239834f3d29e7ed9e043ef`，运行记录 `source_drift=[]`。当前 `late_drain.rs` 为 **6088 字节**、SHA-256 `512cd25fc0bfe4907cb90a6af80da931e64f66d4ae8e05d43667ef77b0e33a89`，与 GREEN 和后续 final 两份清单逐项一致。final 清单含 **674** 个输入，SHA-256 `3f15e093efd3863143d5bcff0be0540ac4fb5c306b01b0650b69935abfd24747`；本次仅核对负责文件，不冒充独立复算整个清单。

定向后资源记录 `after-boundary-resources.json` 为 **0 个测试 schema、0 个其他 client**。该定向成功只证明受控的正常 late drain，不能扩展为任意 supervisor panic、poisoned lock 或历史 V01 根因已解决。

## 最终全量证据

已只读复核 `final-workspace.log` / JSON 与 `validation-summary.json`：最终一次完整 workspace **1262 passed / 0 failed / 5 既有 ignored / 0 filtered**，退出 0，命令用时 **405.60 秒**；duplex target **27 / 0 / 0 / 0 filtered**，target 用时 **45.63 秒**。本分工 late-drain 用例在该完整日志中仍明确为 `ok`，不是只引用此前带过滤的定向结果。原始运行记录维持 674 个输入和上述 final 清单身份，`source_drift=[]`；定向 GREEN、RED 和子进程 reentry 不重复累计到最终总数。

最终 `final-resources.json` 在命令结束后约 **0.584 秒**记录 **0 schema / 0 other clients**。已读取 `cleanup.json`，其中先核对本轮 exact container ID 与 `alia.validation=v01-partial-20261015` label，再停止并 `rm -v`，随后查询证实该任务容器已不存在；没有据此声称停止共享 runtime 或修改其他容器。本分工没有执行这些命令，只核对 Root 已保存的证据。

全部结果只支持当前受控测试边界；未做任意数据库初始化取消、poisoned native / supervisor panic 穷举，也未验证 Linux / Pi / 模型 / 真实硬件。历史 V01 保持 open，最终 commit 身份由 Root 统一归档。

## Checklist 与交接

- [x] 核对 paired close、Delivery 原 shutdown、late fence owner 与现有回归。
- [x] 区分确认的 fixture 覆盖缺口、待实跑顺序问题和已知 supervisor panic 限制。
- [x] 找到现有 native write guard，提出无需新增屏障的 pending→退出方案。
- [x] 向 Root 交付原期限 / 并发不变、真实时间和异常兜底的测试设计。
- [x] Root 执行受控回归，本分工只读核对定向 GREEN、负责文件身份与定向后资源；本场景未发现生产缺口。

当前分工 **5/5**，最终 workspace 与资源证据已核对，提交收口由 Root 完成。本分工没有运行验证；真实 Linux / Pi / 音频 / 执行器与历史 V01 根因均未由此得到验证。
