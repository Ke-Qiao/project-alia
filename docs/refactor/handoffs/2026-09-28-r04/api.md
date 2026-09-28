# R04-02 Companion REST / OpenAPI handoff

更新：2026-09-28；负责：`companion_policy_28`。基线提交 `a79cb1f0b2c45847dce561dfde549f4318a1312a`。本文件只记录 R04-02，R01 的 630 输入冻结结果不覆盖这些新改动。Root 的 R04 V4 macOS 默认 feature workspace 已通过；API 契约与七项 Companion HTTP/生命周期回归均通过。Root 已完成其余 macOS 检查，Linux V4 两端编译也已通过；Root已完成记录与源码提交 `8f5bb83`。

## 实现与边界

- [x] 读取四个 handler、AuthUser/AppError、service/repository 调用链、全部响应 DTO 及现有 HTTP 回归后实施。
- [x] `api/companion.rs` 为 policy GET/PUT、shadow POST、replay GET 注册 `utoipa::path`，写明当前 active owner、CAS、410、两类 mutation 503 和 read 503。
- [x] `PolicyRequest` / `ShadowRequest` 注册真实 schema，保留 deny_unknown_fields；政策完整七字段、revision 正数和现有数值边界进入契约。
- [x] `modules/companion/types.rs` 仅增加 ToSchema derives/schema 元数据；Root 已明确允许 `repository.rs` 的实际 `ShadowReport` / `ShadowDecision` 增加 ToSchema，未复制响应 DTO。
- [x] 特殊错误从 `json!` 改为 typed tagged enum 序列化，状态码、code、message、身份字段均保持原语义。shared AppError 路径未改。enum 的各变体分别要求自己的原 revision / operation 字段。
- [x] ApiDoc paths/components/tag 收录四操作，现有 Scalar 直接使用该 ApiDoc。
- [x] 新增一个 `companion_contract_matches_owner_routes_required_fields_and_real_errors` 回归：序列化方法/请求必填/边界/Bearer/security/嵌套响应 schema；对照真实 AppError 和特殊错误 helper 的九种输出，核对 status、Content-Type、code、必填身份、原 message；保留 Axum Path<Uuid> 的 plain-text 400。
- [x] [操作说明](../../../operations/companion.md) 覆盖 GET→保存原意图→CAS PUT、409、先读后重试的未知结果、shadow 原身份与原来源集合、410 不可重建，含后续 checklist。
- [x] 指定 Rust 文件 `rustfmt --edition 2024 --config skip_children=true` 和范围内 `git diff --check` 已通过。
- [x] 新增 ApiDoc 回归已由 Root 在 [V2 server-lib](../../../../run/validation-20260928-r04/server-lib-v2.log) 和 [V3 workspace](../../../../run/validation-20260928-r04/workspace-v3.log) 中运行通过；V3 Server lib 总结果为 309 passed / 0 failed / 3 ignored。这不代表 V3 全包通过。
- [x] V4 真实 PostgreSQL / HTTP 的七项既有 Companion 回归全部通过，包含本轮非法 replay UUID 的 400 text/plain 断言；ApiDoc 回归在同一 V4 也通过，见 [workspace-v4.log](../../../../run/validation-20260928-r04/workspace-v4.log)。
- [x] Root审查、实际测试结果回填、R04最终manifest归档及正常本地源码提交 `8f5bb83` 完成，633输入逐项一致。

本agent未运行macOS Cargo/PG；统一macOS验证由Root执行，本agent仅执行下文Linux编译。未启动或调用用户 Server、未执行文档 curl、未修改实际政策、未新增 CLI/前端/依赖，也未自行提交。编译或契约检查失败时由 Root 协调修复与重跑；不能把 rustfmt 当作 ApiDoc 验证。

## 对应验证命令

```sh
cargo test -p alia-server --lib \
  openapi::tests::companion_contract_matches_owner_routes_required_fields_and_real_errors \
  --locked --offline

# 使用 Root 的隔离真实 PostgreSQL fixture 环境，7 个既有 companion_ 测试。
cargo test -p alia-server --test server_runtime companion_ --locked --offline
```

回归继续沿用既有真实 HTTP 测试证明 owner/CAS/超时/擦除；新 ApiDoc 用例只验证发布契约和实际响应构造器，不能替代数据库动态授权或 Pi 真机验收。

## Linux 准备历史与当前状态

R01 的 manifest、源码、缓存和证据保留，不被 R04 结果覆盖。各轮都使用独立目录与容器，旧缓存通过 APFS `cp -cR` 写时复制后使用。镜像为已存在的 ARM64 native `sha256:6cd7a7a06976f8651cdb57fcde457e9dc7e2abe20587900dd750e1f373a25c70`；源码与 Cargo registry 只读挂载，target/cache 独立可写。

