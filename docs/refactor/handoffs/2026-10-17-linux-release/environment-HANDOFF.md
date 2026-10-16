# Linux 软件发布验收：环境与原生链接 HANDOFF

日期：2026-10-17。负责人：`v01_fixture`。本轮基线为 `da3eca2`，生产源码身份为 `4e2968f18d765f326e76d24a3bcdf5bba9034efc`。

本文件记录只读环境审计，以及 Root 执行的当前版本 Linux 默认测试、双 release 链接、独立产物和资源回收证据。本 agent 没有启动或修改容器、运行 Cargo／测试／数据库、下载依赖、访问硬件，也没有修改生产代码。默认 shadow、当前政策、测试原期限和默认并发保持不变，R03 继续暂停。

## Checklist

- [x] 核对 2026-10-12、2026-10-10、2026-10-06 的实际命令和原始链接证据，区分历史结果与当前版本。
- [x] 确认宿主机 cache、registry、native 目录实际存在。
- [x] 比较当前与历史 `Cargo.lock`；核对所有 lock 中 registry crate 归档 checksum。
- [x] 重新计算 Linux Sherpa 归档 hash，并逐个核对已解压的 15 个静态库。
- [x] 核对默认 feature、原生依赖和双 release 的最小复用命令；标出真实证据缺口。
- [x] 独立读取本轮 `setup.json`、`native-inputs-before.json`、`source-preparation.json`，核对 exact image、任务隔离与冻结输入；宿主文件再验 677 个 source 和 15 个私有 native library 均无差异。
- [x] 核对本轮 environment stage exit 0、677 个输入无 drift、实际工具／系统库版本和 9 个系统动态库 SHA-256。
- [x] 核对本轮双 release、ELF／动态链接器／Sherpa 符号证据，重新 hash 两个独立产物副本；构建后的 15 个 native 库无 drift。
- [x] 核对 `cleanup.json complete=true`、全部 16 条资源闭环命令成功，任务两容器、PG volume 和 internal network 已回收，共享服务保持。

环境与链接审计项已完成。Root 的 Linux workspace 单次完整运行计 1285 passed／0 failed／5 既有 ignored／0 filtered；Server 与 Embedded 各自 no-default all-targets check、双默认 release 和链接审计全部 exit 0。具体当前产物身份、资源闭环及交付边界见下文；未扩展硬件、模型或其他平台验证。

## 可复用环境身份

历史 Linux holder 固定使用 `--platform linux/arm64 --pull=never` 与 image ID：

`sha256:6cd7a7a06976f8651cdb57fcde457e9dc7e2abe20587900dd750e1f373a25c70`

该值是本地内容 image ID；本轮 Root 的 exact inspect 已确认它存在，`Architecture=arm64`、`Os=linux`、`RepoDigests=[]`，因此没有 registry RepoDigest 可填写。本轮 `environment.log` 再次证明当前 holder 运行 Debian GNU/Linux 12 bookworm、`aarch64-unknown-linux-gnu`，Rust／Cargo 1.95.0、LLVM 22.1.2；实际宿主内核为 `7.2.9-orbstack-00391-g2b298d05fbd4`、aarch64。最初本 agent 的 Docker socket 读取受 sandbox 限制，后续通过只读核对 Root 保存的 `run/validation-20261017-linux-release/evidence/setup.json` 完成身份确认，没有重新调用 Docker。Root 的初始资源清单确认旧 holder／旧任务 PG 均未运行，未复用启动它们。

| 内容 | 已确认宿主机路径 | 本轮复用要求 |
| --- | --- | --- |
| Cargo home 与编译缓存 | `run/validation-20261008-layering/linux-arm64-v1/cache` | 已复制为本轮 `run/validation-20261017-linux-release/cache` 私有 APFS COW cache，挂 `/cache`；不继续写历史 cache |
| Cargo registry | `run/validation-20261010-l12/registry` | 本轮使用 `run/validation-20261017-linux-release/registry` 私有副本，挂 `/cache/cargo/registry`；不挂写历史原件 |
| Sherpa 本地归档 | `run/validation-20261008-layering/linux-arm64-v1/native` | 本轮副本 `run/validation-20261017-linux-release/native` 只读挂 `/native`，显式 `SHERPA_ONNX_ARCHIVE_DIR=/native` |
| 当前源码 | 当前仓库基线的冻结快照 | 本轮 `run/validation-20261017-linux-release/source` 的 677 个输入只读挂 `/workspace`，没有挂旧 source |
| Linux 原始证据 | `run/validation-20261012-settings-read-authorization/linux-arm64-v3/evidence` | Server 测试／no-default 参考，不包含该轮双 release |
| Server release 原件 | `run/validation-20261010-l12/linux-arm64-v4/evidence` | `server-release.*`、`server-binary.json` |
| 双 release 原件 | `run/validation-20261006-cleanup/linux-arm64-v3/evidence` | `release.*`、`binaries.json`、独立 no-device smoke 原件 |

