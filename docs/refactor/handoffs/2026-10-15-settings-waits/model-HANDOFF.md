# Settings 慢模型配置锁与取消边界 — 2026-10-15

分工：`settings_model`。基线 `e89d34c`；本分工只负责 LLM 请求配置所有权与本地模拟回归。Root 统一执行 Cargo、保存 RED/GREEN 证据和提交。本分工不运行外部模型、数据库、SSH、硬件或独立后台服务，不改变政策和 Settings 部分成功语义。

## 已核对范围

- [x] 读取 [上一轮 Settings 等待审计](../2026-10-14-settings-hardware/flow-HANDOFF.md)。
- [x] 检索三个 LLM 的全部内部请求调用方、四个公有生成入口、apply 和 health。
- [x] 确认六条实际网络路径跨 await 保留配置读锁；已有健康探测使用快照，可沿用相同模式。
- [x] 编写有界 loopback 复现，生产修改等待 Root 记录 RED。
- [x] Root 记录 RED：三家 provider 均在设置更新 watchdog 超时失败。
- [x] 完成配置快照修改并核对所有调用方；scoped rustfmt 和 `git diff --check` 通过。
- [x] Root 记录 GREEN：`providers::` 51 passed / 0 failed / 0 ignored，包含三家慢模型回归。

## 根因及修改面

`providers/llm/openai.rs` 的 `chat_internal`/`responses_internal`、Anthropic 和 Gemini 的 `chat_internal`/`stream_internal_with_tools` 共六条路径，持配置 `RwLockReadGuard` 跨 `.send().await` 和错误 body；后两家还跨成功 JSON body。Settings apply 等同实例写锁，并使持有的全局 Settings update gate 延迟释放。该因果链无需异常 extension 或真实慢外部模型即可复现。

OpenAI 原公有入口先读取 `api_family`，内部请求再读配置；两次读之间可能发生一次完整设置更新。现统一为每次公有请求获取一次完整快照，并传给已有两个请求方法，避免旧协议与新端点/凭据拼合；Responses 的工具流式降级仍走原非流式 API，但保留同一快照，不再递归进入公有方法重读配置。Anthropic/Gemini 在已有请求入口 clone 即足够。初始化、能力查询、健康检查和 apply 本身的短本地锁保持原状。

完整修改面为三个 provider 文件、公有入口对应测试、测试专用 `providers/llm/test_support.rs` 与 `mod.rs` 的 cfg(test) 声明。无 API、依赖、配置项或 schema 迁移。

## 回归设计和证据边界

每家一个 `stalled_generation_does_not_block_settings_and_cancels_connection` 测试；OpenAI 覆盖 Chat Completions 和 Responses 两家族。每家族仅四种情形：普通 chat 成功 body 停顿、tool chat 错误 body 停顿、stream 响应头停顿、stream tool 成功 body 停顿。覆盖四个公有入口及六条底层路径，不做全部入口×全部网络阶段的重复组合。

helper 明确禁用代理的本地 HTTP client；先见证完整请求头和 body 到达（分别 8192/16384 字节上限），再保留不完整响应。并发 poll 原生成 future 和 apply，以 1 秒 watchdog 验证设置写锁不被远端等待占用。上一请求已发送的 model/key 保持原值，下一请求使用刚更新值。最后真正 drop 所拥有生成 future，2 秒内确认本地 peer EOF/reset；不以客户端任务结束替代服务端取消，也不 spawn 无 owner 的后台任务。

这些是本地稳定调度下的测试 watchdog，不是新增生产 HTTP deadline。请求仍由现有调用方拥有；VoiceTurn `cancellable_stage` 会在 token 取消时 drop borrowed future；普通非 VoiceTurn 的聊天仍没有本轮新增总时限。单个 provider `stop()` 的默认实现不是在途请求的全局取消器。Settings 的 SQL/desired/live/file 部分成功与运行时关闭由 Root/其他分工验证，不能用本域测试替代。

Root 已执行本域 RED：3 tests / 0 passed / 3 failed，三家均在 `test_support.rs` 的“stalled generation must not retain the settings read lock”断言失败；RED 输入核对 `source_drift=[]`。原始证据在 `run/validation-20261015-settings-waits/red-model.*`，最终归档路径由 Root 收口。

Root 完成所有基线运行并解除冻结后，本分工才完成上述生产修改。随后 Root 执行 `providers::` GREEN：**51 passed / 0 failed / 0 ignored**，包含三家慢模型回归。GREEN 输入 SHA-256 为 `2b4524993978bcc30c675b4ffc8bd2b2757e8b2250ebdddbcc912932d9fd105e`，`source_drift=[]`；原始证据在 `run/validation-20261015-settings-waits/green-providers.*`。本分工 checklist **7/7** 完成并停止生产/测试编辑；Server 全量回归及最终提交由 Root 统一收口，不在本域重复运行。

未执行真实模型或硬件；局部 EOF/reset 证据不宣称外部供应商的任务已停止。网络总 deadline、客户端断线与服务器 handler 的关系、共享音频额度和整个 Runtime 的关闭证据均不由本域单独外推。

## Root 最终收口补记

本分工完成后统一Server全目标868/0/5通过；随后仅`llm/mod.rs`的cfg格式换行，最终provider51/0/0及无默认all-targets检查通过。源码提交`4e99134`的669个输入匹配final清单；green到final的精确格式转换和统计边界见[Root HANDOFF](root-HANDOFF.md)与[证据索引](evidence-index.json)。本轮隔离数据库已删除，共享服务和运行时保留；没有新增硬件动作。
