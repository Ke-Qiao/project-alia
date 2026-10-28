# Server 与 Embedded 无前端操作入口

更新：2026-10-28。部署目标为 Mac Server、Pi 5 / 8 GB Embedded，Easy-Turn 位于 Server。Pi 的 Ubuntu 24.04.4 / aarch64 和 R1 分阶段采播基线已有记录；本轮复用这些结果。模型方案、有限组件和配置／SQL 隔离演练已完成；本机既有数据与身份、Mac 服务材料见下节，实际生产启停／升级恢复仍待授权窗口。以下指向当前 CLI/REST；Web/iOS/Android 不参与本轮安装或验收。

当前硬件/部署输入、只读核对与R02/R03→R05门槛集中在 [部署资料清单](hardware-deployment-readiness.md)。

当前软件产物／平台证据和真实模型端点／版本／部署待确认项见[软件发布记录](software-release.md)。Linux noop通过不代表当前Mac部署或真实模型链已验收。

Harness／Embodiment 版本切换使用[成套升级与失败恢复步骤](upgrade-recovery.md)：双端协议、数据库迁移、插件 grant 和 Appraisal pin 必须一起核对。

## 当前部署决定（2026-10-28）

用户决定**不以旧库或备份作为新生产数据来源，删除两套旧 PostgreSQL**：`alia-postgres`＋`project-alia_pgdata`、`alia-python-postgres`＋`alia_python_postgres-data`。此前“保留旧卷并查询停服副本”的建议已撤回；下方 10 月 25 日的元信息与只读证据仍作为历史保留。新库后续按全新部署准备，不继承旧账号、embedding pin 或 Companion 政策，也不把演练 fixture 账号当成生产 owner。首次初始化后须实际核对 schema、选定的 1024 维 profile 与 shadow 政策；空库没有历史记忆需要重建。

**两套旧容器和专属数据卷已删除并核验不存在。** 用户明确授权临时启动 OrbStack 后，核对完整容器 ID、停止状态和数据卷独占关系，使用非强制删除；其他 53 个容器、48 个数据卷及网络清单保持不变，初始化脚本源目录保留。未新建备份或生产库，配置、密钥、模型和历史证据未纳入删除。引擎启动期间 `astrbot` 按既有 `always` 策略自动运行，随后已随 OrbStack 恢复停止；未改变该策略。操作、删除后检查和 `Stopped` 状态见[完成记录](../../run/deployment-decisions-20261028/deletion-approved/result.json)。此前启动审批阻断及首次过窄挂载断言保留为历史；后者发生在任何删除前，按实际只读 init bind／独占数据卷核对后继续。

首轮运行域采用 **`ke-qiao` 登录后的 macOS 用户会话**。minor 已改为官方 `deepseek/deepseek-flash`＋none，独立输出预算保留；[新配置](../../run/fresh-start-20261028/config/server.production.pending.toml)与[新服务材料](../../run/fresh-start-20261028/services/README.md)已同步，旧 Instant 驻留不再是就绪条件。只保留 local embedding 的精确实例与 context8192 要求，不自动改共享 JIT。保留 `alia-main` 密钥、major、embedding、Easy-Turn 与 Juno；共享隐私／登录设置和 LaunchAgent 尚未应用。macOS 登录不自动创建 Alia 管理员。

新生产 PostgreSQL 使用[独立 manifest](../../run/fresh-start-20261028/postgres-production/compose.yaml)：固定已验 ARM64 image 内容 ID、仅 loopback5432、新 external 卷 `alia-production-pgdata`、同一私有密码文件、首次 restart=no 和 Docker 日志轮转。manifest 已解析并静态核对，卷／密码／生产数据库均未创建；不使用含开发密码及宽监听的旧 compose 直接开生产。完整首次激活步骤见[数据库准备](../../run/fresh-start-20261028/postgres-production/README.md)。

启动材料补齐了新库凭据连接：Server gate 从与 Compose 相同的私有 `postgres-password` 文件读取，复用 owner／权限／非symlink检查，按镜像 FILE 的尾LF语义处理并URL转义，仅向子进程注入固定loopback数据库DSN；清除外部继承值，不打印原值。最终13项 gate／特殊字符／权限／exec拦截检查通过，3份plist、15项ready fixture通过。另以6份合成日志验证系统newsyslog停服轮转，固定保留5个归档；首次count配置错误及修正保留。这不构成在线日志硬上界，也未安装任务或创建生产凭据。

