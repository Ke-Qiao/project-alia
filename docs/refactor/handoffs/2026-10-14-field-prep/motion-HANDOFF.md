# 现场前置：R03 动作资产、校准与部署 HANDOFF

日期：2026-10-14。基线 `283f38f`（Root 提供），Owner `settings_auth_fix`。先只读核对现有实现和操作资料，再按 Root 明确分工补离线 motion helper 与操作文档。本 agent 未运行 Cargo/Git/SSH/Docker、设备 I/O、录放音或执行器动作。

## Checklist

- [x] 读取 R03-A 恢复检查点、motion-assets、配置和硬件部署记录。
- [x] 搜索既有 CLI/examples、资产 pin/读取/解析、校准与 PCA 参数检查入口。
- [x] 确认哪些检查已实现，哪些不能通过普通启动代替。
- [x] 将可落地缺口、最小完整交付与待现场资料报告 Root。
- [x] 记录无需重复询问的既有决定、限制与建议验证。
- [x] Root 选定范围后完成 motion 离线 helper、最少回归与 motion-assets 操作说明，源码停写；独立静态审查无阻断。
- [x] Root 统一运行实际回归后读回日志，记录计数/输入/限制。
- [x] 完成本分工交接并停写，提交身份与总检查点交由 Root 最后统一收口。

当前 **8/8（本分工完成）**。生产与测试已实现并格式化，Root 实际统一验证通过；本文件不预报尚未提交的 commit。不是 R03 实机完成率，R01—R06 仍按总台账计数。

## 已有可复用能力

| 路径 | 已实现事实 | 操作限制 |
| --- | --- | --- |
| `crates/alia-core/src/body.rs:19` | `BodyAnimationAsset` 严格字段；canonical 单叶 `.anim`、version>0、lowercase SHA-256、1..65536 bytes；清单最多64项/无重复；纯 hash 与 exact bytes 校验 | hash 计算是内容身份，不是操作者批准；没有独立 operator CLI |
| `packages/alia-embedded/src/module/motion/assets.rs:10` | 已批准完整 tuple 匹配；directory fd、no-follow/nonblocking、regular file、同次最多 bytes+1 读取并校验 | 私有生产读取能力；不能直接用于“尚待审核”文件并偷偷创建批准 entry |
| `packages/alia-embedded/src/module/motion/plan.rs:139` | 同 buffer UTF-8、三 finite 角度、每角绝对值≤180、最多600源帧；prepare 再核验 loops/fps/duration | 目前私有；没有独立离线报告入口。原动作身份/权限必须保留，工具不应假造 lease 来调用准入 |
| `packages/alia-embedded/src/module/motion/neck_mapper.rs:35` | 矩阵 JSON、≤1MiB、regular file、维度、finite 系数、sigma、neck/servo 限位、通道数与角度目标检查 | 私有模块；矩阵可解析不证明实际机构已校准；现 `load` 在 File::open 后才查 regular，不能直接声称所有特殊路径读取均有界 |
| `packages/alia-embedded/src/module/motion/runtime.rs:477` | 构造时清单校验；enabled 时完整配置检查；confirmed 时矩阵/servo_count/center 检查；native backend deferred 不在构造中打开设备 | `calibration_confirmed=false` 有意不加载矩阵；disabled 也不跑全部参数检查。不能临时设确认=true 来伪装离线检查 |
| `packages/alia-embedded/src/module/motion/runtime.rs:1090` | fps、servo_count、速度、中心、face 参数、min/max PWM 的基本边界；实际 hardware+pca 时才查地址覆盖/范围/重复 | 关闭硬件的配置加载不等于 PCA 参数已全面核验。当前只要求 frequency>0，prescale 与 duty 后续仍有 clamp |
| `packages/alia-embedded/src/module/motion/servo.rs:533` | 软件按 configured pulse/frequency 计算计数，按25MHz计算 prescale，再 clamp 为3..255；duty clamp 0..4095 | 这是现有软件计算，不是板上实际 oscillator/frequency 或 MG996R 电气规格证据；不能从“配置可解析”推断这些值安全 |