PostgreSQL image ID 为 `sha256:306e00ca129081f4a69081c09ff0bdf0ef5169779019200a5c22ce9e0c6148f3`。本轮 exact inspect 确认 `linux/arm64`，RepoDigest 为 `pgvector/pgvector@sha256:feb68f4f15446397d8cac7f4fe48fe4586de83160d1fc48b46283312d1a33966`。Root 以 exact ID、`--pull=never` 新建任务 PG，预检实际运行 PostgreSQL 17.10、aarch64、GCC 12.2.0；不重新启动历史任务 PG。

## 本轮 setup 与输入身份已核实

独立读取的三份原件为 `run/validation-20261017-linux-release/evidence/{setup.json,native-inputs-before.json,source-preparation.json}`。`setup.json` 记录以下成功创建的精确资源，全部带任务 label `alia.validation=linux-release-20261017`：

| 资源 | 精确 ID | 隔离方式 |
| --- | --- | --- |
| network | `4d151571f476bd84449a796458a5a069e98cef385dd431d9a0dc455bad1be89e` | `--internal`；本轮专用 network |
| PostgreSQL container | `7f760321c3d3345db7db26ca411af88552a0f775e1191d0eaf993ea4aa710e51` | 仅连接本轮 internal network；没有 host port 发布参数 |
| Linux holder | `348b7462f152c6ba4ee06f752e7f4065a81cbe97626fa7b176cfd96ab33e1005` | 共享本轮 PG container network；没有 host port 发布参数；exact Linux image ID |

源码 preparation 记录 677 个输入，仅对 590 个内容逐字节相同的输入保留旧 mtime。新 source manifest SHA-256 为 `c239c76799e4f70096c2c7e5898e396534beb2a762fb7628203c671d2941c5bf`；本 agent 在宿主上读取冻结 snapshot 并重新计算全部 677 个文件 hash，与 manifest 差异为 0。挂载命令对 source、native 明确设 `readonly`，cache、registry、runtime、evidence 均指向本轮目录。

本 agent 又读取本轮 private archive 和 private cache 的 15 个静态库，分别与 `native-inputs-before.json` 比较 byte size／SHA-256，差异为 0；新 archive 仍为 19167687 bytes 与下节固定 hash。环境准备和最终执行分别留证；最终 release、native after 和 cleanup 原件见后文。

## 依赖与原生归档的本轮只读核对

当前 `Cargo.lock`、Oct12 v3 source、Oct10 v4 source 三份文件均为 138277 bytes，SHA-256 均为：

`845d709734b382d24baaec28bd9d69a524e92c4b41366184de02f0ef7a322dfc`

使用 Python 标准库读取 lock 和宿主 registry，对 544 个 registry package 的 `.crate` 逐一计算 SHA-256：缺失 0，checksum 不匹配 0；没有 git 等其他外部 source。部分跨平台和可选 crate 没有解压 source 目录，但其归档齐全。最初的只读挂载建议不能保证任意 feature 的首次解压；本轮实际 setup 已使用可写的任务私有 registry 副本，消除修改历史原件或只读解压的问题。仍不能下载、修改历史 registry 或偷偷改变 feature。

Linux Sherpa 原生归档实际存在：

