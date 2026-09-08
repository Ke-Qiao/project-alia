# 隔离平台验证

更新：2026-09-08。最新验证覆盖已提交的 P2 `5aecb38cc9bceb51f1b424c99f6e76f944f0a544`；下方保留早期 `ef07d43` 结果。两次都使用固定 `git archive`，不把正在修改的工作区当作已验证源码。

## P2：固定提交 5aecb38

本次将 `5aecb38cc9bceb51f1b424c99f6e76f944f0a544` 导出到 `/private/tmp/alia-platform-5aecb38.rTx8mP/source` 并只读挂载为 `/src`。上次 `/private/tmp/alia-platform-ef07d43.maWX2V/cache` 通过 macOS APFS `cp -cR` 复制到本任务独立 `cache`；后续构建没有覆盖旧缓存或旧产物，也没有挂载宿主 Cargo 缓存、工作区源码或 Docker socket。

环境保持官方 `rust:1.95-bookworm` 的 Linux ARM64 镜像：

| 输入或环境 | 本次实际值 |
| --- | --- |
| 镜像固定摘要 | `rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1` |
| ARM64 image ID | `sha256:7cf1e580ef5539f03b58560753e8ab84c8c360960d99dff714004aa98f203977` |
| Cargo.lock SHA256 | `f602172bab2485216d75eeb6bfffd0b5321cefadd12b08ec39d92fdc3e7e129e`，开始/结束相同 |
| Rust / Cargo | `rustc 1.95.0 (59807616e 2026-04-14)` / `cargo 1.95.0 (f2d3ce0bd 2026-03-21)` |
| Rust host / LLVM | `aarch64-unknown-linux-gnu` / `22.1.2` |
| 系统 | Debian 12，Docker Engine 29.4.0，OrbStack kernel `7.0.14-orbstack-00380-ga7e0a2dc9535` |
| 隔离限制 | 6 CPU、12 GiB、512 PIDs，`CARGO_BUILD_JOBS=4` |
| 任务容器 | `alia-platform-5aecb38-rtx8mp`，`--rm` 退出后已确认移除 |
| 真实数据库 | `host.docker.internal:32768/alia_refactor_p2_verify2`，专用测试账号 `alia_test`；PostgreSQL `17.10 (Debian 17.10-1.pgdg12+1)`，aarch64 |

容器安装了下方历史环境表中的同版本系统构建依赖，额外增加数据库探测客户端 `postgresql-client 15+248+deb12u1` 和 `postgresql-client-15 15.19-0+deb12u1`。真实 SQL `SELECT current_database(), version()` 已成功确认库名；没有连接用户的 5433 端口或操作用户 `alia-postgres`。库测试通过 `ALIA_TEST_DATABASE_URL` 连接该专用实例，创建和删除自身随机 schema；数据库缺失会导致测试明确失败，不会跳过。

### P2 实际结果

| 检查 | 命令 | 实际结果 | 墙钟耗时 |
| --- | --- | --- | --- |
| Server 默认 features，所有 targets | `cargo check -p alia-server --all-targets --locked` | exit 0，无警告 | 17.78 s |
| Server 默认 features，库测试 | `cargo test -p alia-server --lib --locked` | exit 0；152 passed / 0 failed / 2 ignored | 37.52 s，测试执行 0.35 s |
| Embedded 默认 Sherpa features，完整构建 | `cargo build -p alia-embedded --locked` | exit 0，编译及最终链接通过，无警告 | 15.07 s |

三阶段实际时间为 2026-09-08 **02:59:30–03:00:40 UTC**（北京时间 **10:59:30–11:00:40**）。这些耗时使用复制的现有缓存，不是全新缓存基准。未修改代码、Cargo.lock 或任何 feature 选择；Embedded 默认仍是 `wakeword-sherpa`、`stt-sherpa`、`direct-response`。

本次库测试中的 `providers::conversation::database::tests::durable_input_provenance_preserves_time_owner_and_atomicity` 在真实 PostgreSQL 上通过，覆盖实际来源行 UUID、原始接收时间、owner 校验和消息/会话事务回滚。容器自己的 loopback HTTP 测试也执行通过。两项 ignored 仍是 setup 与 CLI 真实 PTY 测试：`setup_terminal_worker_releases_input_after_cancellation`、`idle_terminal_worker_joins_and_restores_terminal_on_shutdown`，原因明确且未计为通过。

