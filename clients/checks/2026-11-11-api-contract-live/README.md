# 真实后端 API 契约检查 — 2026-11-11

状态：本轮真实后端检查完成。按项目验证脚本指定的 PostgreSQL 17 重跑完整服务端检查，54 个顶层 target、997 个顶层用例：**988 通过、0 失败、9 跳过，退出 0**，其中包含全部 7 个新增 HTTP 用例。原始 PostgreSQL 18 首轮的 3 个失败保留在下方，纠正 WASM 构建方式并改用项目数据库版本后，这 3 个用例全部通过。两套临时数据库均已停止，端口拒绝连接，数据目录保留。机器可读结果见[最终汇总](final-summary.json)。

起点为干净的 `0bf23731bc407cb6259be8a5f0a857642159dcb5`（`codex/api-contract-errors`），见[环境记录](logs/00-preflight.log)。本轮实现范围只有新增 [`api_contract_auth.rs`](../../../packages/alia-server/tests/server_runtime/api_contract_auth.rs)、[`api_contract_conversations.rs`](../../../packages/alia-server/tests/server_runtime/api_contract_conversations.rs)，以及在 [`server_runtime.rs`](../../../packages/alia-server/tests/server_runtime.rs) 注册这两个测试模块。检查文件和本文另行留档；未更改生产代码、依赖或原有测试断言。

## 独立数据库与运行范围

用户授权独立测试数据库和真实后端联调，不操作真实设备。本轮先安装并使用 Homebrew 原生 PostgreSQL 18.6 和 pgvector 0.8.7，随后安装 PostgreSQL 17.11；这是本机系统工具安装，没有新增仓库依赖，安装的工具保留。Docker / OrbStack 原先停止，本轮未启动，也未重启旧服务，见[工具安装](logs/02-install-isolated-db-tools-20261111T073647452381Z.log)、[PostgreSQL 17 安装](logs/16-install-postgresql17-20261111T083213991801Z.log)及[环境记录](logs/00-preflight.log)。

项目 [`verify-software.sh`](../../../scripts/verify-software.sh) 指定的数据库镜像是 `pgvector/pgvector:pg17`。首轮误选了本机 PostgreSQL 18；定位版本差异后，[初始化另一套独立的 PostgreSQL 17 集群](logs/18-init-pg17-new-20261111T083351691801Z.log)完成重跑。没有执行 Homebrew 默认数据库服务启动。PostgreSQL 18 和 17 临时集群均已[正常停止](logs/26-verify-stopped-20261111T084056776290Z.log)并保留目录。

两套集群均独立初始化，没有使用或恢复旧数据库：

| 用途 | 实际版本与扩展 | 回环监听 | 独立数据目录 | SQL 核对 |
| --- | --- | --- | --- | --- |
| 首轮 | PostgreSQL 18.6 / pgvector 0.8.7 / pgcrypto 1.4 | `127.0.0.1:52558` | `/private/tmp/alia-api-contract-20261111-ks58v_1w/data` | [18 查询](logs/08-verify-test-database-20261111T074007255764Z.log) |
| 按项目版本重跑 | PostgreSQL 17.11 / pgvector 0.8.7 / pgcrypto 1.3 | `127.0.0.1:50196` | `/private/tmp/alia-api-contract-pg17-20261111-u5orc0u9/data` | [17 查询](logs/21-verify-pg17-20261111T083435478780Z.log) |

两套数据库名均为 `alia_api_contract`，角色均为 `alia_test`。切换版本时，准备脚本分配端口被沙箱拒绝，目录标记仍指向原 PostgreSQL 18 非空目录，首次 PostgreSQL 17 `initdb` 因而[拒绝非空目录并退出 1](logs/18-init-pg17-20261111T083326518371Z.log)。这是准备步骤失败，随后改用新的 17 路径成功；未清空或覆盖原目录。数据库目录和连接信息文本未复制进本目录；数据库日志仅在各集群正常停止后复制最终版本。

运行的 Server 使用真实 HTTP 路由、鉴权、事务和 SQL。共用 fixture 清除设备 channels，关闭 discovery、Home Assistant、Easy-Turn，STT / TTS / LLM / Embedding 选 noop；需要适配器行为的原有用例仅连接自己创建的回环服务。配对用例构建的本地 Embedded 关闭所有硬件和模型槽位。本轮没有连接生产数据库、真实模型服务或真实设备，没有使用 `--ignored` / `--include-ignored`。