[隔离新库验证](../../run/fresh-start-20261028/database/summary.json)已使用冻结 Mac 发布二进制完成首次启动、HTTP 注册管理员／普通用户、默认 shadow、选定 embedding 双 pin 与同身份重启。隔离账号／key 全部为临时测试身份，清理后不可迁作生产；生产本人首位管理员须在首次受控 loopback 窗口登记，密码在本机设置，不发入对话。真实 Pi 的 owner／设备 pin／网络仍待现场绑定。本轮只在隔离环境运行 Server，未创建生产服务或启用音频。

## 本机数据与常驻准备盘点（2026-10-25）

本次只读盘点，没有启动数据库、加载模型、安装服务、接触 Pi 或音频；没有新建或覆盖生产数据。[盘点与材料汇总](../../run/mac-production-readiness-20261025/summary.json)保留容器／数据元信息、身份公钥指纹、服务状态、检查结果与原文件摘要。它补充前轮的合成 SQL 演练，不能当作已读取真实数据库。

### PostgreSQL 实际位置

在 PATH、Homebrew 已安装目录、Postgres.app 和已查 launchd 目录中未发现 native PostgreSQL；实际已有两套 **OrbStack Docker、Linux ARM64、PostgreSQL 17.10**。这不是全磁盘不存在其他安装的证明。

| 项目 | 当前 Rust 项目关联数据 | 历史 Python 数据 |
| --- | --- | --- |
| 既有容器 | `alia-postgres`，ID 前缀 `905470a16802` | `alia-python-postgres`，ID 前缀 `7365984e79e5` |
| 数据卷 | `project-alia_pgdata`，约 179.7 MB | `alia_python_postgres-data`，约 49.72 MB |
| 已创建容器的宿主端口 | **5433 → 5432** | **5432 → 5432** |
| 最近停止 | 2026-10-14，正常 shutdown／exit 0 | 2026-07-02，正常 shutdown／exit 0 |
| 容器 seed user／database | `alia`／`alia` | `alia`／`alia` |

两容器均停止，5432／5433 当前拒绝连接；只读取得 `PG_VERSION=17`、`global/pg_control` 等元信息，复核控制文件摘要未变。seed 值不证明当前 SQL 账号／密码；现有配置凭据与两套 seed 都匹配，不能据此决定数据归属。image 固定为 `sha256:306e00ca129081f4a69081c09ff0bdf0ef5169779019200a5c22ce9e0c6148f3`，详见 [PG 盘点](../../run/mac-production-readiness-20261025/postgres/summary.json)。

当前 Rust 卷在引擎内的路径为 `/var/lib/docker/volumes/project-alia_pgdata/_data`，Mac 可从 `/Users/ke-qiao/OrbStack/docker/volumes/project-alia_pgdata` 查看；底层 VM 映像位于 `/Users/ke-qiao/Library/Group Containers/HUAQ24HBR6.dev.orbstack/data/data.img.raw`。这是存储定位，不代表可用 macOS 原生 postgres 直接打开，也不能把复制运行中的 VM 映像当作一致备份。

**存在三处端口漂移：**旧 Server base／settings 指向 localhost:5432，当前 `docker-compose.yml` 也声明 5432，但已创建 Rust 容器实际是 5433，历史 Python 容器占用的映射才是 5432。不要直接 `compose up` 重建或新建空库来解决连不上。两容器当前 HostIp 为空、数据库监听 `*`；直接启动会恢复其原发布范围，不能假定只监听回环。

建议下一轮先以 **`project-alia_pgdata` 的停服隔离副本**为首选核对对象，原卷、原容器和 Python 历史卷均保留；明确授权后再准备一致副本，仅在回环／隔离网络启动副本并执行[只读目录查询](../../run/mac-production-readiness-20261025/postgres/01-readonly-catalog.sql)。确认表结构后才执行[账号／政策查询](../../run/mac-production-readiness-20261025/postgres/02-readonly-rust-identities.sql)，不启动 Alia 来代替查询，不触发自动迁移。实际账号、schema journal、embedding pin 和 Companion mode 未取得之前，生产目标仍未绑定。数据库启动自身会写运行状态／WAL，因此本轮“只读盘点”未执行这一步。

### 已有身份与不能沿用的 fixture

`alia-main` 的 `config/server.key` 有效，64 bytes、0600，公钥 SHA-256 为 `f1e259198c2ea443ed83a0ae2d35b7c8d76b4ee6a0f8690fa152a4acfbff95a4`。以保留原 Server 身份为默认，不生成替代密钥。只在内存派生公钥，未调用会 chmod 的应用 key loader，未保存私钥原值。