Embedded 产物为 `/private/tmp/alia-platform-5aecb38.rTx8mP/cache/target/debug/alia-embedded`，已由 `file` 确认为 ARM aarch64 ELF 64-bit PIE，解释器 `/lib/ld-linux-aarch64.so.1`，BuildID `1e27ee9a122f1f4e048074a2a0fb32bb56d54494`。产物 SHA256 为 `1f4c0723437d8fdcc12bd4ce5099ffaeeb3b2882c91bba0174a2f4b89305378c`；本次重新编译了 `alia-proto`、`alia-transport`、`alia-core`、`alia-embedded` 并完成链接，未启动设备进程。

Sherpa 继续使用缓存中的官方 `sherpa-onnx-v1.13.1-linux-aarch64-static-lib.tar.bz2`，重新核对 SHA256 为 `86d0d3818770d4f6a087e52691d3ae171393972030f133df064a271fe9fe1749`。没有设置 `DOCS_RS`、关闭 Sherpa 或下载任何模型。Cargo.lock 相比 `ef07d43` 只增加了 `alia-embedded` 对 `alia-core` 的工作区依赖，原生归档内容仍需独立摘要约束。

### P2 复现与证据

从仓库根目录执行。下面使用相同已授权的专用测试库；应先确保该库可达，不能改指向用户或生产数据库。缓存复制仅用于加速，可省略后从空缓存构建。

```bash
validation_dir=$(mktemp -d /private/tmp/alia-platform-5aecb38.XXXXXX)
mkdir -p "$validation_dir/source"
git archive 5aecb38cc9bceb51f1b424c99f6e76f944f0a544 | tar -x -C "$validation_dir/source"
cp -cR /private/tmp/alia-platform-ef07d43.maWX2V/cache "$validation_dir/cache"
docker run --rm --platform linux/arm64 --cpus 6 --memory 12g --pids-limit 512 \
  --mount "type=bind,source=$validation_dir/source,target=/src,readonly" \
  --mount "type=bind,source=$validation_dir/cache,target=/cache" \
  --env CARGO_HOME=/cache/cargo --env CARGO_TARGET_DIR=/cache/target \
  --env CARGO_BUILD_JOBS=4 --env DEBIAN_FRONTEND=noninteractive \
  --env ALIA_TEST_DATABASE_URL=postgresql://alia_test:alia_test@host.docker.internal:32768/alia_refactor_p2_verify2 \
  --workdir /src \
  rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1 \
  bash -eu -c '
    apt-get update
    apt-get install --no-install-recommends -y ca-certificates pkg-config protobuf-compiler libprotobuf-dev libssl-dev libasound2-dev libdbus-1-dev cmake clang time postgresql-client
    rustc -vV
    cargo -V
    sha256sum Cargo.lock
    psql -X -v ON_ERROR_STOP=1 "$ALIA_TEST_DATABASE_URL" -c "SELECT current_database(), version();"
    cargo check -p alia-server --all-targets --locked
    cargo test -p alia-server --lib --locked
    cargo build -p alia-embedded --locked
    sha256sum Cargo.lock
  '
```

实际逐阶段脚本及原始日志保存在 `/private/tmp/alia-platform-5aecb38.rTx8mP/evidence/`，包括 `run.sh`、`image-inspect.json`、`environment.txt`、`system-packages.txt`、`database-preflight.log`、各阶段 `.command/.start/.end/.status/.log`、`lock-after.txt` 与 `embedded-file.txt`。每个构建阶段独立限时 2400 秒，使用 `/usr/bin/time -v` 记录资源和耗时；三阶段退出码均为 0。

本轮未执行 Server 集成测试目标、Embedded 单测或二进制实机运行，也未启动真实 Easy-Turn/LLM/TTS、下载模型或验证 Pi 5 音频、蓝牙、AEC 和双讲。Linux ARM64 结果不证明 Linux x86_64、Windows 或 macOS Intel 原生支持。该固定提交之后的生命周期和 Memory batch 修改必须另行验证，不能沿用本次结果。

## 历史验证：固定提交 ef07d43

以下记录只覆盖 `ef07d43b937682df1b600447f14c370cff6a5dfa`，不包含其后播放协议、凭证续期与 P2 修改。保留用于说明缓存来源和首轮环境修复。

## 范围与环境

