# R04-01 HTTP / CLI 配置安装交接

2026-09-28，`companion_replay_28`。这是 R04-01 的 canonical handoff，承接只读 [readiness 第 1 项](../2026-09-28/r04-readiness.md)。本次与已提交的 R01 `a79cb1f` 分开；Root 负责统一验证和提交。旧 `2026-09-28/r04-oobe.md` 不再更新。

当前冻结版本为 **R04 V4，633 个输入**，manifest SHA-256 `35efd1807b5c2820b3024fc6142bf769ace5973cc13275e0b5649f8bec011a8b`，见 [final-source-manifest-v4.json](../../../../run/validation-20260928-r04/final-source-manifest-v4.json)。Root 已完成V4全量、专用环境及平台验证并提交 `8f5bb83`，见末尾验收回填与整包摘要。V4 只为旧 Companion 配对测试补固定阶段诊断，没有因 V3 超时改变业务行为或放宽截止时间。

## 已实施与组件验证

- [x] HTTP `generate-config` 从既有嵌入 base 与分片按实际配置加载顺序 deep-merge，返回完整单文件 `template`，不再依赖尚未生成的 `server.d`。
- [x] CLI 继续生成主文件 + `.d` 分片；拒绝覆盖现有主文件或分片树。新建安装失败清理本次创建的分片树，主文件在全部写入/验证后发布。
- [x] `ServerConfig::load_candidate` 与 `load` 共用合并后的解析、默认 provider、环境覆盖和路径解析；验证目标的实际 `.d`，不会静默忽略既有覆盖。
- [x] `validate_llm_roles` 检查四个必需角色与三个已配置可选角色的 profile、Chat/Embedding kind、provider 引用及已有 embedding provider 限制。未配置可选角色、字面 `noop`、未被角色引用的 profile 不新增限制。语义检查不初始化 Provider、不联网或连接数据库；不声称证明模型可用或编译 feature 已提供。
- [x] HTTP 保存通过语义验证后，使用同目录临时文件、`sync_all`、atomic rename 发布；CLI 首装用 hard_link 原子创建最终主文件，避免覆盖并发出现的目标。Unix 主文件权限为 0600。发布前出错保留原配置，不发 ConfigReady；临时文件清理。
- [x] HTTP 省略 path 时复用 CLI 的 `default_config_target`，尊重 `ALIA_CONFIG_PATH`。现有符号链接目标按真实 loader 的 canonical path 验证/发布，避免验证一套 `.d` 却加载另一套。
- [x] banner 改为 loopback HTTP API、curl / SSH 与 CLI 说明；删除仅为旧 banner 使用的 LAN 地址枚举。`network_interface` 仍为 `discovery.rs` 使用，未删依赖。
- [x] 仅四个指定源码文件运行 rustfmt；diff --check 无误。本 agent 未运行 Cargo / PostgreSQL / Provider，也未提交。
- [x] Root 的 [server-lib-v2.log](../../../../run/validation-20260928-r04/server-lib-v2.log) 实际通过下列全部 **8 / 8** 配置回归，以及新增的 loopback listener / setup 成功响应与慢 body 排空 **2 / 2**。
- [x] R04 完整冻结验收及正常源码提交已由 Root 收口；保留下文各版本的实际失败与证据边界，不以局部重跑替代全量验收。

验证状态沿革：

- **V1**：lib 编译失败，测试未执行。
- **V2**：lib **308 passed / 1 failed / 3 ignored**；唯一失败为 OpenAI connection-refusal fixture。本 setup 包的 8 项配置与 2 项新增生命周期测试均实际通过。
- **V3**：独立 Provider fixture 修复后，全量运行仍在既有 `duplex_playback` 的三个 Companion 配对用例处因 25 秒截止失败；该 binary 的当次结果为 **6 passed / 3 failed**，见 [workspace-v3.log](../../../../run/validation-20260928-r04/workspace-v3.log)。同一个 workspace binary 随后单独按默认线程并行运行 **9 / 9 通过**，13.65 秒，见 [paired-workspace-binary-v3.log](../../../../run/validation-20260928-r04/paired-workspace-binary-v3.log) 与 [运行身份](../../../../run/validation-20260928-r04/paired-workspace-binary-v3-result.json)。**根因未定**；不据此宣称负载是根因、测试已修复或 V3 全量通过。
- **V4**：冻结身份见上文；固定阶段诊断已加入，[workspace-v4.log](../../../../run/validation-20260928-r04/workspace-v4.log) 已由 Root 完成，1053/0/5、源码无漂移。本 agent 没有并行 Cargo / PostgreSQL 或源码修改。

Root 另行持有 `bootstrap/setup.rs` 的 loopback-only listener、移除 setup CORS/frontend/mDNS 以及关闭时 HTTP 响应排空。现在完整 `run_setup_on` 用例实际验证：保存成功响应能读完；另一连接停在未完成 JSON body 时，2 秒 grace 后实际 socket 被关闭，原 server join 返回；listener 随后不再接受连接。此证据来自新增 setup 测试，不是从下面的独立路由 fixture 推导。

## 实际 setup 操作限制

