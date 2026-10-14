# Settings 等待与关闭 — SoVITS 音频额度 HANDOFF

日期：2026-10-15。分工：`settings_audio`。基线：`e89d34c`。Root 统一运行 Cargo、归档证据、更新总清单及提交；本分工没有启动模型、设备、数据库、SSH 或 Cargo。

## 检查范围与结论

已读取上轮 [flow-HANDOFF](../2026-10-14-settings-hardware/flow-HANDOFF.md)、[regression-HANDOFF](../2026-10-14-settings-hardware/regression-HANDOFF.md)，核对 `GptSovitsTts` 的初始化、Settings apply、权重请求、合成和流式合成，以及共享 `ResponseAudioBudget` 和既有 HTTP/额度测试。

`configure_remote_weights` 仅由 `GptSovitsTts::init` 与 `apply_settings` 调用，最多顺序设置 GPT 与 SoVITS 两个权重；旧 `set_remote_weights` 在 HTTP `.timeout(request_timeout_sec)` 之前获取错误体的 4096 字节额度。全局额度耗尽会让该等待超出既有请求期限；Settings 在模块 apply 期间持有 update gate，因此会传导到后续管理请求。

TTS 的配置写锁已经在远端请求前释放，合成请求也使用配置快照。本轮不需要增加新的 TTS 配置锁抽象、工作线程、全局超时或独立控制池。全局编码额度、每响应额度和先保留后复制的内存保护必须保持。

本地配置更新早于远端权重切换，远端失败原本记录 warning 后返回已应用键。取消/超时不能证明远端未切换权重，也不撤销本地新配置；该部分成功语义必须保留。TTS 没有自有后台 worker，权重请求是 caller 持有的 borrowed future；future 真正 drop 才释放本次预留/HTTP 资源。它不证明客户端 socket 一断，整个 Settings handler 就立即被 drop。

## 文件与回归

所有权限定于 `src/providers/tts/gpt_sovits.rs`、新增 `src/providers/tts/gpt_sovits/tests/settings_waits.rs` 及本文。未修改 Settings API、Registry、Runtime、Delivery budget 的公共接口、配置或依赖。

新增测试每项以精确测试名和显式 child 环境变量重启当前 libtest 可执行文件。子进程独占真实全局额度，避免干扰并行用例；只持 semaphore permits，不创建 256 MiB 音频。父进程最多等待 10 秒，失败路径 kill 后 wait，子进程使用 Tokio paused clock。额度仍被完整持有，因此测试不会请求示例 HTTP 地址。

- `weight_setup_deadline_includes_audio_capacity_admission`：先占满全局额度，poll 权重请求见证 Pending，推进 101 ms，要求 100 ms 的权重请求期限结束。释放测试额度后重新取得完整全局额度，防止过期等待者残留。
- `cancelling_weight_setup_keeps_local_settings_and_releases_queued_capacity`：在同样的额度屏障中执行真实 TTS apply；见证挂起时本地配置已更新，真正 drop apply future 后新配置仍保留。释放测试额度后能再次取得全部容量。

上述局部用例不含数据库、真实模型或硬件，也不代替 Root 对实际 Settings HTTP、共享 gate 和服务关闭的验证。

## RED 与修复

Root 在冻结基线执行 `cargo test -p alia-server --offline --locked --lib settings_waits -- --nocapture`，实际 **1 通过 / 1 失败 / 0 忽略**，exit 101；`source_drift=[]`。额度期限用例在明确 Pending 见证后失败于“weight setup deadline must include its audio capacity wait”，取消保留配置/重获容量用例通过。原始证据为 `run/validation-20261015-settings-waits/red-audio.json` 与同名 `.log`；源码 manifest SHA-256 为 `6c15c1053908e4cf9c3fca572798cd73aef50ad9f82e099df65fbdc9e4d78490`。

Root 完成全部冻结 RED 后才释放编辑。修复将既有每次权重请求 timeout 移到 `set_remote_weights` future 外层，范围包含额度准入、HTTP 响应和错误体；无新期限配置、工作任务、池或接口。保留 `request_timeout_sec.max(1)`，两次顺序请求各自使用原上限（默认各 60 秒）；这不是 Settings 整体或任意 extension 的总时限。timeout 时本次 future 被 drop，释放它拥有的预留和 HTTP 对象，不承诺撤销远端可能已发生的权重切换。`init/apply_settings` 的 warning 与保留本地配置行为保持。

本分工 scoped rustfmt 与 diff whitespace 检查通过。Root 冻结修复后源码，执行 `providers::` GREEN **51 通过 / 0 失败 / 0 忽略**，包括全部 GPT-SoVITS 原有 HTTP 回归与本轮两项 `settings_waits`；`source_drift=[]`。原始证据 `run/validation-20261015-settings-waits/green-providers.json` 与同名 `.log`，源码 manifest SHA-256 为 `2b4524993978bcc30c675b4ffc8bd2b2757e8b2250ebdddbcc912932d9fd105e`。两个隔离子进程各自打印的 `1 passed` 属于父测试重入，不额外累计测试数。Server 全量与交付证据由 Root 继续收口。

## Checklist 与当前进度

- [x] 核对调用者、配置/额度所有权和既有测试。
- [x] 明确部分成功与取消的界限。
- [x] 写入确定性额度期限与取消回归。
- [x] Root 运行并归档 RED。
- [x] 以既有每次权重请求期限覆盖额度获取、HTTP 与错误体；保持 warning 和本地配置语义。
- [x] Root 完成 GREEN 与相关回归后更新结果。

本分工 **6/6**，RED 已复现且修复后相关 provider 回归通过。合成正常数据流的背压仍按已有 Response/turn 所有权管理，本轮不把 Settings 的管理请求期限扩成所有音频流的统一期限。真实远端权重切换和远端取消不可逆副作用不在本轮实测范围。

## Root 最终收口补记

本分工完成后统一Server全目标868/0/5通过；随后仅`llm/mod.rs`的cfg格式换行，最终provider51/0/0及无默认all-targets检查通过。源码提交`4e99134`的669个输入匹配final清单；green到final的精确格式转换和统计边界见[Root HANDOFF](root-HANDOFF.md)与[证据索引](evidence-index.json)。本轮隔离数据库已删除，共享服务和运行时保留；没有新增硬件动作。