## 基线审计确认的操作缺口

1. **没有无需启动设备的 motion 命令入口。** `examples/` 只有 `audio_evidence_replay.rs`。`main.rs:50` 起会 ensure keypair，缺 identity/config 时写文件，随后 `EmbeddedBuilder` 构建并运行节点；即使 motion disabled，也不能据此推断其它音频/发现/vision 模块零设备动作。普通启动不是静态预检。
2. **现有 loader 不等同严格只读预检。** Embedded `config/mod.rs:24` 的缺文件语义是默认配置，不应向操作者报告“指定文件已检查”。Server `config.rs:2077` 的 `load` 先执行 publication recovery，可能同步/恢复文件，不能直接包成纯读取检查。`load_candidate` 有现成不发布的 merge/validation 逻辑，但访问级别和 missing/recovery marker 的语义需明确复用，不重新手写一套分片合并规则。
3. **资产批准流程没有完整可复制的离线链。** motion-assets 已说明空默认、双端 tuple、停进程后改清单、重启；但未给明确的候选 pin 生成、文件格式报告、审核记录、选定 Server channel 的有效清单与设备清单比较，以及复制后目标文件重核验步骤。只跑两次 `shasum`/`wc` 不能复用同次读取，也不包含 canonical/帧/大小/清单检查。
4. **校准资料仍没有实际值。** 文档没有可填写的 channel→board/address/output→servo→机构作用映射、独立舵机电源/共地/OE、机械限位/中心及测量来源表。示例 `serial`、8通道、50Hz、500–2500µs、90°都是仓库示例，不能作为已选 PCA9685A+MG996R 的实测部署值。
5. **历史工具描述容易误导。** motion-assets 末尾仍提 Python `neck_control`/Windows 校准工具的保留备份；当前仓库搜索未发现可用旧工具入口。可以说明这是历史资产来源，但不应指引现场依赖仓内不存在的校准工具，也不应为补缺口重建旧运行时。

## 审计时建议的最小完整交付

推荐本轮准备“离线内容/参数检查 + 明确审核记录模板”，不改执行器状态机、不代填硬件数值，不启用 `hardware_enabled` 或 `calibration_confirmed`。若 Root 决定本轮只做资料，则上述缺少离线入口必须明确保留为后续项，不能写成现有能力。

- 一个显式离线入口或 example，输入必须是操作者指定的已存在配置/资产路径；缺文件返回 missing。只用普通受控文件和现有依赖，绝不调用 main、identity ensure/save、Builder.build、runtime start、设备枚举或网络。
- 资产候选检查复用 canonical 名称/64KiB 边界、同一次 regular-file buffer 的 hash/length 与实际帧 parser。输出 version由操作者提供的 **candidate tuple**、源帧与角度范围；不写清单，不扫描目录批量自动批准，不生成命令/lease/签名。已批准资产校验则复用 exact tuple verification，不能用新计算结果覆盖旧 pin。
- 校准检查直接复用实际 NeckMapper 校验，独立于确认开关。报告矩阵尺寸、通道数、configured center是否在各通道限位、限位/中心文件身份及“实际机构未验证”。不为了检查篡改确认值，不声称动画在真实负载下安全。
- PCA 检查从所选 backend 配置报告地址覆盖/重复、通道、请求频率/脉宽、既有换算结果和可表示性；不设置未知的最佳值，不依据估计声称 board 在线。若识别出 clamp 导致请求值不可表达，先单独报告并保留该结果，不在现场准备轮次静默改生产参数。
- 双端核对采用明确 channel ID、Server **有效**分片合并清单与 Embedded 本地清单，按完整 tuple 检查差异，不把名字一致当内容一致；操作者先停止进程，审阅并分别写入受控配置，再核对部署目标文件，最后才决定是否重启。检查工具不执行批准或部署。
- 更新现有 motion-assets/硬件资料，补实际填写表和正确命令，删除对不存在校准工具的操作暗示；保留历史来源说明及 R03-A 版本迁移要求。

