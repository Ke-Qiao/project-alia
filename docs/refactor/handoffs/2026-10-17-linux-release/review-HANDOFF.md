# 当前版本 Linux ARM64 — 独立验收审查

2026-10-17；基线 `da3eca2fe6853f104dd558051aebf92dd62f067b`，应用源码 `4e2968f18d765f326e76d24a3bcdf5bba9034efc`。当前 **5/5，最终 GO，无阻断**：当前版本 Linux ARM64 构建、软件测试和两端 release 链接证据已独立核对。本轮没有生产修改；恢复材料的正常文档提交与提交后 HEAD 核对由 Root 完成。审查仅阅读源码、日志和脚本，计算文件及 Git blob 身份，未执行 Cargo、数据库、容器或硬件操作，未修改源码。

## Checklist

- [x] 区分既有 Linux 测试、release 链接及实际进程运行的版本与范围。
- [x] 审查源冻结、native 输入、退出码、期限、计数及数据库证据的风险。
- [x] 给出本轮有限完整门槛，保持既有 Linux ARM64 平台和资源边界。
- [x] 独立核对本轮构建／测试／release 原件、ELF／链接身份和源输入无漂移。
- [x] 核对失败保留、最终资源回收、基线 Git blob 与归档哈希，收口最终意见。

## 既有证据的身份

| 证据 | 已验证版本和范围 | 不能替代的内容 |
|---|---|---|
| [Settings GET 验证包](../2026-10-12-settings-read-authorization/README.md) | `d080710052ae308b962b826d0f096fb10067469e`；655 项输入；Linux ARM64 Server 默认完整测试 852/0/5、Server 无默认 all-targets 检查 | 后续源码、当前 Embedded、当前 release 和部署运行 |
| [L12 验证包](../2026-10-10-l12/README.md) | `d9f97216a6599742c45e0a6389110b6b465e8461`；641 项输入；Server 默认 release 实际链接。记录的 Server ELF SHA-256 为 `adb24d2348ead3e03629121c5bbf68f63b0ef70a15ab20b08215fb37b58ee23a` | 当前源码的 Server／Embedded 二进制；Server 的 ldd 不证明 Embedded Sherpa 链接 |
| [10 月 6 日验证包](../2026-10-06/README.md) | `4434be4`；619 项输入；Server 和 Embedded 默认 release／链接及独立进程启停、重启 | 当前提交的部署进程验收；容器结果也不是 Pi 外设验收 |

这些历史记录可复用方法与既有离线环境，不能借用旧产物或通过数字给当前版本背书。当前 macOS 1277/0/5 也不是本轮 Linux 结果，平台条件编译导致测试数不一定相同。

## 已确认的脚本与证据风险