本机旧 `config/device.key` 对应 `alia-embedded-b40503784388`，但旧 Embedded TOML 含退休字段、`provisioned=false`；旧 SQLite 只有音量设置，没有 `binding_state`，也没有 R1 claim。10 月 21 日 Pi noop 和 10 月 24 日 R1 任务使用独立验证身份，不能升级为生产账号或绑定。现有 `pi-living-room` 仍缺 owner UUID、设备 pin 和可达 endpoint；后续从权威 SQL 与实际 Pi 持久状态核对。33 个配置／key／状态文件前后未变，见[身份报告](../../run/mac-production-readiness-20261025/identity/inventory.json)。

### Mac 常驻方式的选择

当前资产与进程都归 `ke-qiao`（UID 501），因此首轮建议沿用该用户的**登录会话服务**。已观察到 Magpie 127.0.0.1:3425、LM Studio 127.0.0.1:1234 和共享 GPT-SoVITS `*:9880`；独占 Juno 19880、Easy-Turn 8091 尚未运行。Magpie 的现有 tray LaunchAgent 处于 not running，但网关另有活动 PID，不能把 tray job 状态当作网关状态。

LM Studio 的 HTTP autoStartOnLaunch 已开，而桌面 enableLocalService 关闭；当前目录存在模型不等于已加载，也不证明无人登录开机可用。当前 Instant 已加载、embedding 未加载；JIT 与自动卸载前一个 JIT 模型的设置仍在。部署候选要求显式预加载两个选定实例，并检查 native `loaded_instances`／上下文，不仅 GET `/v1/models`。现有 `logSensitiveData=true`、verbose=true 应在获准应用设置时关闭；本轮只记录并准备变更，不修改原设置。

待安装的 Easy-Turn／独占 Juno／Server LaunchAgent、独立 Juno YAML、只读就绪检查及首次启动顺序见[服务材料](../../run/mac-production-readiness-20261025/services/README.md)。复用已有 Magpie／LM Studio 管理入口，不重复创建会竞争相同端口的服务。保留共享 9880，Juno 新配置只使用 19880；上游会写 YAML，运行副本必须在私有目录，不能指向旧共享配置或证据原件。

材料验证：3 份 plist lint、就绪判断 13 个 fixture 用例、Server 的 7 个拒绝／exec 拦截用例通过，均未启动服务。首次只读 HTTP 观察保留在 [readiness-current.json](../../run/mac-production-readiness-20261025/services/readiness-current.json)；[最终判定](../../run/mac-production-readiness-20261025/services/readiness-final.json)结合该观察与最终只读策略检查，未重复整套 HTTP，Juno 的实际 job／PID／YAML／权重分支因服务未运行而未执行。当前整套 ready 为 false，不能把静态通过写成服务已就绪。

首轮模型 jobs 按登录启动准备，Server 由操作者在就绪检查通过后显式启动。launchd 没有这些应用的依赖排序，Server 使用 `RunAtLoad=false`、全部新 job 使用 `KeepAlive=false`；保留首次失败，不自动重启求绿。下方系统 LaunchDaemon 模板仅描述 Server 的系统域运行方式，**不能证明整套 GUI 模型栈在无人登录时可用**。若选择开机无人登录运行，须另行落实 headless 模型服务、资产／凭据权限、OrbStack 启动和冷启动验收，不能直接混用两种运行域。

以上为 10 月 25 日盘点时的待选项；数据库来源和登录运行域已由上方 10 月 28 日决定替代。共享 LM Studio 策略仍待定，参考文本和保留 `alia-main` 密钥无需重选。

## Mac 与 Pi 的安装和常驻入口

以下是**待部署材料**，本轮没有安装服务、切换二进制或启动设备。先在部署清单填入双端主机、服务账户、候选 commit／产物 SHA-256、绝对版本目录、私有配置／状态／模型／日志／备份目录、监听地址和端口。复用已有绑定时沿用原身份和状态；首次部署才创建新身份。`DEPLOY_USER`、`CANDIDATE` 和 `/absolute/...` 都是待替换值，不是现场已确认路径。

1. 从[候选交付记录](software-release.md)选定同一源码身份的 Mac Server 和 Linux ARM64 Embedded，核对归档与解包后二进制摘要，放入各自独立版本目录。不要复制整个开发仓库或覆盖现有运行文件。Linux 按交付包的 `ldd` 清单核对依赖；两端都复核平台和可执行权限。
2. 私有目录由实际服务账户拥有，目录权限 `0700`、含凭据配置 `0600`；版本目录只用于代码，配置、SQLite、插件、R1 claim 和模型放在版本目录外。核对服务账户对目标 PCM／UART 的实际访问权限，不把整个设备目录设为全员可写。Server 的 PG／pgvector 和所选模型服务须先单独就绪；下列服务管理器不负责启动或等待它们。
3. 按下节准备配置，在 Pi 用候选二进制 `--check-config` 检查副本；核对 `speech.channel_id` 与 Server channel、设备公钥 pin、真实网络可达地址。Mac 的 `server.public_endpoint` 必须是 Pi 可达地址，不能把 `0.0.0.0` 当作对端地址。生产身份首次配对只在受控网络和人工核对目标后执行，见本页账号入口。
4. 首次正常启停、配对和模型链验收通过后，再启用常驻启动。保存实际服务文件及其摘要、所有非敏感环境变量和凭据来源名称；不要把 secret 写进下面的公共模板。生产日志与备份留在私有目录，不收集完整 settings 响应。