- 文件：`sherpa-onnx-v1.13.1-linux-aarch64-static-lib.tar.bz2`。
- 大小：19167687 bytes。
- 本轮重新计算 SHA-256：`86d0d3818770d4f6a087e52691d3ae171393972030f133df064a271fe9fe1749`，与 Oct12 v3 `native-archive.json` 一致。
- 本轮读取归档并核对 `cache/target/sherpa-onnx-prebuilt/sherpa-onnx-v1.13.1-linux-aarch64-static-lib`：15 个归档文件全部为 `.a`，15 个已解压文件逐字节 hash 一致，缺失 0、差异 0。
- 代表性静态库：`libonnxruntime.a`（93676618 bytes，SHA-256 `3db52f251125ef4f01ee9ed62fdf59be9adb94142cc2a3f76d7f6fbb26282438`）；`libsherpa-onnx-core.a`（15873494 bytes，SHA-256 `4c20fc0948f0fe19b7b40b82bbdf1d37a6691d047be2969d8290aed1853eb566`）；`libsherpa-onnx-c-api.a`（495144 bytes，SHA-256 `b3f635ef534661c3d28975e14aa3c1889643eb866e4b700035e80cdf565627fb`）。

`sherpa-onnx-sys-1.13.1/build.rs` 的实际行为：先使用已解压 `target/sherpa-onnx-prebuilt`；不存在时才使用本地归档。指定 `SHERPA_ONNX_ARCHIVE_DIR` 后，归档缺失会报错而不会走网络下载。`DOCS_RS` 只要被设置便跳过原生下载和链接，所以验证必须保持 unset；不能以该变量绕过真实 native link。

## 系统库与 release 链接边界

本轮 `run/validation-20261017-linux-release/evidence/environment.{log,result.json}` 已完成：exit 0、0.536 秒、前后 677 个 source 输入无 drift，使用上述 exact holder ID。实际 `RUST_TEST_THREADS`、`RUST_MIN_STACK`、`DOCS_RS`、`SHERPA_ONNX_LIB_DIR` 均 unset，`SHERPA_ONNX_ARCHIVE_DIR=/native`。Root 运行的工具版本与 Oct10 v4／Oct06 v3 历史证据一致：

| 项目 | 本轮实际版本 |
| --- | --- |
| protoc | 3.21.12 |
| ALSA pkg-config | 1.2.8 |
| D-Bus pkg-config | 1.14.10 |
| OpenSSL pkg-config | 3.0.20 |
| glibc | 2.36，Debian `2.36-9+deb12u14` |

`dpkg-query` 进一步记录 `libasound2-dev:arm64=1.2.8-1+b1`、`libdbus-1-dev:arm64=1.14.10-1~deb12u1`、`libssl-dev:arm64=3.0.20-1~deb12u1`、`libc6:arm64=2.36-9+deb12u14`、`libstdc++6:arm64=12.2.0-14+deb12u1`、`protobuf-compiler=3.21.12-3+deb12u1`。

历史 `file` 均为 ARM aarch64 的 ELF 64-bit PIE、动态链接、解释器 `/lib/ld-linux-aarch64.so.1`。Server 历史 `ldd` 完整解析 OpenSSL 3 的 `libssl.so.3`、`libcrypto.so.3` 及 libc／libm／libgcc。Embedded 历史 `ldd` 完整解析 `libstdc++.so.6`、`libdbus-1.so.3`、`libasound.so.2` 和 libc／libm／libgcc，D-Bus 的 systemd、cap、gcrypt、lzma、zstd、lz4、gpg-error 传递库也有解析记录。当前双 release 已在本轮 `release-audit.json` 再次完成独立 `file`、`readelf`、`ldd` 和 loader verify／list，结果均成功。本轮环境阶段也补上原先历史材料缺少的系统 `.so` SHA-256，以下均来自当前环境原件，真实解析路径位于 `/usr/lib/aarch64-linux-gnu/`：