## 新增用例与已确认结果

改动前针对的反例包括：大小写或空白不同的同一邮箱并发注册；两个刷新同时使用同一旧 token；账户验证 SQL 被真实表锁阻塞到期限后误报 401；停用账户继续访问；请求提取错误的媒体类型与 OpenAPI 不符；普通用户删除他人会话；超限历史和任意 JSON 被截断或错误转型。用例没有用替换数据库、缩短生产期限或放宽原断言来制造通过。

7 个用例均通过，独立执行的完整输出见 [HTTP 契约日志](logs/09-real-http-contract-20261111T074012759777Z.log)，退出码 0，耗时 3.87 秒。全部 7 项也在 PostgreSQL 17 的[完整服务端检查](logs/22-server-suite-pg17-20261111T083500837090Z.log)中通过，没有另行重复单跑。它们覆盖本批声明的 8 个操作：`POST /api/v1/auth/logout`、`PATCH /api/v1/users/me`，以及会话列表、详情、删除、消息、事件和表达跟踪 6 个操作；同时通过注册、登录、刷新和账户查询验证关联行为。

| 用例 | 实际检查 |
| --- | --- |
| `http_registration_normalizes_email_and_serializes_duplicate_writes` | PostgreSQL 证明两个请求同时到达锁屏障；一个 200、一个 409 `ALREADY_EXISTS` / `email_already_registered`，仅一条用户、身份链接和刷新记录；后续重复注册仍为 409。 |
| `http_auth_reasons_rotation_logout_and_inactive_accounts` | 缺失和无效 access token、错误登录凭据、无效 refresh token 的独立 reason；并发刷新仅一方成功；旧 token 不可再刷新；登出幂等；停用账户被拒绝且不消费原 refresh token；已过期 refresh token 被拒绝。 |
| `http_account_lookup_lock_timeout_is_503_and_recovers_after_unlock` | 真实 PostgreSQL 表锁使账户查询约 2 秒后返回 503 `SERVICE_UNAVAILABLE` / `account_verification_unavailable`，释放锁后同一个 access token 返回 200；独立执行记录 2003 ms，PostgreSQL 17 完整检查记录 2002 ms。 |
| `http_json_extractors_and_patch_me_match_declared_wire_contract` | JSON 语法错误 400、错误媒体类型 415、字段错误 422、超限请求体 413 的真实 `text/plain` 回执；自助资料更新、普通用户改权限被拒绝和字段校验。 |
| `conversations_http_authority_rejections_and_owned_delete` | 本人、他人、无所有者和不存在会话的权限区别；管理员读取；UUID / Query 提取错误；只删除本轮创建的会话并核对级联数据，其他人的记录保持。 |
| `conversations_http_preserves_json_and_filters_trace_sources` | `null`、数组、标量 JSON 保真；消息与事件排序；表达跟踪来源过滤、turn 过滤和空结果。 |
| `conversations_http_zero_default_and_clamped_limits` | 列表与历史的零值、默认值、上限钳制；真实创建 501 个会话和 5001 条历史后核对返回数量。 |

认证用例检查实际状态码、媒体类型、错误枚举和成功字段；会话用例另外按本轮 `ApiDoc` 的组件 schema 做离线 JSON Schema 校验。这不是自动读取每个 operation 的响应声明再逐一校验。纯文本提取错误另查状态、媒体类型和非空内容。测试中用于删除的账户、会话、消息和事件全部由对应临时 schema 自己创建。两名独立复核者已交叉审读对方新增测试，未报告阻断问题，审阅期间未改文件。

## 完整服务端检查与收尾结果