Mac 使用系统级 LaunchDaemon，以选定的普通账户运行 Server；它不依赖 GUI 登录。保存为私有待审副本 `com.alia.server.plist`，替换用户和路径并用 `plutil -lint` 检查：

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>com.alia.server</string>
  <key>UserName</key><string>DEPLOY_USER</string>
  <key>ProgramArguments</key><array><string>/absolute/releases/CANDIDATE/alia-server</string></array>
  <key>WorkingDirectory</key><string>/absolute/alia</string>
  <key>EnvironmentVariables</key><dict>
    <key>ALIA_CONFIG_PATH</key><string>/absolute/private/server.toml</string>
    <key>RUST_LOG</key><string>info</string>
  </dict>
  <key>StandardOutPath</key><string>/absolute/private/log/server.stdout.log</string>
  <key>StandardErrorPath</key><string>/absolute/private/log/server.stderr.log</string>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><false/>
</dict></plist>
```

下一轮安装时将审核后的文件以 `root:wheel`、`0644` 放到 `/Library/LaunchDaemons/com.alia.server.plist`，先确保日志父目录存在。`sudo launchctl bootstrap system /Library/LaunchDaemons/com.alia.server.plist` 会立即启动；用 `sudo launchctl print system/com.alia.server` 核对 PID、退出状态，读取上面的两个日志。普通停止先 `sudo launchctl kill SIGTERM system/com.alia.server`，等待 PID 退出并记录清理结果；确认退出后 `sudo launchctl bootout system/com.alia.server` 卸载。禁用下次开机加载还需移走这一个 plist，保留私有副本；不要删除配置和状态。再次加载前核对数据库、模型与当前版本。LaunchDaemon 与用户 LaunchAgent 的登录边界见 [Apple 指南](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/CreatingLaunchdJobs.html)。

Pi 使用系统级 `alia-embedded.service`；这里的普通启动**不发送 R1 初始化命令**：

```ini
[Unit]
Description=Alia Embedded
Wants=network-online.target
After=network-online.target