新入口如实施，最少验证应覆盖：missing config不fallback；不创建identity/数据库/目录、不改配置；oversize/非regular/symlink资产拒绝且无阻塞；候选 hash与解析同buffer；已有 pin改一个byte拒绝；清单不一致与重复；坏矩阵/通道数/中心越界；确认仍false且没有 native backend构造/start。测试只用临时合成文件，Root 统一 Cargo 窗口。无需实机或另一个模拟框架。

## 已实施候选与实际范围

Root 选定新增 Embedded `--check-config <path>` 为显式离线入口（main/preflight及 CLI 集成测试由 Root 拥有）。本 owner 完成以下文件，并已停止源码修改：

- 新 `packages/alia-embedded/src/module/motion/inspection.rs`，通过 `motion/mod.rs` 导出 `inspect_configuration(&MotionConfig) -> Result<MotionInspection>`；report 实现 Serialize。
- 复用 `runtime::validate_config`；将原 PCA 地址范围/覆盖/重复逻辑提为 `validate_pca_addresses`。运行时原 `hardware_enabled && pca` 触发条件保持；离线 inspector 对所选 PCA 在 hardware=false 时也查静态地址规则，避免需要启用硬件才能发现配置错误。
- `plan::read_animation` 仅改为同模块可见，inspection 复用原 approved tuple校验、同次有界读取及真实帧parser，不复制动画规则。
- 校准走专用只读检查：先 symlink_metadata 拒绝非regular/symlink，再 no-follow/nonblock/cloexec 打开并复核 fd；≤1MiB，同 JSON parse/servo_count/center validation。未确认且缺文件为 `status:not_checked,reason:missing`；存在的未确认矩阵可以被 checked，但原 `calibration_confirmed` 仍false；confirmed缺失或任何已存在非法文件报错。
- report 仅记录输入 flags/backend/servo_count/fps/center，校准检查状态、PCA配置数值、已批准原asset及source_frames；始终 `physical_state:not_observed`。没有 runtime/backend/worker 构造、自动hash批准、清单写入、模拟命令或 lease。未增加 PWM电气/频率新规则，明确静态检查不测量实际频率/位置。
- `motion-assets.md` 增加真实 `--check-config` 命令与字段解释、现成 shasum/wc 候选身份步骤（单独两次读取不是原子快照、不是批准）、停进程后双端手工tuple核对、现场校准记录。旧 Python/Windows工具仅保留历史资产来源，不再指引使用仓内不存在的工具。

与审计建议相比，本轮**没有新增自动候选hash工具或 Server有效清单比较器**。Root 明确选择候选身份用现成命令，双端清单核对保留操作者步骤；顶层报告将 `server_catalog_agreement` 列为未检查。只核对已明确批准的资产，不自动批准历史 `.anim`。这不是未声明的实现缺口，也不扩Server config恢复接口。

已编写5项 meaningful unit tests，精确集合 filter 为 `module::motion::inspection::tests`：

1. 未确认缺矩阵返回unchecked且不创建任何文件；确认后缺矩阵拒绝。
2. 有效未确认矩阵可检查而不改变确认；confirmed/hardware=true仍不打开不存在的I²C路径，原矩阵不变。
3. 维度、中心、fps、PWM、未确认native、重复/缺失/非法PCA地址、坏矩阵及超1MiB拒绝；地址错误在hardware=false也拒绝。
4. 已批准tuple原样返回、source_frames准确；同长度换字节拒绝；即使pin匹配但帧格式错误仍由生产parser拒绝。
5. 临时普通文件/symlink/目录/FIFO边界，校准和资产特殊文件在读取前拒绝。

Root 的 CLI 集成回归负责 missing config、参数路由、输出 JSON、不创建身份/DB/目录等入口证据。本 agent 不运行 Cargo；独立 reviewer 已只读确认 helper 的调用路径与边界无阻断。以下记录已实际读回的结果。

## 实际验证与保留的首次失败

最终冻结输入 **665 项**，manifest SHA-256 为 `dd888cec2f84c06a213cadc236911d088c89015b144025284e8eab72070d3573`。Root 在 macOS 上统一执行，本 owner 仅只读复核日志/JSON；两份最终测试 JSON 都是 exit 0、`source_drift=[]`。

