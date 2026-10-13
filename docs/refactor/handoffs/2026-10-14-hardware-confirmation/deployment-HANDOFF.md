# 硬件与部署 — HANDOFF

日期：2026-10-14。`settings_auth_review` 更新部署资料并只读审阅音频/动作交接；本文件由 Root 归纳。实际 SSH 盘点由 Root 执行，不归为该 subagent 的实机工作。

## 已确认的输入

用户选定当前开发 Mac 运行 Server，Pi5/8 GB 运行 Embedded；用户给出地址后按 `ke-qiao@192.168.10.221` 使用。Pi 为 Ubuntu24.04.4 LTS、5 V/5 A DC，散热未提供。模型服务后续确认。R1 当前未接入，后续 USB 接 Pi；扬声器/功放待选。PCA9685A + MG996R 停止/断连保持，完成机构校准后允许启动归中；尚无校准资料。详细确认与未知项统一见 [部署清单](../../../operations/hardware-deployment-readiness.md)，不另造部署配置。

## 实际只读结果

- 首次不加载 SSH config 的批处理尝试 exit255（身份认证失败），使用既有配置身份后 exit0。两次保持严格已有 known_hosts 验证；未新增 host key、凭据、转发或本地 SSH 命令。
- Pi5 Model B Rev1.0、Ubuntu24.04.4、aarch64、kernel6.8.0-1065-raspi、7.8 GiB RAM；根盘约117 GiB/可用104 GiB。36.4℃仅单次温度，不证明散热长稳。
- USB 只有 root hubs，用户已说明 R1 未接入。内核有两张 HDMI 卡；账户对 `root:audio` 0660 的现有音频节点不可读写，ALSA 列举没有可访问声卡。没有录放音，不将 exit0 当音频就绪。
- 非交互 PATH 中未找到 rustc/cargo/protoc；只读补查发现 Rust shims，并列出 `stable-aarch64-unknown-linux-gnu (active, default)`。未查询固定 Rust 版本、未安装或变更 PATH；protoc 可用性仍待解决。
- I²C 节点存在，不代表 PCA 板卡在线。未扫描总线或打开执行器节点，串口稳定标识目录缺失不能推断 I²C 板卡缺失。

原始结果见 [索引](evidence/evidence-index.json)。当前主机确认不等于服务安装、设备绑定、模型或物理验收。

## 独立审阅与后续

三份文档已交叉核对：区分用户/历史/实测；先 R03-0 后 R03-A，R02-A 可并行；校准后归中仍未知初始机械位置；保持测试区分 stop、binding revoke、lease Close/到期，不将单个观察者掉线扩大成全局撤权，不把授权期限当物理停止上界。Root 已纳入本包。

- [ ] 固定服务身份、工作/配置/日志/数据库目录及最小权限。
- [ ] R1 接入后记录稳定身份、固件、PCM 和服务账户访问；按实际部署需要处理权限，当前未改组/ACL。
- [ ] 补散热、独立舵机供电/接线/OE/限位/校准、功放参考电气兼容性。
- [ ] 模型服务待用户后续确定；默认 probe=false，不自动启用付费探测。
- [ ] 部署恢复及 24 小时负载随 R05 实测，不套用历史容器结果。

本分工没有新生产代码、测试、安装或硬件动作；Root 只完成上述盘点。前端、通用 Embedded、端上模型和其它平台继续延期。