| 检查 | 当前结果 | 日志 |
| --- | --- | --- |
| 新增 HTTP 用例编译 | 退出 0 | [编译](logs/03-compile-http-contract-20261111T073726848741Z.log) |
| 初次 WASM fixture 构建 | 命令退出 0，但从仓库根目录运行 `--manifest-path`，漏用了示例目录 `.cargo` 中的 `--max-memory=16777216` 链接参数，产物不满足原有运行时测试 | [初次构建](logs/04-wasm-fixtures-20261111T073727981604Z.log) |
| 7 个新增 HTTP 用例 | 7 通过、0 失败、0 跳过 | [HTTP](logs/09-real-http-contract-20261111T074012759777Z.log) |
| PostgreSQL 18 首轮完整 `cargo test -p alia-server --offline --locked --no-fail-fast -- --nocapture --test-threads=4` | 54 个顶层 target；997 个顶层用例：985 通过、3 失败、9 跳过；退出 101 | [完整首轮](logs/10-server-suite-20261111T074054436502Z.log)、[逐 target 统计](server-suite-initial-summary.json) |
| 服务端边界检查 | 退出 0 | [边界检查](logs/11-server-boundaries-20261111T074055638152Z.log) |
| 格式检查 | 退出 0 | [格式](logs/12-format-20261111T074056783432Z.log) |
| 在 `examples/wasm-plugins` 目录重建 WASM fixture | 退出 0；两项原有 WASM 用例在最终完整检查均通过 | [纠正构建目录](logs/13-wasm-fixtures-correct-cwd-20261111T082734979428Z.log) |
| `cargo clippy -p alia-server --all-targets --offline --locked` | 退出 0；保留完整警告输出 | [Clippy](logs/14-clippy-20261111T082736110420Z.log) |
| PostgreSQL 18 迁移目录核对 | 17 条残留 `body_%` 约束全部为 `contype=n` / `NOT NULL`，失败原因已确定；未改实现或原有断言 | [目录查询](logs/15-pg18-constraint-evidence-20261111T083208794620Z.log) |
| PostgreSQL 18 正常停止 | 退出 0；数据库日志记录 `database system is shut down`，目录保留 | [停止](logs/17-stop-pg18-20261111T083243370893Z.log)、[最终数据库日志](logs/postgresql.log) |
| 按项目脚本同主版本的 PostgreSQL 17 建库 | PostgreSQL 17.11 独立集群和扩展核对完成 | [初始化](logs/18-init-pg17-new-20261111T083351691801Z.log)、[启动](logs/19-start-pg17-20261111T083406235087Z.log)、[建库](logs/20-create-pg17-database-20261111T083420742608Z.log)、[核对](logs/21-verify-pg17-20261111T083435478780Z.log) |
| PostgreSQL 17 完整 `cargo test -p alia-server --offline --locked --no-fail-fast -- --nocapture --test-threads=4` | **54 个顶层 target；997 个顶层用例：988 通过、0 失败、9 跳过；退出 0**。包含 7 个新增用例，原 3 个失败用例均通过 | [最终完整检查](logs/22-server-suite-pg17-20261111T083500837090Z.log)、[逐 target 统计](server-suite-final-summary.json) |
| `cargo check -p alia-server --no-default-features --all-targets --offline --locked` | 退出 0，耗时 25.38 秒；仅编译检查 | [无默认特性编译](logs/23-no-default-features-20261111T083501992752Z.log) |
| PostgreSQL 17 最终状态 | 仅剩应用 schema `public`；其他数据库客户端数为 0 | [SQL 核对](logs/24-pg17-final-database-state-20261111T084010394091Z.log) |
| PostgreSQL 17 正常停止 | 退出 0；最终数据库日志记录 `database system is shut down` | [停止](logs/25-stop-pg17-20261111T084020223826Z.log)、[数据库日志](logs/postgresql17.log) |
| 两套临时数据库停止核对 | 两套 `pg_ctl status` 均退出 3，`postmaster.pid` 均不存在；`127.0.0.1:52558` 和 `127.0.0.1:50196` 均 `ECONNREFUSED`；数据目录均保留 | [停止核对](logs/26-verify-stopped-20261111T084056776290Z.log) |

首轮 3 个失败是：