1. **数据库缺失可能出现表面通过。** 大部分当前 PostgreSQL fixture 使用 `expect`，但 `packages/alia-server/tests/plugin_storage.rs` 的 `postgres_plugin_storage_round_trips_records_and_policy` 在缺少 `ALIA_TEST_DATABASE_URL` 时打印 skipping 并直接返回，libtest 会显示 ok。本轮无需扩展生产修改；应记录同一测试进程的任务数据库环境注入，并在隔离 PG 上做成功连接／数据库身份预检。单独的总通过数不能证明这个用例实际经过数据库。
2. **已解包的 Sherpa 缓存先于 tar 被采用。** 本地 `sherpa-onnx-sys 1.13.1/build.rs` 在 `target/sherpa-onnx-prebuilt/<archive>/lib` 存在时直接返回；`SHERPA_ONNX_LIB_DIR` 也优先于 archive 路径。只重算 `/native` 中官方 tar 的 SHA-256，无法证明实际链接的缓存 `.a` 相同。应核对实际使用的 13 个静态库与同一已验证 tar 成员的内容，或使用本轮从该 tar 得到的受控库目录，并记录其真实路径与哈希。既有 tar 为 19,167,687 字节，SHA-256 `86d0d3818770d4f6a087e52691d3ae171393972030f133df064a271fe9fe1749`。保持 `DOCS_RS` 未设置，明确排除意外 native 覆盖，不重新下载或更换库。
3. **旧阶段脚本的内外期限不同。** 10 月 12 日 `run-stage.py` 正确保留 docker exec／容器内 timeout 返回码，并在前后核对源输入；内层为原 1800 秒、TERM 后 10 秒 KILL。但宿主 `subprocess.run(docker exec ...)` 没有超时，daemon／exec 通道异常可能不返回，异常路径也可能没有最终 result。没有 result、非零退出、watchdog 触发、进程仍在运行或仅有部分 ok 都不能标通过。若增加宿主管理上限，超限必须记录未完成，随后明确回收任务进程／容器；不能只杀客户端就宣称容器内工作结束，不能放宽原测试期限。
4. **默认 native 特性不能由 check 或库文件存在推导。** `cargo check` 不产生最终可运行链接产物。Server 无 Sherpa 默认依赖；Embedded 当前默认 `sherpa-onnx`。真实 release 证据需绑定构建成功、准确产物路径／hash、ELF aarch64 与解释器、动态依赖无缺失，以及 Embedded 实际包含预期 Sherpa 定义符号。`ldd` 只能解释动态库，不能单独证明静态 Sherpa 已链入。旧目录中已有 ELF 也不能替代本轮命令产生的产物身份。
5. **测试汇总不能叠加子进程或失败轮次。** 既有 `count_cargo_tests.py` 能按 Cargo Running／Doc-tests 边界只取外层最终汇总，校验起始 selected 数，并要求包含结束时间与退出码的 sidecar；缺失末尾、解析歧义、非零状态或源漂移必须保留为失败／未完成。只选若干 ok 行、将内部重入 summary 累加、把旧失败后的定向通过加进最终数字，都不成立。原始失败必须保留，后续修改按新 manifest 独立验证；不降低默认测试并发，不延长个案期限，不新加 ignore。

## 本轮最小完整门槛

1. **同源与既有环境。** 冻结当前源码和 Cargo.lock，保留逐文件哈希与 Git blob 关系；复制成只读源码目录，阶段前后及最终检查无漂移。记录现有 Linux ARM64 镜像 ID、rustc／目标架构、原系统库版本、Cargo feature 选择、native 实际输入与缓存使用。复用依赖缓存可以，但必须能把当前项目的构建输出追溯到本轮只读快照；不得只检查旧二进制存在。默认 feature 不改成 no-default 或 all-features 以绕过链接。
2. **一次有界软件测试矩阵。** 在本轮独立 PostgreSQL／pgvector 环境完成约定的默认测试范围；若要称“当前 workspace Linux 全量”，需一次完整 `cargo test --workspace --offline --locked`，否则明确仅已执行包／目标。前后保存数据库身份、测试 schema 与连接观测，维持真实 DB 用例；五项既有真实 PTY／Easy-Turn ignore 单列，不当作通过。避免相同输入的无理由重复批次。
3. **两端构建与最终链接。** 同源 Server／Embedded 的 no-default all-targets 检查仅作为 feature 边界证据；另行默认 feature `cargo build -p alia-server -p alia-embedded --release --offline --locked`。保存成功退出、产物哈希／大小、ELF／动态依赖和 Embedded native symbol 证据；这些不自动等同实际服务启动、配对、模型或音频验收。
4. **失败与资源闭环。** 每阶段保存准确 argv、开始／结束、退出／超限和源码身份。失败先保留现场与原件，再按已确认问题决定有界下一步。关闭前观测任务数据库、相关进程和容器；回收按完整 ID＋任务 label 复核，保留共享 astrbot／其他用户资源。结束后快照须标明观测时间，不声称与测试结束原子同步；主动兜底删除的 schema 也不得伪称自然清理为零。
5. **有限交付边界。** 归档原始日志、结果、计数、输入清单、native／二进制身份及回收证明，逐份 hash 可核对。本轮是当前 Linux ARM64 软件构建／测试／链接出口，不扩大至 x86_64、Windows、Pi 真实录放音／执行器、付费模型或长期运行。R03 保持暂停，default shadow、当前政策和权限边界不变；历史 V01 根因是否关闭须另有证据，不能因本轮通过自动关闭。