- OOBE 只在选定配置不存在时进入；已存在但无效的配置直接报错，不自动覆盖。交互式 `c` / Enter 需要 stdin 和 stdout 都为 TTY；否则使用 HTTP setup。
- setup 固定监听 `127.0.0.1:3000`，尚不采用待保存主文件中的 Server 端口。它没有账号认证，因此只提供 loopback 入口；远程操作使用 SSH 隧道。成功后 setup 关闭，正常 Server 才按其配置的地址/端口启动，setup API 不是常驻管理 API。
- 两个配置操作都是 POST。`generate-config` 返回 `{template: TOML字符串}`；`save-config` 接受 `{config: TOML字符串, path?: 主文件路径}`。目标路径属于 Server 机器的文件系统；省略时优先采用进程 `ALIA_CONFIG_PATH`。
- HTTP 模板是包含样例分片内容的单文件，HTTP 保存不另建 `.d`；CLI 保留主文件加 `.d`。已有同名 `.d` 对主文件仍有覆盖优先级，保存检查的是实际合并结果。
- 保存的正常 JSON 包含 `success`、`path`，失败时还有 `error`。配置验证或保存失败也可能返回 HTTP 200，操作脚本必须检查 `success`，不能只用 `curl --fail`。失败不发 ConfigReady，setup 继续等待修正请求。
- 配置语义检查不证明数据库/模型在线、编译 feature 已提供或硬件可用。setup 的 `/health` 返回 `status: "setup"`，不是正常 Server 的模块健康结果。

可执行生成→编辑 TOML→`jq --rawfile` 保存步骤见 [Server/Embedded 操作](../../../operations/server-embedded.md#配置与启动)；Rust-only 工具链与首次配置复制见 [本地开发](../../../operations/local-development.md)。操作文档与本节不把软件配对通过当作 Pi 声学验收。

## 8 项可运行回归

测试在 `packages/alia-server/src/oobe/routes/tests.rs`，每项使用独立临时目录；HTTP 用实际 Axum loopback listener 和 reqwest，不依赖 PostgreSQL。fixture 有有限客户端 deadline 和停止排空，失败 Drop 会关闭自己的 server/目录。

- [x] `generated_http_template_loads_all_example_roles_without_external_fragments`：真实生成→保存→ConfigReady→ServerConfig::load；全部样例 profile/provider 可解析，major 保持 local_openai / qwen2.5-7b-instruct。
- [x] `missing_role_profiles_leave_original_config_and_setup_state_unchanged`：7 个角色分别引用缺失 profile，旧文件逐字不变、无 Ready/临时残留。
- [x] `wrong_role_kind_or_provider_is_rejected_before_publication`：Chat/Embedding 错配、缺 provider、不支持的 embedding provider 不发布。
- [x] `target_fragments_are_validated_and_preserved_with_the_proposed_base`：目标分片引入无效角色时拒绝，合法 operator 模型覆盖保留并与实际 load 一致。
- [x] `publication_io_failure_keeps_original_target_and_emits_no_ready`：在 rename 边界遇到既有目录时保留原内容、无 Ready、清理临时文件。
- [x] `malformed_config_does_not_create_target_or_signal_ready`：无效 TOML 不创建配置目录或发送 Ready。
- [x] `noop_and_absent_optional_roles_do_not_validate_unused_profiles`：已支持的 noop/未配置可选角色不被新增校验误拒；未引用错误 profile 不拦截安装。
- [x] `cli_keeps_fragment_layout_and_refuses_to_replace_existing_configuration`：CLI 保持模块化布局并真实 load 所有角色；既有主文件/分片不被默认值覆盖。

Root 统一窗口命令：

```sh
cargo test -p alia-server --lib oobe::routes::tests -- --nocapture
```

除本组外，Root 应保留现有配置加载回归和 setup 生命周期回归；Linux/macOS 后续结果只按实际运行或编译记录，不把仅源码检查计为通过。

## setup graceful 只读复核

本 agent 有界读取了 `bootstrap/setup.rs`、`bootstrap/runtime.rs::HttpListener` 和 `crates/transport/src/connections.rs`，没有修改源码或另行运行验证，未发现新增可操作问题：

- `ConfigReady` / cancel / input 完成后保留同一个 pinned server future；2 秒 timeout 借用该 future，不会先将原 HTTP 服务丢弃。
- `Connections::close` 在同一 admission mutex 下禁止后续 wrap，并取消已接受连接的 read/write 等待。强制关闭后继续 await 原 server；当前 setup 路由的待处理异步工作是 HTTP body，未出现脱离 socket 的长期任务。
- 正常完成、外部取消及监听/输入错误都进入关闭路径，随后 join CLI 输入 worker；`SetupConnections` 的 Drop 同样封住连接，取消 guard 使输入 poll 退出。

这一审阅确认了原 ConfigReady 响应竞争的修复结构，运行证据仍以上述两项真实 setup 回归为准；不扩展成所有 HTTP 协议或文件系统故障已经穷举。


## V4 macOS 验收回填

Root 已取得完整冻结 V4 结果：workspace **1053 passed / 0 failed / 5 ignored**，319.39 秒，633 输入前后哈希无漂移。Server lib 309/0/3、server_runtime 53/0/0、duplex_playback 9/0/0；上述新增测试均包含其中，不另加总。真实 PTY 3 项及实际 CPU Easy-Turn 2 项另行通过，合计 **1058 unique macOS tests**；Server/Embedded no-default all-targets、fmt 也通过。V3 超时仍没有已证明的根因，不因本轮通过删除历史失败。最终 Linux/提交状态以 [整包摘要](validation-summary.json) 和 [Root交接](runtime.md) 为准。

最终源码：`8f5bb83f4a313723428abf854c1e663b159c6c45`；[已提交输入核对](committed-source-verification.json)、[整包验收](validation-summary.json)。后续从 [检查点](../../resume-checkpoint-2026-09-28.md) 和 R02/R03/R05/R06 checklist 接续。