| 轮次 | 实际状态与证据 | 后续处理 |
| --- | --- | --- |
| V1 | 容器仅创建、未启动；0 源码复制、0 Cargo/测试。Root macOS 编译先暴露测试中的未声明 tower import，见 [superseded.json](../../../../run/validation-20260928-r04/linux-arm64-v1/evidence/superseded.json)。 | 保留证据，取消本轮 Linux 检查。 |
| V2 | 633 输入已复制，host/container 哈希通过；SHA `8994cf93c68e3fe6aa81c767b6541eafd4e0889b389286bad9e901e2e026b28f`。工具版本已读，**从未执行 Cargo/PG/测试**；容器已停止，见 [final state](../../../../run/validation-20260928-r04/linux-arm64-v2/evidence/docker-final-state.json)。 | 保留源码、缓存和证据，不再启动 V2 编译。 |
| V3 | 未准备 Linux 目录、容器或快照，未执行 Linux 检查。Root macOS workspace 在三个既有 duplex 场景超时后停止；原二进制独立重跑 9/9 通过不能代替完整全包验收。 | Root 在 V4 增加诊断并重跑。 |
| V4 | Root 已冻结 633 输入，manifest SHA `35efd1807b5c2820b3024fc6142bf769ace5973cc13275e0b5649f8bec011a8b`；独立 Linux 快照及两端编译已完成，633 输入前后哈希一致；容器已停止。 | 本轮 Linux 验证完成，窗口已归还 Root；证据保留。 |

V1 的 ApiDoc 测试已移除 tower/独立 oneshot 路由及相关 imports，保留 text/plain schema 断言。原有 `companion_shadow_http_has_durable_retry_and_erasure_without_side_effects` 增加三行真实 HTTP 验证：已认证的 `/replay/not-a-uuid` 返回 400 且 Content-Type 为 text/plain；没有引入依赖、helper 或改变运行语义。

V2 macOS server-lib 为 308 passed / 1 failed / 3 ignored，唯一失败来自 TCP connection-refused fixture 的测试等待，由负责 health probe 的 agent 修复夹具；这不是 Linux 失败。V3 macOS 全包未完成也不能记为 Linux 编译失败。

- [x] V1/V2 准备状态、取消原因和停止记录保留；无 Linux Cargo 通过或失败的虚构记录。
- [x] 规范 handoff 使用当前状态，旧准备阶段的 idle / 待编译不再作为恢复入口。
- [x] V4 独立源码复制及 host/container 准备前后哈希核对；633 输入全部通过，0 差异。
- [x] Root 明确交出唯一 Cargo 窗口后，已顺序执行 Server → Embedded `--no-default-features --all-targets --locked --offline -j2`，两项 exit 0；source/Cargo.lock 前后哈希保留。
- [x] V4 容器已停止，缓存/快照/证据保留，唯一验证窗口已归还 Root。

准备阶段（08:31 UTC）：`alia-r04-arm64-20260928-v4` 当时仅运行 idle sleep；Rust 1.95.0 `aarch64-unknown-linux-gnu`、ALSA 1.2.8、DBus 1.14.10、protobuf 3.21.12 已重新读取。新缓存直接从停止的 R01 cache 写时复制，未复用 V2 源码，也未修改旧轮证据。**截至该准备时点 V4 Linux Cargo / PG / 测试均为 0 次**；实际编译见后面的完成记录。准备证据见 [source-prepared.json](../../../../run/validation-20260928-r04/linux-arm64-v4/evidence/source-prepared.json)、[environment-summary.json](../../../../run/validation-20260928-r04/linux-arm64-v4/evidence/environment-summary.json)；其中记录工作区与快照的前后逐文件哈希，以及容器前后 `sha256sum -c` 的实际退出码。

V4 macOS 补记：已只读核对 Root 的 [workspace 结果](../../../../run/validation-20260928-r04/workspace-v4-result.json) 与日志；默认 feature workspace 为 **1053 passed / 0 failed / 5 ignored**，66 Running targets + 5 Doc-tests，319.39 秒，exit 0、source_drift=[]。这覆盖本页 ApiDoc 和七项 Companion HTTP/生命周期回归；后续 no-default、PTY、模型与 Linux 检查由 Root 各自记录，不能从这份默认 feature workspace 结果推算。

## V4 Linux ARM64 最终结果与窗口释放

Root 明确结束 macOS 验证并移交唯一窗口后，本 agent 运行 `linux-arm64-v4/evidence/run-checks.py`，按顺序完成实际编译：

| 命令 | 真实结果 | 外部进程耗时 |
| --- | --- | --- |
| `cargo check -p alia-server --no-default-features --all-targets --locked --offline -j2` | exit 0 | 16.18 秒 |
| `cargo check -p alia-embedded --no-default-features --all-targets --locked --offline -j2` | exit 0 | 1.93 秒 |

[结果摘要](../../../../run/validation-20260928-r04/linux-arm64-v4/evidence/result-summary.json)包含每个实际命令及退出码。host/container 的 **633 inputs** 编译前后逐项哈希均通过，manifest 保持 `35efd1807b5c2820b3024fc6142bf769ace5973cc13275e0b5649f8bec011a8b`；最终工作区源码再核对也为 `workspace_source_errors=[]`。Cargo.lock 前后均为 `845d709734b382d24baaec28bd9d69a524e92c4b41366184de02f0ef7a322dfc`。

08:40 UTC 已停止 `alia-r04-arm64-20260928-v4`，确认 exited / Running=false，见 [docker-final-state.json](../../../../run/validation-20260928-r04/linux-arm64-v4/evidence/docker-final-state.json)。源码快照、独立缓存、旧轮环境和全部证据保留；本 agent 已释放唯一 Cargo/PG 验证窗口。这里仅为 ARM64 Linux native 编译，没有运行 Linux 测试、PostgreSQL 或模型，不等于 Pi 真机安装/声学验收。

最终源码：`8f5bb83f4a313723428abf854c1e663b159c6c45`；[已提交输入核对](committed-source-verification.json)、[整包验收](validation-summary.json)。后续从 [检查点](../../resume-checkpoint-2026-09-28.md) 和 R02/R03/R05/R06 checklist 接续。