| 系统库实际文件 | bytes | SHA-256 |
| --- | --- | --- |
| `libasound.so.2.0.0` | 1116648 | `9fade6b3aedf1ba944e6795b52b74378f2f89166b92ee542582171416a76572d` |
| `libdbus-1.so.3.32.4` | 395336 | `e35b314463bc7d7e48d542b13941c49172abb2ba8e8fe5b1f54e6a50b636bdfc` |
| `libssl.so.3` | 737192 | `9e171264a6651d714938e83d81277d16524c150a36eaa59ce56e79b9cdbea7ce` |
| `libcrypto.so.3` | 4532392 | `dfbbbff72234e55b7bb2b8ae57a1fbed326d49d4b56882e3d4a046399f93cacf` |
| `libc.so.6` | 1651408 | `e4ac8ae1d81e4865e3aadedb962879cf9415903b3f2ba81ec75e9962b86ab8b0` |
| `libstdc++.so.6.0.30` | 2174296 | `532f3a12d7b8fef6cc7a064ea106b6c4ab2365674b73e743e4a385d4ffb6c8ab` |
| `libgcc_s.so.1` | 133448 | `046856f95f4636f1fc7c3a12bf4f3cd5634c2fc5145c3fdf7395d4f349fa69c7` |
| `libm.so.6` | 591960 | `3c4cb3be0b974edf05f023f85ab15107fb5afc2687163593d0d4cf8e80c17b39` |
| `ld-linux-aarch64.so.1` | 202912 | `17538b8f9889a470c061f69a8fea8124da89627311cd16546c133a89f09056df` |

当前默认配置中 Embedded 的 Sherpa 是默认 native feature；Linux 专属 ALSA 和 `bluer`／D-Bus 依赖仍存在。`vision-opencv` 是可选 feature，不属于历史 default 或本轮既定范围。Server `stt-whisper` 是 HTTP provider feature，并不要求运行 `scripts/setup-whisper-cpp-linux.sh`；该脚本会安装／下载／启动外部模型服务，不应作为本轮编译前置。Easy-Turn 和真实音频模型不因软件链接验证而启动。

本轮准备开始时，只读核对到历史 cache 中两份 release binary 为：

| binary | 构建前历史 cache 大小／SHA-256 | 对应历史证据 |
| --- | --- | --- |
| `alia-server` | 45473304 bytes；`adb24d2348ead3e03629121c5bbf68f63b0ef70a15ab20b08215fb37b58ee23a` | Oct10 v4 `server-binary.json` |
| `alia-embedded` | 49598528 bytes；`ad37a321ff3c7398e1c17187aa2d8746571e64595a1ab0809ee1a3cd35318e37` | Oct06 v3 `binaries.json` |

本轮从新 snapshot 完成了双 release，后文的新产物 hash 与这两份历史 binary 均不同。历史原件和 cache 保留，本轮写入任务私有 cache。

## 当前执行、交付产物与资源闭环

下列结果均来自 `run/validation-20261017-linux-release/evidence/`，共用 677 输入 manifest `c239c76799e4f70096c2c7e5898e396534beb2a762fb7628203c671d2941c5bf`，全部阶段前后 source drift 为 `[]`：

| 原件 | 结果 | elapsed_seconds |
| --- | --- | --- |
| `workspace-default.result.json`／`workspace-counts.json` | `cargo test --workspace --offline --locked` exit 0；1285 passed／0 failed／5 ignored／0 filtered | 483.737 |
| `server-no-default.result.json` | Server `--no-default-features --all-targets` check exit 0 | 21.055 |
| `embedded-no-default.result.json` | Embedded `--no-default-features --all-targets` check exit 0 | 6.163 |
| `release-default.result.json` | Server＋Embedded 默认 `--release --offline --locked` build exit 0 | 115.452 |
| `release-link-audit.result.json` | 当前双 binary 链接与静态 native 审计 exit 0 | 0.405 |

workspace 计数来自 72 个 Cargo target 边界，未重复计算 3 个嵌套子测试摘要；本 agent 核对原日志 SHA-256 与计数原件一致。5 个 ignored 是既有 3 个真实 PTY 和 2 个真实 Easy-Turn 服务用例，本轮未新增 ignore，也没有把它们算作通过。

`release-audit.json` 的 `passed=true`，两个 binary 均为 AArch64 ELF64 PIE、动态解释器 `/lib/ld-linux-aarch64.so.1`。Server 解析 6 个 runtime 文件（含 loader），Embedded 解析 14 个 runtime 文件（含 loader），均没有 `not found`；实际解析文件各自另有 SHA-256 与 byte size。Embedded 有 170 个已定义 `SherpaOnnx` 符号，包含 KeywordSpotter 和 VoiceActivityDetector，证明 native 实现已进入最终 binary。Root 只执行了 Embedded 已审计的安全 `--help` 入口并 exit 0，没有启动设备、模型或服务；Server 执行的是 ELF／loader 检查，没有把 loader 成功写成服务启动验收。

