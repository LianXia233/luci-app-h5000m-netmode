# 更新日志

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/) 格式，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [v1.5.0] — 2026-09-14

本次发布修正了出口判定、IPv6 对齐与界面交互中的若干实质缺陷。根因分析与排查手法见 [FIXES.md](FIXES.md)。

### 新增

- 新增 procd 看门狗服务 `/etc/init.d/h5000m-netmode`：周期重算出口状态，覆盖 Hotplug 无法感知的场景（锁竞争期间丢失的事件、其他管理器改动默认路由、接口 proto-up 但路由消失）；稳定配置下不产生任何写入
- 后端新增 `watch` 子命令（看门狗主体）与 `iface-role <section>` 子命令（Hotplug 分类的唯一来源）
- 后端新增 `health` 子命令与可选的链路健康探测，探测目标为公共 anycast 地址而非下一跳
- UCI 新增 `watcher`、`watch_interval`、`health_check` 三个配置项
- 前端新增「仅用此出口」独立控件、「对齐出口」按钮，以及看门狗与健康探测的运行状态显示
- 新增 `tests/run_tests.sh` 与 `tests/mockbin/*`：135 项断言的确定性测试，可在设备上的 BusyBox ash 与 CI 的 dash 下运行

### 变更

- 出口判定改用 FIB 查询（`ip route get`）并用最低 metric 选路，取代只读 main 表且依赖路由 dump 顺序的旧写法
- IPv6 对齐改为单一写者（`apply_ipv6_desired`），切换时先拆除旧族默认路由再建立新族
- Hotplug 脚本不再自行判断 section 类型，改为委托后端，消除两处分类逻辑漂移导致的漏触发
- 接口通断判定改为分级（`available` / `pending` / `up`），网卡 carrier 仅作参考信号
- 所有写动作（含 `set-device-map`）纳入同一把锁串行执行
- 单击出口卡片改为「设为首选并保留备用」，不再降级为 only 模式
- 设备下拉框的未提交编辑不再被状态轮询覆盖

### 修复

- 修复策略路由（mwan3 / qmodem / VPN / daed）环境下默认出口判定错误的问题
- 修复 netifd 返回符号设备引用（如 `@2_1`）导致 IPv6 出口判定失真的问题
- 修复 IPv4 与 IPv6 可能走不同出口造成双栈分流的问题
- 修复模组 section 缺少标识字段时不触发 Hotplug 重算的问题
- 修复单击卡片即禁用备用链路导致的误触发
- 修复接口协商期间被误报为「已断开」的问题

## [v1.4.0] — 2026-08-04

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

## [v1.3.1] — 2026-08-02

### 新增

- ETH fallback 接口手动选择面板（LuCI 前端芯片式多选）
- `eth-candidates` / `eth-fallback set` / `eth-fallback get` 子命令
- `h5000m_netmode.settings.eth_fallback` UCI 持久化

### 变更

- `discover_modem_interfaces()` 重写：以接口名为准（wan/wan6/loopback/lan 归 WAN，其余默认归 5G）
- 新增 `qmodem` 物理兜底（原仅支持 MT5700M）

### 修复

- 修复非标准 5G 模组 section 名（如 `2_1`）导致 `modem_present=0` 的问题

## [v1.3.0] — 2026-07-30

### 新增

- 四种出口策略：wan_first / modem_first / wan_only / modem_only
- 卡片式 LuCI 交互界面
- IPv6 出口跟随 IPv4，防止双栈流量分裂
- daed 自动重载
- Hotplug 触发 reconcile