[Service]
Type=exec
User=DEPLOY_USER
WorkingDirectory=/absolute/alia
Environment=ALIA_EMBEDDED_CONFIG_PATH=/absolute/private/embedded.toml
Environment=RUST_LOG=info
ExecStart=/absolute/releases/CANDIDATE/alia-embedded
Restart=no
KillMode=mixed
TimeoutStopSec=60
SendSIGKILL=no
UMask=0077
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
```

下一轮用目标 Pi 的 `systemd-analyze verify /absolute/alia-embedded.service` 检查替换后的文件，再安装到 `/etc/systemd/system/alia-embedded.service`、`sudo systemctl daemon-reload`。`sudo systemctl start alia-embedded` 才会启动设备；首次验收后 `sudo systemctl enable alia-embedded` 才设置开机启动。诊断使用 `systemctl status alia-embedded`、`systemctl show alia-embedded -p MainPID -p Result -p ExecMainCode -p ExecMainStatus` 和 `journalctl -u alia-embedded --since '本轮开始时间'`。正常停止用 `sudo systemctl stop alia-embedded`；禁用开机启动用 `sudo systemctl disable alia-embedded`。60 秒是服务管理器的停止观察上限，不修改应用或测试期限；超时仍记录失败，不以自动强杀或重启隐藏资源未清理。`KillMode=mixed` 先让主进程负责子任务关闭，`SendSIGKILL=no` 保留未退出进程供定位，后续启动前必须确认它们已经退出。语义见 [systemd.service](https://www.freedesktop.org/software/systemd/man/latest/systemd.service.html) 与 [systemd.kill](https://www.freedesktop.org/software/systemd/man/latest/systemd.kill.html)。

两个模板均不自动重启失败进程；初次生产验收先保留原退出与 Unknown 状态，后续若启用重启策略，应连同故障恢复一起验收。系统服务也不会防止 Mac 休眠，24 小时验收需同时记录实际供电／休眠策略和日志保留上界。不要为了长稳验证临时增大应用期限。

R1 系统模式在**确认真实板端冷启动后**，需要用同一个长期运行进程执行一次 `--initialize-r1`，不能先运行初始化然后另启一份普通 runtime。采用上述 systemd 服务时，可在已停服且设备 owner 已释放的现场窗口创建唯一的临时 `/run/systemd/system/alia-embedded.service.d/initialization.conf`：清空 `ExecStart=`，再指定同一候选二进制的 `ExecStart=/absolute/releases/CANDIDATE/alia-embedded --initialize-r1 /absolute/private/embedded.toml 已确认冷启动的新UUID`。`daemon-reload` 后只启动一次；保存初始化结果和 claim，再移除这一个临时 override 并 `daemon-reload`，**不重启正在保持采集流的进程**。失败或结果未知也先移除 override、保留 claim 和错误，不能重发。普通重启使用无参数模板；机器重启／设备断电不自动获得重新初始化许可，必须先诊断并由操作者确认冷启动。完整条件见 [R1 启动契约](../../packages/alia-embedded/docs/audio-and-voice.md#r1-system-controlled-capture)。

停服顺序为 Server → 确认其退出 → Embedded。记录每端 exit status、子进程、原监听端口、数据库连接及 PCM／UART owner；监听关闭或服务 inactive 单独不足以证明清理完整。只有双端和同库写入者全部停止后才执行[一致备份与恢复](upgrade-recovery.md)。实际配对恢复、开机／普通重启／设备冷启动、升级恢复和 24 小时混合负载均须在下一轮生产部署中验证，本页模板静态检查不替代这些结果。

## 配置与启动

从仓库根目录按 [本地开发](local-development.md) 准备主文件和 `.d` 分片；[Server 配置](../../packages/alia-server/docs/configuration.md)与 [Embedded 配置](../../packages/alia-embedded/docs/configuration.md)分别管理业务/模型和设备实时运行时。`ALIA_CONFIG_PATH` 可以选择 Server 主文件，配套分片目录为同名 `.d`；Embedded 使用独立的 `ALIA_EMBEDDED_CONFIG_PATH`。

正常启动会产生实际副作用。Embedded 可能创建或调整设备密钥权限、保存配置、创建/迁移 SQLite、绑定监听地址，然后在 `Builder.build()` 返回前启动选定的播放、motion、camera、capture 和 discovery。未配对不代表麦克风尚未开始采集；`motion.hardware_enabled=false` 只限制执行器，不能关闭其他设备。通用 [embedded.example.toml](../../config/embedded.example.toml) 是配置说明示例，其中原生采播、模型、vision 和发现仍按模块选择启用，不能直接当本轮 Pi 现场预设。

已有二进制提供独立的只读检查入口，必须指定实际存在的配置文件：

```sh
./target/debug/alia-embedded --check-config /absolute/path/embedded.toml
```

此入口只读配置，以及配置指向的校准文件和已批准动作文件，执行静态验证；不创建/调整 key，不进入 Builder、不打开数据库、模型、PCM、camera、串口或 I²C，不执行绑定、网络发现或启动服务。缺少配置明确报错，不将 compiled defaults 当作已检查的文件。报告指出配置选择与前置条件，不能证明真实设备、模型、机构安全或目录写权限已经可用，也不核对 Server 的批准清单。构建二进制本身会写入构建目录，应与这次只读运行区分；不要用普通 `cargo run` 或视觉 setup 脚本代替预检。

如需后续验证无设备的正常生命周期，使用专门的 [embedded.staging.example.toml](../../config/embedded.staging.example.toml)，先复制到新的私有配置目录，再检查复制后的文件。模板显式关闭全部 module 槽位、motion、vision/debug、毫米波和 mDNS/BLE，监听 `127.0.0.1:0`，保留正常绑定边界；state、key、plugin、cue 使用相对配置文件的独立 `staging/` 路径。不要与已部署设备共用这些目录。正常 staging 启动仍会写 key/state/config 并创建 loopback listener，**不属于只读操作**；它也不检验真实录放音或执行器。

```sh
ALIA_CONFIG_PATH=/absolute/path/server.toml cargo run -p alia-server
ALIA_EMBEDDED_CONFIG_PATH=/absolute/path/embedded.toml cargo run -p alia-embedded
```

上面是正常运行命令，现场前置未完成时不执行硬件配置的启动。staging 需要正常运行时，应把第二行路径明确指向复制后的 staging 文件。使用该前台进程的 Ctrl+C 或针对其 PID 的 SIGTERM 请求正常关闭，并核对进程退出与清理结果；超时不证明 native 工作或物理输出已安全停止，不使用按名称批量 kill 或顺带停止共享服务。

2026-10-17 已在既有 Linux ARM64 release 上完成隔离全 noop 的两轮实际启停与恢复，见[验收与恢复检查点](../refactor/resume-checkpoint-release-lifecycle-2026-10-17.md)。联调使用私有loopback端口和任务数据库；Server选择noop模型、禁用Easy-Turn/HA且保留默认政策，现有channel内部mDNS browse仅在隔离网络内。首次Embedded启动会按新密钥推导device ID并写回配置，即使仍未provisioned；重启应保留该配置、key和state，以首次正常关闭后的身份为基线。Server持久settings也需保留；noop模块的`ignored_runtime`只表示保存了期望值，不能当作模型已应用。此次通过不覆盖真实服务账户部署、绑定恢复、异常强杀/断电或硬件/模型。

仓内 `pnpm dev:backend` 会同时启动开发数据库、Server 和 Embedded，`pnpm dev` 还包含前端；这些便利入口不负责本轮隔离目录、设备权限或测试资源归属。`scripts/setup-embedded-vision-pi.sh` 是安装/模型/摄像头配置工具，可能安装依赖、下载文件和查询设备节点，`--run` 会启动完整 Embedded；它不是无设备只读预检。Server 普通配置加载也可能执行已有发布恢复；不能把正常 Server 启动称为纯只读解析。

未找到 Server 配置时进入 OOBE；配置已存在但不能加载时会报错退出，不自动改写成样例。stdin/stdout 都是交互终端时，输入 `c` 再按 Enter 可生成主文件加 `.d` 的完整布局，该操作拒绝覆盖已有主文件或分片树。

setup 固定监听 `127.0.0.1:3000`，此时尚未使用主配置的 Server 端口，不在局域网广播未认证的配置写入口。远程机器可使用 SSH 隧道；无交互终端时使用 HTTP setup。两个配置操作均为 POST：`generate-config` 返回 `{template: TOML字符串}`，`save-config` 接受 `{config: TOML字符串, path?: 目标主文件路径}`。`path` 指向 **Server 所在机器**的文件系统；省略时先采用该进程的 `ALIA_CONFIG_PATH`，然后按 CLI 默认路径选择。

setup 只接受 `127.0.0.1`、`localhost` 或 `[::1]` 的 Host；浏览器带 Origin 时必须与 `http://Host` 精确一致，否则返回 403。保存必须使用 JSON Content-Type，错误媒体类型返回 415。无 Origin 的本地 curl/SSH 隧道按现有方式工作；不要把入口放到任意域名的反向代理后。该策略约束浏览器来源，仍信任本机操作者，文件可写范围由服务账户权限决定。