| 验证 | 实际结果 | 证据（`run/validation-20261014-field-prep/`） |
| --- | --- | --- |
| Embedded 默认 features `--all-targets --offline --locked` | **316 passed / 0 failed / 0 ignored / 0 filtered**，45.05 秒 | `final-embedded.{log,json}`；14个Cargo targets，241 lib + 75 integration |
| Embedded `--no-default-features --test preflight --offline --locked` | **5 passed / 0 failed / 0 ignored**，17.77 秒 | `final-no-default-cli.{log,json}`；仅这5个CLI用例，不冒充无默认features整包通过 |
| 普通示例及 staging示例 `--check-config` | 两次 exit 0，输入配置 SHA-256 前后相同 | `cli-smoke.json`、`example-report.json`、`staging-report.json`；没有普通启动或staging服务启动 |

5项 motion inspection 单测在最终日志中全部通过；Root 5项 CLI 测试也全部通过，覆盖含 CPAL默认模块的配置仍先走离线分支、KWS环境变量不能绕过检查、无效key文件不解析及权限0640保持、missing/坏/过期字段/超限配置、symlink/目录/FIFO拒绝、help/错误参数不进入主流程。读回测试确认原文件快照与目录数量被核对，不仅检查stdout成功。

首次 `green-embedded` 并非最终通过：80.86秒 exit 101，manifest `cb14c07e0324213a1567836025fc89f5332cfae2b2987eadf7567b20b7166c96`，无输入漂移。motion5项已过，但新增 CLI fixture 的 `[identity]` 缺既有必填 `device_id`，解析准确拒绝，导致1项失败；Root只补齐该fixture必填字段后重新冻结并跑最终整包。原日志/JSON、`candidate-preflight-test.rs` 与候选补丁保留，这不是通过旧实现的业务RED，也未删除失败记录。

CLI输出只证明离线解析/文件检查，不证明设备权限、模型、服务、Server有效清单一致、实际校准或声学/执行器验收。本轮未运行 SSH、Linux应用、Pi、I²C/UART、录放音或真实动作。本分工源码和包文档已停写，Root负责最终提交及证据归档。

## 待现场补齐的信息

以下资料阻止真实启用，但不阻止上述离线工具/模板工作；由 Root 集中询问，避免重复逐项打断。

- PCA 板实际型号/照片/链接/地址跳线、接线图或逐线说明、板数、已接通道；MG996R 实物来源、数量和机构安装/负载。Pi 5V5A 规格不能代替舵机电源信息。
- 舵机电源电压/额定和峰值能力、共地方式、OE 接线/上电默认、断电与 Pi/PCA 重启时的允许行为；本轮不猜板上的电路。
- 每个 actuator 的 channel映射、实际机械安全范围、中心与校准记录来源/日期；角度与脉宽之间的实际标定，而非拷贝500–2500µs与0–180°示例。
- 待操作者审核的具体 `.anim` 文件及目标机构版本、预期动作、审阅人/版本记录；目前没有已批准资产，仓内历史文件仍未批准。
- 实际服务账户、双端配置/资产部署目录与权限；已有同用户插件部署权不能被目录检查当作 OS 隔离。

不用重问：用户已确定 PCA9685A+MG996R、停止/断连保持、完成机构校准后才允许启动归中；当前无校准资料。Pi5/8GB/Ubuntu与主机身份已有记录。幻尔方案、前端与额外平台均保持延期。本轮不再开一个泛化硬件抽象重构。

## Root最终接续记录

实现提交 `e409dc944a589104d0e2d2c2977f642ad8aaef28` 的665个blob已与最终输入核对一致。用户在分工停写后回复：R1仍未接入、AEC参考规格缺失，允许Mac模拟；R03机构资料均未定，后续先暂停；部署值待定，Pi可SSH。本轮交接以Root最新检查点为准，前文“待补回复”是当时审查状态。Mac只运行既有合成PCM示例，不启用真实音频、模型或执行器。
