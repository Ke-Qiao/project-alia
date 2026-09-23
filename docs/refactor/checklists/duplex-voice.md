# Server VoiceSession v2 检查表

2026-09-22，工作树 `run/worktrees/alia-duplex-integration`。本文件记录本 agent 的 voice/config 子包；完整暂停纵向集成由主代理汇总。共享树不单独 Git 提交，不覆盖其它 agent 的未提交修改。

- [x] 核对原入口、Core freshness、VoiceOutput public API 与原来源双 epoch；确认旧 voice v1 / WAIT 取消须成对替换。
- [ ] 成对 voice v2 能力协商，拒旧版本/无 software hold/非法上限。
- [ ] accepted 与 drained speech frame 都先更新原 VoiceOutput 的近端语音屏障；采集不等待控制 SQL/RPC。
- [ ] 新鲜 evidence 后区分 Wait、Acoustic Backchannel、严格完整 Continue、Stop 与新问题；Evidence 仅报告语义请求。
- [ ] Resume 前重验原凭证/来源；pin 原 Channel Arc；新响应 request_id / producer / cancel 所有权固定，session 不保留 producer。
- [x] `policies.duplex.hold_budget_ms` 默认 60000，解析验证 1..120000；实际 `--test config` 4/4 通过。
- [ ] 该配置受双方能力与原 owner 生命周期限制的实际 voice/Delivery 联合验证。
- [ ] 真实 PG 的正常 Delivery start/recovery fixture；暂停/撤权/新采集/替换/关闭/背压定向回归。
- [ ] 更新真实 Easy-Turn gRPC smoke，不使用伪 Context/Response ID；记录实际运行或未运行原因。
- [ ] 默认 Server 定向检查/测试、格式、日志与已知限制完成，源码冻结交主代理。

此前 Embedded Ready 原 Binding 修复见 `duplex-protocol.md`；不能将其通过数替代本包验证。PG 只使用任务临时数据库与独立随机 schema，运行窗口与 B agent 协调。

## 额度暂停检查点：2026-09-22

用户明确要求暂停。当前共享工作树源码是**部分实现，未完成验收**，本 agent 未 Git 提交。仅收尾已有编译后冻结，未启动 voice PostgreSQL 或真实 Easy-Turn 测试。原始日志在 `run/duplex-voice-validation/`；详细恢复顺序、dirty 文件归属、源码摘要和命令结果见 `run/duplex-voice-handoff-20260922/HANDOFF.md` 与 `RESULT.json`。

已保存实现：v2 能力/Ready、新鲜 evidence 分流、speech capture 更新原 VoiceOutput、固定请求/producer、原 Channel Arc、原来源再鉴权、配置和 bootstrap 接线。上述未勾选项已有草稿代码和回归用例，但测试尚未跑，不能视为完成。

| 已执行命令（均使用 `CARGO_TARGET_DIR=/Users/ke-qiao/Developer/project-alia/target`） | 结果 |
| --- | --- |
| `cargo check -p alia-server --lib --offline --locked` | exit 0，生产库当时编译通过，日志 `production-first.log` |
| `cargo test -p alia-server --test config --offline --locked` | exit 0，4/4，通过含新预算边界，日志 `config.log` |
| `cargo test -p alia-server --lib grpc::voice_session::tests --no-run --offline --locked` | exit 101，当时被其它 owned 区域 playback/tests 的 11 处 v3 接口错误阻塞；Root 后续已迁移，尚未由本 agent 复跑，日志 `actor-compile.log` |
| `cargo test -p alia-server --test voice_stream_live --no-run --offline --locked` | exit 101，当前 live fixture 调用了不存在的 `OutputBinding::validate`，日志 `live-compile.log`；按暂停要求保留失败现场 |

最先恢复：修 live fixture 第 234 行不存在的 `binding.validate()`（使用真实现有契约验证，不能放松身份校验），第 545 行 `fixture.count("responses")` 应核对真实表 `delivery_responses`；重编译 actor/live；协调 PG 窗口后跑 actor；最后以真实模型/真实 canonical PG source→Context→Response 运行 live。当前 live 设备输出是明确的 Core 软件状态机 fixture，不能宣称物理音频或完整 LLM/TTS 已验证。共享树完整 all-targets/workspace 由 Root 后续统一验证。