- 从 `git archive` 导出已提交源码，容器以只读方式挂载。没有使用工作区尚未提交的代码。
- `Cargo.lock` SHA256：`b09b7803651b9586dd094e835e5c24e2623034f04a4a3b0e4f6ac5bc7c3efa2d`。Cargo 命令均使用 `--locked`。
- 官方镜像：`docker.io/library/rust:1.95-bookworm`，固定摘要 `sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1`；实际运行平台为 Linux ARM64 / Debian 12。
- 本次解析到的 ARM64 image ID：`sha256:7cf1e580ef5539f03b58560753e8ab84c8c360960d99dff714004aa98f203977`。
- `rustc 1.95.0 (59807616e 2026-04-14)`；host `aarch64-unknown-linux-gnu`；LLVM `22.1.2`；`cargo 1.95.0 (f2d3ce0bd 2026-03-21)`。
- Docker Engine 29.4.0，OrbStack Linux kernel `7.0.14-orbstack-00380-ga7e0a2dc9535`。容器限制 6 CPU、12 GiB 内存、512 PIDs，`CARGO_BUILD_JOBS=4`。
- 验证目录：`/private/tmp/alia-platform-ef07d43.maWX2V`。源码、Cargo registry、target、日志均独立；未挂载宿主 Cargo 缓存、Docker socket 或用户数据库。
- 容器名 `alia-platform-ef07d43-mawx2v`，退出自动移除。未操作已有 `alia-postgres` 或其他用户容器。

容器额外安装的系统构建依赖：

| 包 | 实际版本 |
| --- | --- |
| ca-certificates | 20250419~deb12u1 |
| pkg-config | 1.8.1-1 |
| protobuf-compiler | 3.21.12-3+deb12u1 |
| libprotobuf-dev | 3.21.12-3+deb12u1 |
| libssl-dev | 3.0.20-1~deb12u2 |
| libasound2-dev | 1.2.8-1+b1 |
| libdbus-1-dev | 1.14.10-1~deb12u1 |
| cmake | 3.25.1-1 |
| clang | 1:14.0-55.7~deb12u1 |

`protoc --version` 为 `libprotoc 3.21.12`。系统包版本反映本次实际安装结果；复跑时 Debian 仓库可能提供更新版本，应重新记录。

## 本次结果

以下为补齐系统依赖后、同一固定源码快照的实际结果。三条命令退出码均为 `0`，结束后的锁文件 SHA256 与开始前相同。

| 检查 | 命令 | 实际结果 | 本次墙钟耗时 |
| --- | --- | --- | --- |
| Server 默认 features，所有 targets | `cargo check -p alia-server --all-targets --locked` | 通过 | 13.58 s |
| Server 默认 features，纯库测试 | `cargo test -p alia-server --lib --locked` | 136 passed / 0 failed / 2 ignored | 31.05 s，测试执行 0.35 s |
| Embedded 默认 Sherpa features，完整构建 | `cargo build -p alia-embedded --locked` | 编译及最终链接通过 | 14.37 s |

默认 features 未被替换成 `--no-default-features`。Server 库测试在容器自己的 loopback 上运行；不连接真实 Easy-Turn、LLM/TTS、数据库或物理设备。

Server 测试包含真实容器 loopback HTTP 请求、Easy-Turn HTTP 契约/取消/容量测试，以及注入 detector 的持续采集与过期结果测试；模型结果是测试注入，不是真实模型推理。两项 ignored 分别是 `bootstrap::setup::tests::setup_terminal_worker_releases_input_after_cancellation` 与 `channels::cli::tests::idle_terminal_worker_joins_and_restores_terminal_on_shutdown`，均明确需要真实 PTY。本次未执行它们，也未运行 Server 集成测试套件。

Embedded 默认 features 为 `wakeword-sherpa`、`stt-sherpa`、`direct-response`。产物 `/private/tmp/alia-platform-ef07d43.maWX2V/cache/target/debug/alia-embedded` 经 `file` 确认为 ARM aarch64 ELF 64-bit PIE，解释器 `/lib/ld-linux-aarch64.so.1`，包含调试信息、未 strip；本次没有启动该设备进程。

Server check 无警告；Embedded 构建有一项非阻断警告：`packages/alia-embedded/src/module/discovery.rs:5` 的 `BTreeMap` import 在此 Linux 构建未使用。没有因此修改生产代码。

成功阶段时间为 2026-09-07 18:56:50–18:57:49 UTC（北京时间 2026-09-08 02:56:50–02:57:49）。复跑复用了下述首轮的专用 Cargo 缓存，因此这些数字不是全新缓存构建基准。已确认一次性容器退出并自动移除，专用日志及缓存保留。