`retained-artifacts.json` 记录两个独立产物副本。本 agent 在宿主重新计算 byte size／SHA-256，与 release audit、retained manifest 三方一致，并通过文件身份比较确认它们与 cache target 不是同一文件：

| 本轮独立产物 | bytes | SHA-256 |
| --- | --- | --- |
| `run/validation-20261017-linux-release/artifacts/alia-server` | 46092832 | `3528c5e02708e126d560cdaade8a431c0ed3e60de6c86892f931dcb0250de330` |
| `run/validation-20261017-linux-release/artifacts/alia-embedded` | 49540200 | `159becdbd3783b77ad308d02096ed39158fc54cfef43f648091a2c0f015f4003` |

`native-inputs-after.json` 再次记录固定 Sherpa 归档 hash、`static_libraries_checked=15`、`errors=[]`。`after-workspace-resources.json` 在 workspace 结束约 11.165 秒后记录同一任务 PG 的 schemas=0、other_clients=0；`cleanup.json` 清理前再次确认 0／0，随后记录 `complete=true`，16 条命令均 exit 0。前述 exact Linux holder、PG container、internal network 均删除，PG volume `5ae0c0635f76f72494608eccdc7186618488c69a87bc2076ce4016c235622502` 已从实际 volume list 消失；最后仍运行的只有既有共享 `astrbot`，没有扩大清理范围。

本轮交付是既定 Linux ARM64 容器环境的默认 workspace 软件验证、两端 no-default 编译边界、双 release 原生链接与可追溯产物。它不覆盖真实 PTY／Easy-Turn 的本轮执行、完整 release 服务启动 smoke、真实音频／执行器、Pi 部署、其他架构或 `vision-opencv` feature；R03 仍暂停。历史 V01 偶发超时原始原因也不能因为本轮通过而宣布关闭。

## Root 最小复用流程

1. 使用 exact image ID 与 `--pull=never --platform linux/arm64`，新建任务专属 holder／PG／证据目录；不更改既有容器。将当前源 manifest 冻结、只读挂到 `/workspace`，native 只读挂载，cache 使用任务私有副本。
2. 沿用 `CARGO_HOME=/cache/cargo`、`CARGO_TARGET_DIR=/cache/target`、`CARGO_BUILD_JOBS=2`、`SHERPA_ONNX_ARCHIVE_DIR=/native`。构建 jobs=2 是历史编译限制，不是测试线程限制；`RUST_TEST_THREADS`、`RUST_MIN_STACK`、`DOCS_RS` 保持 unset，保留测试内原期限及原 CASES。
3. 先记录当前 holder 的 OS／arch／rustc／Cargo／protoc／pkg-config／glibc；需要系统库文件 hash 时由本轮实际环境采集，沿用已安装系统库，不 apt／下载。
4. Root 按本轮验收计划运行当前源码的 Linux build／tests，所有 Cargo 命令加 `--locked --offline`。历史 Oct12 `cargo test -p alia-server --offline --locked` 仅是命令参考，不代替本轮 workspace 范围决定。
5. 双 release 链接复用已通过的命令：`cargo build -p alia-server -p alia-embedded --release --locked --offline`。默认 feature 与 no-default 是两项证据；no-default 参考 `cargo check -p alia-server -p alia-embedded --no-default-features --all-targets --locked --offline`，不能代替默认 native release。
6. 最后记录两个当前 binary 的 byte size／SHA-256／ELF 架构／`ldd`，核对源码无 drift、数据库 schema／client、子进程与 holder 资源。所有结果放本轮新文件，不覆盖旧 evidence。Oct06 no-device smoke 只作为后续软件启动验收素材，不能未经核对直接执行旧脚本、旧 schema 或旧端口／配置。

本轮环境与链接审计未发现需要新增依赖或修改生产源码的确认缺口；当前 Linux 执行、新 binary、native after 与资源闭环证据已补齐。后续如扩展真实 PTY、模型服务或 release 服务启动验收，应沿用本轮产物身份和原有批准边界单独留证，不把本轮容器验证扩大为硬件或部署验收。
