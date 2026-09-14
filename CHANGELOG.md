# 更新日志

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/) 格式，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [v1.6.0] — 2026-09-14

本次发布把界面从「渲染状态」推进到「承载状态」：出口卡片上方新增运行状态总览，8 个动态图标各自绑定一项真实后端字段；链路健康探测改为默认启用。

### 新增

- 界面新增**运行状态总览**：8 个动态 SVG 图标分别对应有线 WAN、5G 模组、无线 AP、转发出口、自动切换、IPv6 出口、链路检测与选路方式，图标旁的数值、配色与动画均取自 `status` 的实时输出，不做任何本地推断
- 后端 `status` 新增无线侧运行状态：`wifi_total`、`wifi_up`、`wifi_ssid`、`wifi_clients`（取自 `ubus call network.wireless status` 与 `ubus call iwinfo assoclist`；无无线模块的设备返回 0 且不影响 status 成功）
- 后端 `status` 新增 `watch_interval`，界面据此显示看门狗的真实周期
- 新增 UCI 配置项 `health_probe_interval`（默认 `60` 秒），限定两次健康探测的最小间隔；设为 `0` 表示不节流
- 出口卡片图标改为动态 SVG，并按链路分级状态换色、启停动画

### 变更

- **链路健康探测改为默认启用**（opt-out）：`health_check` 未设置即视为启用，只有显式写入 `0` / `off` / `false` / `no` 才关闭。旧版本写入的默认值 `0` 与「用户主动关闭」无法区分，因此由 `uci-defaults` 一次性迁移为 `1` 并落下迁移标记，迁移后再改回 `0` 不会被重装覆盖
- `health_check_enabled` 与看门狗 `watcher` 采用同一套 opt-out 语义，两个开关不再一个 opt-in、一个 opt-out
- 健康探测结论缓存新增时间戳，间隔内的复算复用缓存并直接返回，不再每 `watch_interval` 秒对蜂窝链路发起探测
- 图标静止成为有效结论：子系统未启用或无数据时转为灰底并停止动画，不再存在「持续转动却什么都没发生」的指示
- 运行状态总览不再渲染页内标题行（含 `<h2>`、说明文案与「随页面每 5 秒刷新」徽标）：每个图标自带名称、数值与说明，标题行只会与「网络出口」标题争夺同一视觉层级；同步从样式表中移除 `.h5net-stat-head` 与 `.h5net-stat-badge` 及其小屏规则

### 修复

- 修复状态面板每 5 秒整块重建导致 SVG 动画从头重启、图标持续抖动的问题：引入渲染键比较，仅当参与渲染的字段（含未提交的本地编辑状态）发生变化时才重绘
- 修复设计稿中 Wi-Fi 图标缺少 `<svg>` 外层导致该图形完全不渲染的问题

### 测试

- 断言数由 135 增至 162，新增无线状态解析、健康探测默认语义与探测节流三组用例
- 测试桩补全 `ubus call network.wireless status` 与 `ubus call iwinfo assoclist`，`jsonfilter` 桩支持通配路径（`@.*.up` 等）
- `tests/run_tests.sh` 支持按用例名过滤（第二个参数），便于单点复跑

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