- `expression::abi::wasm_tests::real_wasm_appraisal_requires_exact_persistent_pin_and_is_revocable`：WASM fixture 构建目录错误，纠正构建命令后最终通过。
- `wasm::wasm_admin_lifecycle_reaches_unified_execution_and_invalidates_old_grants`：同一 WASM fixture 问题，纠正构建命令后最终通过。
- `motion_rename_preserves_historical_json_and_unknown_without_replay`：失败于“残留 `body_%` 约束应为 0”的断言，实际返回 17。[实际目录查询](logs/15-pg18-constraint-evidence-20261111T083208794620Z.log)证明 17 条全部是 PostgreSQL 18 新增纳入 `pg_constraint` 的 `NOT NULL` 约束；原迁移显式改名的主键、外键、唯一和检查约束没有残留。执行到此之前的历史 `unknown` 状态、dispatch ID、command JSON、progress JSON 和事件内容断言均已通过，不能将这个命名差异写成历史数据损坏。PostgreSQL 18 开始将列的非空声明记录进该目录，见[官方版本说明](https://www.postgresql.org/docs/18/release-18.html)。本轮没有放宽断言或改写既有迁移；同一用例在 PostgreSQL 17 的完整重跑通过。PostgreSQL 18 的命名兼容事项继续保留在设计文档，不因 PostgreSQL 17 通过而关闭。

两轮统计均按 Cargo 每个 `Running` / `Doc-tests` 段最后一条 `test result` 计算，各排除子进程内部的 5 条重复 summary。日志里的预期 panic 和故障注入输出按用例最终状态判断，没有把它们记为失败。两轮都是相同的 997 个顶层用例，9 项跳过的名称和原因一致。首轮日志与最终重跑分别保存，不将它们相加为更多用例，也不抹去首次失败。

## 9 个跳过用例及原始原因

| 用例 | `#[ignore]` 原因（原文） |
| --- | --- |
| `bootstrap::setup::tests::setup_terminal_worker_releases_input_after_cancellation` | requires a real PTY; run with --ignored --nocapture --test-threads=1 |
| `channels::cli::tests::idle_terminal_worker_joins_and_restores_terminal_on_shutdown` | requires a real PTY; run with --ignored --nocapture --test-threads=1 |
| `channels::cli::text::tests::real_console_write_proves_utf8_bytes_and_restores_terminal` | requires a real PTY; run with --ignored --nocapture --test-threads=1 |
| `r1_live::remote_r1_natural_long_speech_real_server_duplex_and_rewake` | opens an explicitly configured remote R1/ALSA node; requires natural long speech with breaths, two short utterances, rewake and low-volume playback |
| `r1_live::remote_r1_short_speech_real_server_duplex_and_rewake` | opens an explicitly configured remote R1/ALSA node; requires human short utterances, rewake and low-volume playback; does not validate natural long speech |
| `r1_live::remote_r1_system_control_transport` | uses an explicitly configured remote R1 and real Server; automatic PCM transport and Start/Stop checks with an ordinary Embedded restart gate, no speech, playback or UART initialization |
| `r1_live::remote_r1_system_start_stop_and_silent_recovery` | requires explicitly initialized remote R1, three ordinary speech windows, 300 seconds silent idle, operator gates and an ordinary Embedded process restart |
| `official_audio_through_production_rust_adapter` | requires a real Easy-Turn service and ALIA_EASY_TURN_PCM pointing to official backchannel_real_1 PCM16 |
| `official_audio_over_grpc_preserves_original_response_across_backchannel_and_wait` | requires real Easy-Turn service, ALIA_EASY_TURN_PCM_DIR and disposable ALIA_TEST_DATABASE_URL |

## 完整日志与后续

本轮 **31 份完整日志、728,606 字节**全部复制完毕，包含两套数据库停止后的最终日志，逐字节核对源文件并校验 SHA-256，见[日志清单](log-manifest.json)。[最终汇总](final-summary.json)关联原始失败、最终 54 个 target 的结果、7 项新增用例、编译检查和停库事实。原件在 `run/api-contract-live-2026-11-11/`，初次被权限拒绝的 `initdb`、误用非空目录被拒绝、纠正后的命令和失败输出均保留。未复制数据目录和连接信息文本。

已将测试脚本直接信任外部数据库 URL、`plugin_storage` 未建独立 schema、PostgreSQL 18 的迁移约束命名三项问题追加到 [API 设计待考察事项](../../../packages/alia-server/docs/api-design-follow-ups.md)，只记录调用路径、影响和后续决策，没有改实现。

本轮授权范围内的 HTTP 检查、完整服务端检查、编译检查和临时服务收尾均已完成。后续可按单独范围处理上述 3 项设计事项；当前无需用户新增决定。本轮没有验证真实设备、真实模型、生产数据库、浏览器到真实后端的完整交互、压力或长时间运行，也没有因此宣称这些范围通过。
