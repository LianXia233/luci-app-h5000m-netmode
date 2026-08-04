---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 9768052f19c9c9070fcb40014c0e726f_340abc088fa011f188ed525400287e28
    ReservedCode1: 9NsOYpNicN38AL65TaIco84YcU1C8jRB8NoThRdQ1o0p0K6jDkKUZc0mXv0YybCrA9MNsccP+QRewxgTLmjQU+iiODggrsMzbtiq4xpiDWR7j7iGRh06zDZEhWtSVlg+kXl761XNPaIfhZW4sj91inyu8SXqf2tCRWjOr2W1NFfgwDA7j4c1tfVcx4o=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 9768052f19c9c9070fcb40014c0e726f_340abc088fa011f188ed525400287e28
    ReservedCode2: 9NsOYpNicN38AL65TaIco84YcU1C8jRB8NoThRdQ1o0p0K6jDkKUZc0mXv0YybCrA9MNsccP+QRewxgTLmjQU+iiODggrsMzbtiq4xpiDWR7j7iGRh06zDZEhWtSVlg+kXl761XNPaIfhZW4sj91inyu8SXqf2tCRWjOr2W1NFfgwDA7j4c1tfVcx4o=
---

# 更新日志

## v1.4.0 — 2026-08-04

### 新增
- Web 界面每张出口卡片底部增加物理接口下拉选择，支持手动指定 WAN 和 5G 模组的 eth 设备
- 后端新增 `list-devices` 子命令：列出系统可用以太网设备（eth0/eth1/...）
- 后端新增 `get-device-map` 子命令：查看当前 WAN 与 5G 的物理接口映射
- 后端新增 `set-device-map {wan|modem} <device>` 子命令：通过 UCI 持久化接口映射
- UCI 新增配置项 `h5000m_netmode.settings.wan_device` 和 `h5000m_netmode.settings.modem_device`
- 手动接口映射在状态采集时覆盖自动发现结果，未设置时回退原行为

### 变更
- LuCI 前端 `netmode.js`：移除原有的 ETH fallback 芯片选择面板，改为每张卡片内嵌下拉框
- 前端全部中文硬编码，不再依赖 LMO 翻译文件
- 界面事件模型优化：下拉框 click/mousedown 事件阻止冒泡，防止与出口卡片选择冲突

### 修复
- 修复下拉框点击冒泡导致意外触发出口选择的问题

---

## v1.3.1 — 2026-08-02

### 新增
- ETH fallback 接口手动选择面板（LuCI 前端芯片式多选）
- `eth-candidates` / `eth-fallback set` / `eth-fallback get` 子命令
- `h5000m_netmode.settings.eth_fallback` UCI 持久化

### 变更
- `discover_modem_interfaces()` 重写：以接口名为准（wan/wan6/loopback/lan 归 WAN，其余默认归 5G）
- 新增 `qmodem` 物理兜底（原仅支持 MT5700M）

### 修复
- 修复非标准 5G 模组 section 名（如 `2_1`）导致 `modem_present=0` 的问题

---

## v1.3.0 — 2026-07-30

### 新增
- 四种出口策略：wan_first / modem_first / wan_only / modem_only
- 卡片式 LuCI 交互界面
- IPv6 出口跟随 IPv4，防止双栈流量分裂
- daed 自动重载
- Hotplug 触发 reconcile
*（内容由AI生成，仅供参考）*