以上是执行前确定的审查门槛和已知脚本局限，不是当前源码新增生产缺陷的断言。以下记录准备阶段静审及最终独立核对；审查者没有自行启动验证。

## 本轮准备与 runner 静审

已只读核对 `run/validation-20261017-linux-release/evidence/setup.json`：复用既有 Linux ARM64 镜像 `sha256:6cd7a7a06976f8651cdb57fcde457e9dc7e2abe20587900dd750e1f373a25c70`，PG 镜像 `sha256:306e00ca129081f4a69081c09ff0bdf0ef5169779019200a5c22ce9e0c6148f3`；任务使用 internal network、独立 PG、只读源码和 native 挂载，构建写入任务私有缓存克隆。实际 SQL 预检返回任务数据库 `alia_linux_release_20261017`、任务用户和 PostgreSQL 17.10 aarch64 身份。记录的 Linux／PG／网络均有完整 ID，未挂载音频或执行器设备。

已独立重读本轮 tar，并与私有缓存 **15 个 `.a`** 逐字节比对，同时核对 `native-inputs-before.json` 中每项 SHA-256／大小，全部相符。15 个成员包含 build script 明确链接的 13 个静态库；不把 archive 存在当作实际缓存身份。

新 `run-stage.py` 静审 **GO**：原容器内 1800 秒、TERM 后 10 秒 KILL 保留；1850 秒宿主上界触发时明确记录失败，按已保存完整任务容器 ID 尝试终止，再排空宿主 exec 进程。没有管道吞退出码；命令异常、源码漂移与清单核验失败均使脚本非零退出，阶段文件已存在则拒绝覆盖。`finally` 保存结果，即使最终只有失败／未完成也保留其身份。宿主兜底若 unresolved，或遭外部 KeyboardInterrupt／强制终止，仍须最后核对真实任务进程和容器，不能仅凭客户端退出称已回收。此段是执行前的静态意见，审查未执行该脚本。

`evidence/release-audit.py` 静审 **GO**。脚本核对两端 release 的实际 ELF／AArch64／解释器、无 RPATH／RUNPATH、ldd 无缺库、动态 loader verify／list、解析到的系统库哈希，以及 Embedded 的已定义 Sherpa C API 符号。与两份旧缓存 hash 不同只排除已知旧产物，必须与本轮默认 release 成功和同源阶段证据联合解释；不能单凭 hash 不同证明由本轮源码构建。脚本只在全部检查后写 `passed=true`，异常由外层 runner 记录非零。

已读当前两端 main：Embedded `--help` 在创建 Tokio runtime、读配置／身份、调用 EmbeddedBuilder 之前直接返回；Server 没有对应分支，脚本不启动 Server。Embedded 帮助输出和动态 loader 验证不是服务启停、模型、声卡或执行器验收。`/runtime` 空目录断言只覆盖该目录，不泛称全文件系统零副作用。审查未执行 release 审计命令。

## 最终动态证据独立核对

本轮冻结 manifest SHA-256 为 `c239c76799e4f70096c2c7e5898e396534beb2a762fb7628203c671d2941c5bf`。已独立读取基线 `da3eca2` 的 **677 个对应 Git blob**，逐项内容与 manifest 相符；当前工作区和只读快照逐项无漂移。与上一轮 macOS 最终 677 条文件记录完整相等，清单文件自身摘要不同来自清单元数据，不能误认为源码变化。

独立从 `workspace-default.log` 按 Cargo 目标分段重算为 **1285 passed / 0 failed / 5 ignored / 0 measured / 0 filtered**，共 **67 Running＋5 Doc-tests**。三条内部子进程重入摘要未重复累计。`server_runtime` 为 85/0/0，`duplex_playback` 为 31/0/0；五项忽略名称和理由与上一轮完全相同，仍是三个真实 PTY、两个真实 Easy-Turn 条件用例。`plugin_storage` 的实际 PostgreSQL 用例日志为 ok；runner 明确注入已预检的任务数据库，缺环境时的静默跳过分支不适用。原始 workspace 日志 SHA-256 为 `20a7b25d3f468756b81d909213bc77dfd851c02c3bbc176da504b389bc8ec211`。