HTTP 模板是包含所需分片内容的完整单文件，不会额外创建 `.d`；已有目标 `.d` 仍按普通加载规则覆盖主文件。保存前校验实际有效的 role/profile 引用，成功后退出 setup 并按配置启动 Server。以下命令需要 `jq`，可在第二个终端执行：

```sh
umask 077
curl --fail --silent --show-error -X POST \
  http://127.0.0.1:3000/api/v1/setup/generate-config \
  | jq -er '.template' > setup-server.toml
# 编辑 setup-server.toml，填写数据库、模型地址及密钥。
jq -n --rawfile config setup-server.toml \
  --arg path /absolute/path/server.toml \
  '{config: $config, path: $path}' > setup-request.json
curl --fail --silent --show-error \
  -H 'Content-Type: application/json' --data-binary @setup-request.json \
  http://127.0.0.1:3000/api/v1/setup/save-config \
  | jq -e 'if .success then {success, path} else error(.error // "setup save failed") end'
```

`save-config` 的正常 JSON 响应包含 `success`、`path`，失败时另有 `error`。配置验证或保存失败也可能返回 HTTP 200，因此必须检查 `success`；仅用 `curl --fail` 不够。发布开始前的失败允许修正后重试。发布已开始但同步失败时，错误会明确持久化仍为 unknown；setup 保留原发布，只接受同一路径、内容和安装方式的重试，不能换请求覆盖。只有持久化确认后才进入 Ready；成功响应排空后 setup 关闭，其端点不作为常驻管理 API。准备阶段的 `/health` 返回 `status: "setup"`，不是模块健康结果；启动完成后使用配置中的实际 Server 地址和端口。

