# luci-app-h5000m-netmode

[![CI](https://github.com/LianXia233/luci-app-h5000m-netmode/actions/workflows/ci.yml/badge.svg)](https://github.com/LianXia233/luci-app-h5000m-netmode/actions/workflows/ci.yml)
[![Build Release](https://github.com/LianXia233/luci-app-h5000m-netmode/actions/workflows/release.yml/badge.svg)](https://github.com/LianXia233/luci-app-h5000m-netmode/actions/workflows/release.yml)
[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)
[![Latest Release](https://img.shields.io/github/v/release/LianXia233/luci-app-h5000m-netmode)](https://github.com/LianXia233/luci-app-h5000m-netmode/releases)

面向 Hiveton H5000M 的 OpenWrt 出口优先级管理器。通过卡片式 LuCI 界面，一键决定**有线 WAN** 与 **5G 模组**两条链路的启用范围与优先顺序，后端服务自动维护接口状态与默认路由。

- 当前 Release 版本：`v1.4.0`
- 版本格式：`主版本.次版本.修订版本-r打包修订`（GitHub Release 使用语义化标签，OpenWrt 安装包追加打包修订号）

---

## 目录

- [功能特性](#功能特性)
- [工作原理](#工作原理)
- [快速开始](#快速开始)
- [使用指南](#使用指南)
- [接口映射](#接口映射)
- [配置说明](#配置说明)
- [后端子命令](#后端子命令)
- [目录结构](#目录结构)
- [版本历史](#版本历史)
- [常见问题](#常见问题)
- [许可证](#许可证)

---

## 功能特性

| 特性 | 说明 |
| --- | --- |
| 四种出口策略 | `wan_first`（有线优先）/ `modem_first`（5G 优先）/ `wan_only`（仅有线）/ `modem_only`（仅 5G） |
| 卡片式交互 | 点击出口卡片即可切换策略，当前出口与链路状态即时反馈 |
| 链路自动恢复 | 接口 Hotplug 自动触发策略重算，链路恢复后无需人工干预 |
| 代理自动联动 | 默认 IPv4 出口变化时自动重新加载 daed，无需手动重新应用代理设置 |
| IPv6 出口约束 | 自动跟随 IPv4 出口，避免 IPv4 走 WAN、IPv6 意外走 5G 的双栈流量分裂 |
| 手动接口映射 | Web 界面下拉框手动指定 WAN / 5G 模组的物理接口，覆盖非标准接口命名场景 |
| 权限分离 | 只读状态查询与策略写入分权，普通监控账号无法改写出口策略 |
| 配置升级保留 | 升级时保留 `/etc/config/h5000m_netmode`，策略不丢失 |
| UCI 持久化 | 所有配置通过 UCI 持久化存储，重启后生效 |
| 隐私安全 | 不依赖云服务，不收集、不上传任何网络数据 |

---

## 工作原理

1. 用户通过 LuCI 卡片选择出口策略，前端调用后端写入 UCI 配置；
2. 后端根据策略维护 WAN / 5G 接口状态与默认路由（含 IPv6）；
3. 链路状态变化时，Hotplug 脚本（`95-h5000m-netmode`）自动触发重新计算；
4. 默认出口变化时联动重载 daed，保持代理链路一致。

---

## 快速开始

### 编译

```sh
git clone https://github.com/LianXia233/luci-app-h5000m-netmode.git \
  package/luci-app-h5000m-netmode
make menuconfig
# LuCI -> Applications -> luci-app-h5000m-netmode
make package/luci-app-h5000m-netmode/compile V=s
```

> 提示：GitHub Releases 中的软件包由 GitHub Actions 使用官方 OpenWrt SNAPSHOT `mediatek/filogic` SDK 在线构建，附带中文语言包、SDK 构建公钥和 SHA256 校验文件。软件包应安装到 ABI 匹配的近期 SNAPSHOT 固件。

### 安装

```sh
opkg install luci-app-h5000m-netmode_*.ipk
```

安装完成后刷新 LuCI 页面，进入 **移动网络 → 出口优先级** 即可使用。

### 卸载

```sh
opkg remove luci-app-h5000m-netmode
```

---

## 使用指南

### 出口策略

| 策略 | 说明 | 适用场景 |
| --- | --- | --- |
| `wan_first` | 有线 WAN 优先，WAN 不可用时自动切换 5G | 日常办公，追求稳定低延迟 |
| `modem_first` | 5G 优先，5G 不可用时自动切换有线 WAN | 追求移动网络带宽 |
| `wan_only` | 仅使用有线 WAN，禁用 5G 出口 | 有流量配额或安全要求 |
| `modem_only` | 仅使用 5G，禁用有线 WAN 出口 | 有线故障排查或场景隔离 |

### LuCI 界面

打开 **移动网络 → 出口优先级**，页面展示两张出口卡片：

- **有线 WAN 卡片**：显示当前链路状态，点击切换策略
- **5G 模组卡片**：显示模组在线状态与链路状态
- 卡片底部提供**物理接口下拉框**，用于手动指定出口对应的设备

---

## 接口映射

当有线 WAN 口命名非标准（如部分设备将真正的有线口注册为非 `wan` section），或 5G 模组接口名不被自动识别时，可通过每张出口卡片底部的下拉框手动指定物理接口：

| 操作对象 | 说明 |
| --- | --- |
| 有线 WAN 卡片 | 从可用 eth 设备列表中选择有线出口对应的物理口 |
| 5G 模组卡片 | 选择 5G 模组对应的物理口 |

- 选择后点击「应用设置」保存，配置通过 UCI `h5000m_netmode.settings.{wan_device,modem_device}` 持久化；
- 手动映射会**覆盖**自动发现结果；未设置时自动回退到原自动行为。

---

## 配置说明

### 配置文件

- 配置文件：`/etc/config/h5000m_netmode`
- 后端命令：`/usr/sbin/h5000m-netmode`
- LuCI 页面：**移动网络 → 出口优先级**

### UCI 配置项

| 配置项 | 类型 | 说明 |
| --- | --- | --- |
| `h5000m_netmode.settings.mode` | `wan_first` / `modem_first` / `wan_only` / `modem_only` | 出口策略（默认 `wan_first`） |
| `h5000m_netmode.settings.wan_device` | string | 手动指定的有线 WAN 物理接口（可选） |
| `h5000m_netmode.settings.modem_device` | string | 手动指定的 5G 模组物理接口（可选） |

配置示例：

```uci
config settings 'settings'
	option mode 'wan_first'
	option wan_device 'eth1'
	option modem_device 'eth2'
```

---

## 后端子命令

| 子命令 | 说明 |
| --- | --- |
| `list-devices` | 列出系统可用以太网设备（eth0/eth1/...） |
| `get-device-map` | 查看当前 WAN 与 5G 的物理接口映射 |
| `set-device-map {wan\|modem} <device>` | 设置并持久化物理接口映射 |

速查：

```sh
/usr/sbin/h5000m-netmode list-devices                    # 列出可用 eth 设备
/usr/sbin/h5000m-netmode get-device-map                  # 查看当前映射
/usr/sbin/h5000m-netmode set-device-map wan eth1         # 设置有线口
/usr/sbin/h5000m-netmode set-device-map modem eth2       # 设置 5G 口
```

---

## 目录结构

```text
.
├── .github/workflows/
│   ├── ci.yml                     # 持续集成
│   └── release.yml                # Release 自动构建
├── htdocs/luci-static/resources/view/h5000m/
│   └── netmode.js                 # LuCI 前端逻辑
├── po/zh_Hans/                    # 简体中文语言包
├── root/etc/
│   ├── config/h5000m_netmode      # UCI 配置文件
│   ├── hotplug.d/iface/95-h5000m-netmode   # 接口事件热插拔脚本
│   └── uci-defaults/90-h5000m-netmode      # 首次安装初始化
├── root/usr/
│   ├── sbin/h5000m-netmode        # 后端主程序
│   ├── sbin/h5000m-netmode-status # 状态查询
│   └── share/luci/menu.d/         # LuCI 菜单注册
├── scripts/build-release.sh       # 发布构建脚本
├── Makefile                       # OpenWrt 构建描述
├── README.md
├── CHANGELOG.md
└── LICENSE
```

---

## 版本历史

| 版本 | 日期 | 主要更新 |
| --- | --- | --- |
| [v1.4.0](CHANGELOG.md) | 2026-08-04 | 物理接口手动映射（卡片内嵌下拉框）；新增 `list-devices` / `get-device-map` / `set-device-map` 子命令 |
| v1.3.1 | 2026-08-02 | ETH fallback 接口选择面板；接口发现逻辑重写；新增 `qmodem` 物理兜底 |
| v1.3.0 | 2026-07-30 | 四种出口策略；卡片式 LuCI 界面；IPv6 出口约束；daed 自动重载 |

完整变更明细请查看 [CHANGELOG.md](CHANGELOG.md)。

---

## 常见问题

**Q：下拉框里看不到我想要的物理接口？**
先通过 `h5000m-netmode list-devices` 确认接口是否被系统识别；若接口存在但仍未出现在下拉框中，请确认固件与软件包 ABI 匹配。

**Q：手动设置的接口映射会被自动发现覆盖吗？**
不会。手动映射优先级高于自动发现，且通过 UCI 持久化；只有未设置手动映射时才回退到自动行为。

**Q：升级软件包后策略会丢失吗？**
不会。升级流程会保留 `/etc/config/h5000m_netmode`，策略与手动映射均会保留。

**Q：5G 模组状态显示异常（modem_present=0）？**
旧版本存在非标准模组 section 名（如 `2_1`）导致识别失败的问题，v1.3.1 起已修复，请升级到最新版本；如仍异常，可通过手动接口映射指定模组物理口。

**Q：IPv6 流量会走错出口吗？**
不会。后端会自动约束 IPv6 出口跟随 IPv4，避免双栈流量分裂。

---

## 许可证

本项目采用 [Apache License 2.0](LICENSE)。