另按原始日志逐用例名称交叉比较 Mac／Linux：没有仅在 Mac 出现而 Linux 缺失的名称；Linux 独有八项，正是七项 ALSA backend 软件测试和一项 PCM 名称测试。该差异不是新增生产或测试改动，也不是实际声卡证据。

已核对六个阶段结果：environment、workspace default、Server no-default all-targets、Embedded no-default all-targets、双端默认 release、release link audit 全部 exit 0，输入摘要一致，前后源码漂移为空，无宿主 timeout／runner error。workspace 命令 483.737 秒；Server no-default 21.055 秒、Embedded no-default 6.163 秒；默认双端 release 115.452 秒。release 原始日志明确重新编译本轮工作区的 proto、core、transport、Server 和 Embedded 后完成 optimized profile，不是将 cargo check 或旧文件存在当作链接。

| 最终保留产物 | 字节数 | 独立重算 SHA-256 |
|---|---:|---|
| `alia-server` | 46,092,832 | `3528c5e02708e126d560cdaade8a431c0ed3e60de6c86892f931dcb0250de330` |
| `alia-embedded` | 49,540,200 | `159becdbd3783b77ad308d02096ed39158fc54cfef43f648091a2c0f015f4003` |

两份产物重算 hash／大小与 release audit 和 retained-artifacts 一致，且不同于记录的旧缓存。另独立读取文件头确认为 ELF64 little-endian、`e_machine=183`（AArch64）。实际审计原件包含解释器 `/lib/ld-linux-aarch64.so.1`、ldd 无缺库、loader verify／list 全部成功，以及 Embedded **170 个已定义 Sherpa C API 符号**，包括创建 KeywordSpotter 和 DecodeKeywordStream。系统库解析与 hash 已记录；Server 没有执行 main，Embedded 仅走已静审的帮助分支。

`native-inputs-after.json` 记录 15 库无差异。审查又独立重算任务私有缓存中全部 15 个 `.a` 及原 tar：均与执行前逐成员记录相符，原 tar SHA-256 保持 `86d0d3818770d4f6a087e52691d3ae171393972030f133df064a271fe9fe1749`。不依赖 tar 存在或 Cargo 缓存命中来代替实际 native 输入核验。

## 资源与归档闭环

workspace result 结束时间为 `2026-10-16T16:42:58.317576Z`；原任务 PG 在 `16:43:09.482802Z` 观测为 **0 schema／0 other clients**，独立时间差 **11.165226 秒**，不是结束当刻快照。最终回收前再次查询仍为 0/0；Linux 容器 `docker top` 仅剩 holder 的 `sleep infinity`，无 Cargo／测试遗留。

`cleanup.json` 的每个命令均 exit 0，先核对 Linux、PG 和 internal network 的完整 ID＋任务 label，再停止并删除两个任务容器及卷，删除网络，并查询确认容器／网络不存在。记录的 PG 匿名卷 `5ae0c0635f76f72494608eccdc7186618488c69a87bc2076ce4016c235622502` 不在最终卷列表。共享 astrbot `c5aaca070870` 保持运行；其他旧容器未作为本轮清理目标。无需通过删除遗留测试 schema 来把失败包装成自然清理。

已独立核对 [evidence-index.json](evidence-index.json) 的 **50 个归档文件，共 721,154 字节**：大小和 SHA-256 全匹配，并与原 `run/validation-20261017-linux-release/` 对应文件逐字节相同。该包按原 run 层级归档，因此结果原件位于 `evidence/evidence/`，脚本等位于 `evidence/`；没有因路径层级误把不同文件作同一证据。

最终结论仅为本轮已有 Debian 12 Linux ARM64 环境的当前软件测试、构建与链接 **GO**。没有新增生产修改、依赖、平台、真实模型／音频／执行器；不宣称普通独立服务进程启停、安装配对、Pi 运行或 R05 整机验收已经完成。历史 V01 仍 open，R03 仍暂停，default shadow 和当前政策保持。

已交叉审阅 Root／README、checklist／恢复检查点及 progress／platform／next-round 最新段落：计数、版本和上述边界一致，未发现越界结论。checklist 的提交项由 Root 在正常提交后收口；独立审查停止写入。