同一 setup 进程只有一次成功完成：并发 HTTP/CLI 的输家返回失败且不写第二份配置；取消后，即使已接收请求随后才补齐 body，也不会发布。base、分片树及完整 canonical 祖先目录链按发布协议同步；若发布已开始后失败，不删除 base 或 CLI 分片，而是保留私有的相邻恢复记录。记录只含路径和内容摘要，不含配置值或密钥。CLI 按 `c` 重试原安装；HTTP 重发原请求；取消保留磁盘文件。重启时选择相同配置路径（自定义路径用 `ALIA_CONFIG_PATH`），加载器会先恢复再加载，正常无记录时仍纯只读。

恢复发现 base、候选或分片内容改变/丢失时会拒绝盲覆盖，应保留文件与报错交由操作者核对，不要只删恢复记录强行继续。恢复记录建立前崩溃可能留下需人工核对的临时文件/CLI 分片；也不能同时让其它进程修改正在校验的文件树。实际断电、存储硬件与服务账户部署权限仍需独立验证，见 [本轮发布与恢复说明](../refactor/config-durability-2026-10-11.md)。

配置语义有效不等于数据库、模型或硬件已在线。先准备独立 PostgreSQL/pgvector 和选择的 [模型服务](model-services.md)，再核对 `/health`；不要以 HTTP 200 或非空 API key 判断所有 Provider 可用。

## 账号与现有管理入口

通过 `POST /api/v1/auth/register` 建立账号，`POST /api/v1/auth/login` 获得 token，后续请求使用 `Authorization: Bearer …`。首个账号的管理员分配规则以 [认证实现](../../packages/alia-server/src/api/auth.rs)为准；不要在命令历史中直接放入真实密码或 token。下列示例假定 `ALIA_URL` 和 `ALIA_TOKEN` 已指向本人的隔离开发实例。

```sh
curl --fail --silent --show-error \
  -H "Authorization: Bearer $ALIA_TOKEN" \
  "$ALIA_URL/api/v1/users/me"
curl --fail --silent --show-error "$ALIA_URL/health"
```

| 用例 | 已有入口 | 权限与结果边界 |
| --- | --- | --- |
| 渠道/设备发现与绑定 | `GET /api/v1/channels`，`GET /api/v1/device?channel_id=…`，`POST /api/v1/device/bind` | 使用明确的 Embedded channel，人工核对发现结果及设备公钥 pin；发现不代表绑定完成。 |
| 设备状态与设置 | `GET /api/v1/device/status?channel_id=…`，`GET/PATCH /api/v1/device/settings?channel_id=…` | 设备 owner 或当前管理员；缺少遥测保留 null，网络失败不伪造成已观测的离线数据。 |
| 解绑与重连 | `POST /api/v1/device/unbind`，`POST /api/v1/channels/{id}/restart` | 重启渠道需管理员；解绑退休原 session，重新绑定使用新凭证/session，不复用旧权限。 |
| 主动策略、shadow/回放 | 四个 Companion REST 操作 | 见 [Companion 操作](companion.md)；政策完整 CAS，503 必须核对原意图，默认 shadow。 |
| 记忆/隐私/检索配置 | `/api/v1/memories/*`，`GET/PATCH /api/v1/settings` | 见 [Memory 操作](memory.md)；账号归属、来源撤销、持久 revision 不因重试刷新。 |
| 人格与关系状态 | `/api/v1/personas/me`，关系读取/清理/重建接口 | 见 [Relationship 操作](relationship.md)；清除需原 operation/revision，重建不能复活已删除来源。 |
| 工具执行/确认/取消 | `/api/v1/tools/execute`，`/api/v1/effects/{id}`，`/{id}/confirm`、`/{id}/cancel` | 从 `/scalar` 的 execution 契约取得完整请求；保留原 effect identity/version，Unknown 先核对，不再次调用非幂等工具。 |
| Response 与文本/音频/身体结果 | `/api/v1/responses/{id}`、`/{id}/deliveries`、`/{id}/events` | 见 [Delivery API](../refactor/delivery-api.md)；Submitted、Presented、位置反馈和完成分别记录，查询不是重播。 |
| 配置/模块健康 | `/health`、`GET /api/v1/settings` | 检查每模块状态；默认 probe=false；OpenAI/Gemini 在线 GET 需显式启用、单次 5 秒。整体一轮最多 6 秒、8 项并发；busy/disabled 均不能当作在线通过。 |

`GET /health` 保持公开 HTTP 200 和 `status`、`version`、`modules` 响应字段，仍需检查各模块状态。相同配置 LLM profile 的多个角色共享一次检查；Router 只报告本地模式和依赖绑定，模型状态见对应 `llm.*`。同一 Registry 有一轮检查进行时，重叠请求立即返回 `degraded` / `health check already in progress`，不排队、不复用旧结果。6 秒整体期限到达后保留已完成状态，未完成项返回 `degraded` / `health check timed out after 6s`；停机时未完成项返回 `server is stopping`，单项 panic 返回 `unhealthy` / `health check panicked`。后续请求在空闲时重新检查。

