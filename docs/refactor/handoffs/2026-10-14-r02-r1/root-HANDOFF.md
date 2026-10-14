# R02 R1 前置核对 — Root HANDOFF

日期2026-10-14；分支`codex/architecture-refactor`；基线`9890bab`，最近音频代码`28920fb`。用户要求先核对官方资料/实现/证据，明确分工和帧/声道/延迟，集中询问缺项，使用三个subagent；R03暂停、不加依赖、不启用设备、不预建框架。本轮没有生产、配置、API或测试改动。前置包10/10完成，三个subagent均6/6并停写，Root正常提交；六轮仍2/6。

## 已交付内容

- 重新读取官方公开course/product/introduction与三图，六项HTTP200并保存SHA/时间；导航页、web API不可读与沙箱DNS失败均保留说明，成功资料来自公开HTTPS而非猜测。
- UAC处理后单通道capture与功放reference分离；没有证明USBplayback/内部参考，规格图只有机械尺寸；厂商附件和用户实物版本仍缺。
- 当前输入声道全部平均，目标mono不是processed通道选择。确认processedmono端点可以复用现有链，混合语义端点先补已确认选路；没有R1故障复现，不编造修复。
- [中央前置记录](../../r02-r1-aec-preflight.md)给出候选拓扑、职责/帧/延迟条件、单consumer及owner边界；旧时钟文档的强制软件worker表述改成条件式。
- 集中问题已回复：用户确认硬件AEC优先；实物/固件/厂商附件与输出链/参考电气/模式/延迟仍未确定。Pi使用SSH只说明主机访问方式，不证明R1已接入；本次不新增SSH/设备操作。

## 分工与验证

三个原subagent分别负责hardware、host、review，各只写自己的HANDOFF；Root统一中央文档、证据、检查与Git。子任务完成比例见同目录文件，主包见[checklist](../../checklists/r02-r1-2026-10-14.md)。

仅执行网络公开只读和本地静态/文档检查，详见[validation](evidence/validation.json)。6份官方原件身份匹配，660个非Markdown项目输入与基线未变；前轮666项冻结输入只有Embedded说明一处文档差异。相对链接与diff格式通过。没有新源码逻辑，因此不重跑Cargo或添加重复实现的测试。上轮默认Embedded335/0/0、无默认260/0/0仍为历史，不声称本轮复测。未启动服务、数据库、Docker、SSH、PCM或执行器，没有新增runtime资源。ignored目录保留有界资料原件/读取记录，不留厂商程序或安装产物。

## 恢复顺序

从[R1检查点](../../resume-checkpoint-r02-r1-2026-10-14.md)开始，沿用用户已确认的硬件AEC优先路线，等待厂商资料与实物信息，再按真实输入确定端点/模式/声道。不得拿配置默认值填设备能力，不要重复实现已完成的Mac离线/时钟/证据框架。资料齐备后另定只读枚举包和真实音频窗口，最后才进行声学验收或有根据的软件consumer选择。R03须用户恢复；R05仍等待现场出口，六轮仍2/6。

用户回复后的[确认补记](evidence/confirmation-update.json)单独保存；原validation.json中的未回复状态属于首次验证时点，保留原记录。此次只同步决定与文档，不重跑官方读取或运行时验证。
