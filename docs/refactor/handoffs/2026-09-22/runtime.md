# Root 运行时交接 — 2026-09-22 用户额度暂停

状态：用户明确要求停止新增工作并保存。不得把当前整合源码当成完整验证通过；主重构分支生产代码仍为 e6ba75b。当前整合分支 codex/duplex-integration，保存前 HEAD 82167dd；最终 WIP commit 与源码清单见主分支 docs/refactor/resume-state-2026-09-22.json。

## 本轮来源与范围

从主分支1677adc建立 run/worktrees/alia-duplex-integration，依次组合旧A 2619a28、B 76f5b04、C 779f4f8、Root e94f66e，无覆盖旧worktree。已合入新A 7cc9a84（含7bea321），B 4846b4a/56e9acc，C 5333eff。旧B/Root的Core/Proto overlay仍留原树；不复制回新整合树。

主Checklist DU01、DU02完成，2/11；其余9项仍开着。3个原子包已有各自组件验证，Server纵向整合还没完成。这个计数仅为本次暂停工作包，不是整个重构完成率。

## Root 实际落地（新源码尚未完整验证）

- DeliveryService 真正声明/导出 output actor，独立64入口与JoinSet、启动恢复/关闭/显式取消接线；输出控制不排在媒体队列后面。
- VoiceInput新增 admission request_id 与原 Arc<OutputProducer>；VoiceTurn贯穿原owner、固定channel；Response/Context准备后先bind再TTS/Start；原Response预算跨句段与native tail保活。
- 音频enqueue前保留MediaOwner，caller/drop不丢排队owner。监督入口先查原幂等事实，再要求live owner；wait_open不持全局admission锁。
- 暂停保留原Future。短语义期限变化由watch重算，不重建RPC；Fresh SQL permit只一次Control，未知ACK只Observe；command/hold deadline与scope deadline分别收紧。
- B补丁将后加入Audio的硬期限持久化为更小scope期限；Resume清除hold后不丢失该期限。单个60s hold不把原120s output永久缩成60s。
- apply先持久化已收到receipt，再检查scope/command期限、当前Running快照与最新speech；旧结果不能复活取消owner。
- 退休时本地cancel先信号，SQL request_cancel/retire_control与原scope fence并行；2s fence超时保留原Future/owner，shutdown不伪报成功。run_output panic转受监督失败再清理。
- EmbeddedChannel缓存pending入场Arc；Fence移除原scope后，迟到RegisterOutput ACK不能回填；其他session的entry不会被清除。
- Push等真实Start已准备好才启动handshake时钟，避免首块前合法Hold触发5s RPC握手期限。
- Pipeline cancellable_stage前后gate；LLM流读前/后gate，sentence TTS发起前/结果后gate，保留有限在途结果。该新增组合现在触发栈溢出，必须先排查。
- 已绑定voice scope发生生成错误时，只取消原output，不在同scope偷偷构造另一个Response/TTS错误提示；未绑定时固定source notice仍可成为规范首次Response。新generation错误提示流程未实现。
- 旧纯transport playback测试已迁移到新binding/Start参数与3个新增RPC trait方法；fixture明确只验证transport，真实登记由C配对测试覆盖。

## 验证与失败（不得混算）

1. Server lib check：run/validation-20260922/integration/check-after-subpackages.log，exit0，17.33s，属于当时开发快照。
2. Server lib tests --no-run：同目录 lib-tests-compile.log，exit0，39.77s，发生于旧playback fixture迁移后。A后续live fixture等编辑未因此自动获得验证。
3. B真实PG：delivery_service 12 + output_lifecycle 6 = 18 unique通过；cleanup并行修复后的fence单项再绿，仍计入18。详见B HANDOFF及run/output-lifecycle-result.json。
4. **最新Root pipeline回归失败**：命令 `ALIA_TEST_DATABASE_URL=postgresql://alia_test:alia_test@127.0.0.1:32768/alia_refactor_delivery_final CARGO_TARGET_DIR=/Users/ke-qiao/Developer/project-alia/target cargo test -p alia-server --lib channels::pipeline::integration_tests:: -- --nocapture`。日志 run/validation-20260922/integration/pipeline-first.log。启动20项，仅3项明确ok，随后 `text_delivery::a_large_provider_chunk_splits_only_at_utf8_boundaries_under_one_delivery` 所在线程 stack overflow / SIGABRT，cargo exit101；无完整结果表，其余用例终态未知。没有运行更大RUST_MIN_STACK、没有隐藏失败或开始修复。
5. 3个新sentence用例包含原预算/失败case及Held保留在途TTS、禁止下一次TTS；没有独立完成运行，不能记通过。
6. 本轮完整workspace、无默认组合、Linux、PTY、真实模型均未做；旧主生产875 pass/5ignored仅历史证据。

## 恢复后顺序

- [ ] 先读当前主检查点及四份HANDOFF，核对WIP SHA和manifest，保留主分支已验收代码。
- [ ] 第一项：复现上面单个pipeline stack overflow，检查cancellable_stage新增async封装、Future尺寸/深层嵌套与构造栈。原因尚未定位。可以用测量/临时栈参数诊断，但不能把提高测试线程栈当作默认通过；必须恢复默认运行的合理生产Future所有权/大小。
- [ ] 重跑20项pipeline及新增gate 3项、旧9项playback transport；修正真实行为失败，禁止绕开原owner/Context身份。
- [ ] A完成voice actor真实PG、新协议/两类暂停及live模型规范fixture；C执行已编译的5个真实配对gRPC测试。
- [ ] C已确认一个待复现风险：设备替换session后旧output cleanup non-close Fence拿到新active_scope，会被严格新会话admission validator判为Err。需要仅针对原owner清理的真实退休证明，不能放宽新session admission，也不能把active改变当物理静音。
- [ ] 审阅DU03—DU09所有出口，补未知提交/迟到事实/新speech与Resume等遗漏；18项已有生命周期tests在最终冻结源码复验。
- [ ] 冻结源码manifest后跑完整workspace、两端no-default alltargets、适用Linux ARM、3真实PTY与2模型目标；failures/ignored单列，源前后hash无漂移。
- [ ] 全部出口完成再普通提交合入codex/architecture-refactor，更新进度/操作/部署文档。

## 资源与所有权

Root执行会话82531、50340、46545已正常完成；71344最终exit101已收集，无活动Root构建/测试。没有Root新建容器/后台服务。PG容器alia-refactor-test-db:32768保留，用户5433不碰；Root SIGABRT可能留下本次pipeline schemas，B正在与其上一轮baseline精确比较，只清理可证明归属的新增schema，最终名单见B结果。模型8091刚实际健康检查ready，CPU/float32，三revision匹配旧checkpoint，保留不关闭；缓存/模型/工作树/Cargo target/Sherpa均保留。

当前源码文件归属：A=Server voice_session/config/bootstrap/config tests/live fixture；B=tests/delivery_service.rs与output_lifecycle.rs；C=tests/duplex_playback.rs、Cargo.lock/server Cargo.toml、Embedded playback control/submission四个公开入口；Root=DeliveryService/output/gate、VoiceInput、pipeline、Server EmbeddedChannel/Push/旧transport fixture、AdmittedSource.channel getter。所有当前代码将由Root统一WIP提交，子agent不在共享树提交。