这些期限约束合作式异步检查；Server 不启动后台探测，也不能保证已发出的远端请求在远端立即停止。设置更新不等待健康网络探测，已经发出的探测使用其开始时的配置快照。没有新增深度诊断端点或 probe 热切换开关。

四个 Companion 操作和 execution/delivery 契约可在 Server `/scalar` 查阅。其他旧模块的 OpenAPI 覆盖不应由这个页面存在就自动推断；对应 Markdown 和实际 REST 用例仍是操作依据。

`GET /api/v1/settings` 在全部设置快照等待结束后、返回 JSON 与政策 ETag 前核验当前 active admin。等待期间已降权、停用或删除为 403；最终核验失败或超过 2 秒为脱敏 503，均不返回设置或 ETag。成功响应仍保留原值/schema 与 prefix 行为，包括管理员可读取的 secret 值。原 SQL 暂停到期与内存政策投影保持，响应准入后不持续持有用户锁。

纯非政策 `PATCH /api/v1/settings` 会在更新锁、政策读取和 embedding 预览结束后，重新核验当前 active admin。等待期间已降权、停用或删除返回 403；核验数据库失败或等待超过 2 秒返回脱敏 503，且不应用或保存本请求设置。已经准入的操作继续沿用部分成功规则，不因随后降权自动回滚。

运行时 `PATCH /api/v1/settings` 有 SQL 政策提交、期望设置更新、模块生效和文件保存几个独立完成点。HTTP 200 中 `success=false` 不表示此前动作已回滚，应核对 `context_policy_version`、`applied`、`module_reports` 与 `errors`。同值显式重试仍会调用模块并保存。`settings.toml` 通过完整快照原子替换；发布前失败保留旧文件，发布后同步失败明确报告持久性未确认。失败后直接重启只加载磁盘上完整的旧版或新版，SQL 仍是政策权威；此路径没有首次 setup 的恢复记录。详见 [运行时设置说明](../../packages/alia-server/docs/configuration.md#runtime-settings)。

`PATCH /api/v1/memories/privacy` 现要求管理员，提交同一全局 SQL 政策并支持相同 `If-Match`；普通登录用户仍可读取隐私状态。隐私 GET 与 audit 读取实际政策，不再返回旧内存投影。该权限收敛已由用户确认。

绑定请求应沿设备实际发现结果填写 `channel_id`，保持 `require_confirmation=true`。该字段要求 Server 有待确认的发现候选，**不代表设备侧已实现物理按钮确认**；当前 `discovery.require_physical_confirm` 没有实际设备确认处理。首次绑定必须先在受控网络核对目标设备、公钥 pin 与 Server 身份，不能把配置布尔值当作防抢绑保证。请求 JSON 为 `{"channel_id":"实际渠道ID","require_confirmation":true}`；若 Server 使用 wildcard 监听且未配置 `server.public_endpoint`，还需给出 Pi 可达的 `server_endpoint`。成功后检查响应 `status="bound"`，再用受保护的 `/api/v1/device/status?channel_id=…` 和设备 settings 查询核对原 device ID 与绑定。已有绑定正常恢复时不发送 unbind/bind；身份不匹配先保留日志定位，不删除 key／SQLite 来重新配对。这项软件接口检查仍不等于真实 Pi 的麦克风／声卡和执行器已经验收。

## 诊断与恢复

ExplicitWait 只响应新的明确 Continue；临时避让可由新鲜 Backchannel 恢复。恢复使用原 Response、原 generation 和尚未提交的音频后缀，不调用新 TTS 重建前缀。协议要求 voice v2、playback v3，主动邀请额外要求 capture gate v1，见 [语音协议](../refactor/voice-protocol.md)。

数据库或网络错误可能使写入完成状态未知。读取原政策、operation、Response、Delivery 或 Effect 核对，不换身份推测性重做。`cancel_requested` 不是设备已停声/机构已到位；最终还需原 native owner 与设备事实。

本包验证使用独立临时配置、本地受控 Provider 和隔离测试数据库。实际安装、配对及 24 小时混合负载仍归 [R02—R06 checklist](../refactor/checklists/next-round.md)。Rig 模型／工具构建、rmcp 客户端和 Extism WASM 已接入，现行操作见 [MCP](mcp-tools.md) 与 [WASM](wasm-plugins.md)；具体外部服务、模型和硬件仍须各自验收。