## 首轮环境缺项与原生依赖

首轮使用 `apt-get install --no-install-recommends protobuf-compiler ...`，缺少 `libprotobuf-dev`。三阶段均在 `alia-proto` 的 build script 退出 `101`，共同报错：

```text
protoc failed: google/protobuf/empty.proto: File not found.
binding.proto:5:1: Import "google/protobuf/empty.proto" was not found or had errors.
binding.proto:15:24: "google.protobuf.Empty" is not defined.
```

补充容器内 `libprotobuf-dev` 后三阶段通过；没有修改 `crates/proto/build.rs` 或 `.proto`。这是复现环境必须列出的构建依赖，不能把首轮失败计作已证实的源码兼容错误。首轮原始日志保存在 `evidence/attempt-1-missing-protobuf-headers/`。

锁定的关键依赖包括 `sherpa-onnx 1.13.1`、`sherpa-onnx-sys 1.13.1`、`cpal 0.15.3`、`alsa 0.9.1`、`bluer 0.17.4`、`network-interface 1.1.4`、`tonic 0.13.1`、`openssl 0.10.79`、`openssl-sys 0.9.115`。

本次未设置 `DOCS_RS`、`SHERPA_ONNX_LIB_DIR` 或禁用 Sherpa。`sherpa-onnx-sys 1.13.1` build script 从官方 release 下载了 [Linux aarch64 静态库归档](https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.1/sherpa-onnx-v1.13.1-linux-aarch64-static-lib.tar.bz2)，缓存于独立 `target/sherpa-onnx-prebuilt/`。实际归档 SHA256 为 `86d0d3818770d4f6a087e52691d3ae171393972030f133df064a271fe9fe1749`。

Cargo.lock 固定 Rust crate，并不固定该 build script 额外下载的原生归档内容；上面的摘要记录本次输入。发行构建需要保留并校验原生归档，可使用上游已有的 `SHERPA_ONNX_ARCHIVE_DIR` 指定经过校验的本地归档，不能仅凭 Cargo.lock 宣称原生输入也已固定。

## 复现

在仓库根目录执行；下面命令只创建独立临时目录和一次性容器。Cargo 缓存和编译产物保留在该临时目录，方便核验和后续手动清理。

```bash
validation_dir=$(mktemp -d /private/tmp/alia-platform-ef07d43.XXXXXX)
mkdir -p "$validation_dir/source" "$validation_dir/cache"
git archive ef07d43b937682df1b600447f14c370cff6a5dfa | tar -x -C "$validation_dir/source"
docker run --rm --platform linux/arm64 --cpus 6 --memory 12g --pids-limit 512 \
  --mount "type=bind,source=$validation_dir/source,target=/src,readonly" \
  --mount "type=bind,source=$validation_dir/cache,target=/cache" \
  --env CARGO_HOME=/cache/cargo --env CARGO_TARGET_DIR=/cache/target \
  --env CARGO_BUILD_JOBS=4 --env DEBIAN_FRONTEND=noninteractive --workdir /src \
  rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1 \
  bash -eu -c '
    apt-get update
    apt-get install --no-install-recommends -y ca-certificates pkg-config protobuf-compiler libprotobuf-dev libssl-dev libasound2-dev libdbus-1-dev cmake clang time
    rustc -vV
    cargo -V
    sha256sum Cargo.lock
    cargo check -p alia-server --all-targets --locked
    cargo test -p alia-server --lib --locked
    cargo build -p alia-embedded --locked
    sha256sum Cargo.lock
  '
```

本次逐阶段保留了 `/private/tmp/alia-platform-ef07d43.maWX2V/evidence/` 下的环境、镜像信息、系统包版本、命令、开始/结束时间、退出码和完整日志。每个 Cargo 阶段独立限时 2400 秒，并通过 `/usr/bin/time -v` 记录耗时及资源使用；失败后仍尝试下一独立阶段，避免把单一失败误判为所有目标均失败。

## 结论边界

Linux ARM64 容器结果不能证明 Raspberry Pi 5 实机、音频设备、蓝牙守护进程、Sherpa 模型质量、AEC 或自然双讲已通过。它也不证明 Linux x86_64、Windows、macOS Intel 原生支持。后续源码提交需要针对其变化重新验证。
