# H5000M Network Priority

[![CI](https://github.com/FAN789/luci-app-h5000m-netmode/actions/workflows/ci.yml/badge.svg)](https://github.com/FAN789/luci-app-h5000m-netmode/actions/workflows/ci.yml)
[![Build Release](https://github.com/FAN789/luci-app-h5000m-netmode/actions/workflows/release.yml/badge.svg)](https://github.com/FAN789/luci-app-h5000m-netmode/actions/workflows/release.yml)

面向 Hiveton H5000M 的 OpenWrt 出口优先级管理器。用户可直接点击有线 WAN 和
5G 两张出口卡片决定启用范围及优先顺序，服务会据此维护接口状态和默认路由。

## 功能

- 有线 WAN 优先、5G 优先、仅有线和仅 5G 四种策略
- 卡片式直接选择，当前出口和链路状态即时反馈
- 接口 Hotplug 自动重算，链路恢复后无需人工干预
- 自动约束 IPv6 出口，避免 IPv4 走 WAN、IPv6 意外走 5G
- UCI 持久化配置和简体中文 LuCI 界面
- 不依赖云服务，不收集或上传网络数据

## 编译

```sh
git clone https://github.com/FAN789/luci-app-h5000m-netmode.git \
  package/luci-app-h5000m-netmode
make menuconfig
# LuCI -> Applications -> luci-app-h5000m-netmode
make package/luci-app-h5000m-netmode/compile V=s
```

GitHub Releases 中的软件包由 GitHub Actions 使用官方 OpenWrt SNAPSHOT
`mediatek/filogic` SDK 在线构建，附带中文包、SDK 构建公钥和 SHA256 校验文件。
软件包应安装到 ABI 匹配的近期 SNAPSHOT 固件。

配置文件为 `/etc/config/h5000m_netmode`，后端命令为
`/usr/sbin/h5000m-netmode`，LuCI 页面位于“移动网络 → 出口优先级”。

本项目采用 [Apache License 2.0](LICENSE)。
