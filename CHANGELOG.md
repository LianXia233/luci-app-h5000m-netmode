# 更新日志

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/) 格式，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [v1.8.5-r4] — 2026-09-28

本次修复健康探测与切换判定链的七个叠加缺陷。实机表现：`wan4_ready=0`、`addr4_wan=0`、
probe 恒 `unknown`（`attempts=3 ok=0 need=2`）、切换被 `ipv4_not_ready` 误判回滚、
看门狗不自动 failback；而手工 `ping -I eth0/eth2` 双双可达 —— 链路正常，判定链路坏死。

### 修复

- **IPv4 ping socket 未 bind ident（`src/probe/icmp.rs`）**：内核 ping socket 要求先
  `bind()` 到 ident 端口——未绑定时 `inet_sport=0`，socket 不进内核 ping 哈希表，
  `ping_lookup` 匹配不到任何回包（`SKB_DROP_REASON_NO_SOCKET` 全部丢弃），recv 必然
  超时，**v4 探测确定性 100% 失败**（与手工 ping 正常完全吻合）。现按 iputils 同款
  语义在 sendto 前 `bind(INADDR_ANY, htons(id))`，EADDRINUSE 时 id 递增重试（并发
  探测线程各持独立 ident）；IPv6 ping socket 同样补齐。这是 probe 恒 `ok=0` 的
  直接根因。
- **切换等待循环检查冻结快照（`src/switch/mod.rs`）**：`wait_group_family` 在循环中
  反复检查 worker 启动时读取的那份 LiveSnapshot，从不重读内核状态——目标接口若在
  切换开始后才就绪（warm/ifup 生效），等待循环永远看不到，15 秒超时后以
  `ipv4_not_ready` 回滚（用户看到的「切换失败：目标出口的 IPv4 未在限时内就绪」）。
  现在每轮轮询后 `read_live_state()` 刷新快照。
- **`group_complete` 无视 `strict_dual_stack`（`src/network/mod.rs`）**：该判定对每个
  capable 族（含 IPv6）要求结构就绪 + verdict up，与 strict=0 下切换状态机「v6 失败
  仅告警」的语义不一致——坏掉的 IPv6 把 `group_ready_*` / `group_online_*` 恒压 0，
  reconcile 的 failback 门（`group_complete_online`）因此永不放行，FIB 不迁移。
  现 strict=0 时 IPv6 不参与 group 判定（IPv4 是主备切换的裁决族），切换/回切/状态
  三处语义对齐。
- **IPv4 地址枚举架构性失效（`src/network/sysfs.rs`）**：原实现解析
  `/proc/net/fib_trie`，但该文件只给 scope 标签（`| /32 link LOCAL`），**根本不含
  设备名**，任何内核格式下都拼不出有效的 (dev, addr) 对 —— `dev_has_family_address`
  恒 false，`wan4_ready` / `modem4_ready` / `addr4_*` 全部钉死在 0。这是 probe 恒
  unknown（readiness 门拦截，探测从未执行）、切换在 WaitIpv4 阶段 15 秒超时后
  `ipv4_not_ready` 回滚的共同根因。现改为 `SIOCGIFCONF` ioctl 枚举（`ip addr` 的
  内核数据源），纯字节解析器 `parse_ifconf` 附跨平台单测。
- **IPv4 目标地址字节序双重转换（`src/probe/icmp.rs`、`src/probe/tcp.rs`）**：
  `u32::from_ne_bytes(octets()).to_be()` 在小端机上把目的地址反转（223.5.5.5 →
  5.5.5.223）。`octets()` 本身就是网络字节序内存序列，`from_ne_bytes` 后不得再转。
- **IPv6 回包解析越界偏移（`src/probe/icmp.rs`）**：raw IPv6 socket 的 recv 缓冲
  **不含 IPv6 头**（RFC 3542），原代码照搬 raw IPv4 习惯跳过 40 字节，读到包尾之外
  的零填充区，`icmp[0] != 129` 恒 false —— 即使回包在线也判 `ok=0`（对应日志
  `ipv6 probe failed: attempts=3 ok=0 need=2`）。现从缓冲头直接解析，并附
  `IPV6_CHECKSUM` 显式声明。
- **ping socket 受 `ping_group_range` 门控时无回退（`src/probe/icmp.rs`）**：DGRAM
  ping socket 创建失败（EACCES）时错误被吞成 false。现按 iputils 同款语义双路径化：
  ping socket 优先（内核按 ident 分发回包，type 命中即可信），失败自动降级 raw
  socket（回包含 IPv4 头按 IHL 剥离、按 type+ident 过滤并发探测者的流量）；接收
  循环以硬截止时间重挂 `SO_RCVTIMEO`，RA/NS 等无关 ICMP 不再污染判定，也不会
  挂死切换。

## [v1.8.5-r3] — 2026-09-28

### 修复

- **`json_bool` 无法解析 ubus 的 pretty JSON（`src/network/mod.rs`）**：ubus 返回的
  接口状态是带缩进的 JSON（`"up":\ttrue`，冒号后有空白），字段扫描没有跳过冒号后的
  空白，所有 `up` / `available` 布尔量在真实 netifd 上恒判 false —— `group_ready_*`、
  `group_online_*` 全 0，健康检查报 unknown，看门狗因此永远不触发 failback/救援。
  现读取前先 `trim_start()`，并附 pretty JSON 回归测试。

## [v1.8.5-r2] — 2026-09-28

### 修复

- **`uci_get` 三段式键查询恒返回空（`src/config/uci.rs`）**：键拆分误用
  `splitn(3, '.')`，第二次 `next()` 已把 section 名切出，`option` 恒为空串，
  所有 `cfg.section.option` 形式的读取（如 `network.wan.device`）在真实设备上
  恒返回空 —— `wan_device` / `modem_device` 因此保持空白，WAN 与 5G 的物理
  接口解析全部失效。现改为 `split_once` 逐段拆分（新 `split_key()`），并附
  三段/两段键的回归测试。`uci_has_key` 的同类隐患一并修正（三段键现在语义
  正确：检查 option 是否存在）。
  注：v1.8.5-r1 已包含 interface_sections 修复但被本缺陷掩盖了设备解析效果。

## [v1.8.5] — 2026-09-28

本次解决实机上一组「WAN 与 5G 全部显示 eth0 / 第三方路由接管 / 5G 硬件未配置」的连锁故障，
并让外部路由（mwan3 / VPN / daed 等）与物理出口监管解耦。

### 修复

- **设备发现根因（`src/config/uci.rs`）**：`interface_sections()` 此前按
  `opts.get("type") == "interface"` 过滤 UCI section，但 `parse_uci` 从不存储 section 头部
  的类型（`config interface 'wan'` 的 `interface`），该条件在所有真实设备上恒为假 ——
  network 的接口 section 永远发现不了，modem 分组永远不成立（页面显示「硬件未配置」），
  下拉框回退到前端伪造的 `eth0/eth1/eth2`。现改为直接解析 `config <type> '<name>'`
  头部（新增 `section_types()`），并附真实 network 文件的回归测试
  （`config device` section 与幽灵注释行不得泄漏）。
- **IPv6 不再回滚已成功的 IPv4 切换**：新增 `verify_commit()`，`strict_dual_stack=0`
  （新默认）下 `ipv6_*` 验证失败降级为告警；`reconcile` 与切换状态机的全部
  VERIFY_TARGET 校验点统一走该函数。
- **双角色冲突拒绝**：`set-device-map` 对「device 已绑定到另一角色」直接拒绝
  （exit 64 + 日志），前端同步改为本地拦截提示，不再静默把另一角色顶到别的接口。
- **日志可读性**：syslog 行改为 `[netmode] [<ts>] <LEVEL> <module>: <message>`，
  module tag 不再丢失；init.d 的 stderr 与 hotplug 通知接入 syslog。

### 新增

- **外部路由三字段状态契约**：`status` 新增 `external_route`（外部路由是否持有默认路由）、
  `external_route_source`（`tun` / 检测到的策略引擎名：mwan3、daed、sing-box、openclash、
  clash、passwall、homeproxy / `unknown`）、`external_physical_owner`（非隧道承载设备的
  角色归属）。插件不退场：外部路由只享调度权，物理出口主备监管仍归本插件。
- **前端如实展示**：`loadDeviceMap()` 删除本地猜测 fallback（`['eth0','eth1','eth2']` 与
  `{wan:'eth1',modem:'eth2'}`），后端不可达时显式提示；外部路由提示拆分为
  路由来源 / 实际物理承载两行结构化展示；`set-device-map` 失败时透传后端 stderr。

### 构建链

- `scripts/build-release.sh`：video feed（github.com 托管、本包无依赖）在解压后剔除；
  SDK 压缩包 sha256 校验通过时复用缓存；解压后把 `staging_dir/host/bin` 的悬空 symlink
  重新指向本机同名工具（官方打包机的 `/usr/bin/gcc` 在其他主机上不存在）；显式关闭
  `LUCI_CSSTIDY` / `LUCI_UTMIN`（本包无 css/ut 资源，csstidy 源码托管在 github.com）。
- `scripts/build-rust.sh` 产物随本版本重新提交（含上述全部后端修复）。

## [v1.8.4] — 2026-09-28

### 修复

- **对齐/救援路径补上族停靠**：`apply_group_holes` 此前只在 `set` 切换状态机里调用，
  `align_to`（一键对齐、看门狗 failback/救援走的同一条路径）提交单栈出口后从不停靠
  备用出口缺失族的默认路由，造成「IPv4 走新出口、IPv6 留在旧出口」的分流。现在对齐
  提交后同样停靠，并在停靠失败时判对齐失败。

## [v1.8.3] — 2026-09-28

本次解决「切换失败：目标出口没有 IPv6 接口（双栈出口要求，拒绝切换）」：IPv6 从此不再阻塞主备切换。

### 修复

- **IPv6 缺失不再拒绝切换**：族参与判定改为纯结构能力（目标出口有该族的接口成员才参与），
  `strict_dual_stack` 不再把结构性缺失的 IPv6 强行纳入要求集。没有 IPv6 成员的出口按
  单栈直接切换，预检不再产出 `ipv6_not_configured` 拒绝。
- **看门狗可以救援/回切到无 IPv6 的出口**：`group_complete` / `group_complete_online` /
  `group_degraded` 随判定同步改为按能力评估，此前 `strict_dual_stack=1` 会让健康检查、
  故障转移与救援全部无视一个「只有 IPv4 可用」的完好出口。
- **单栈提交后防分流**：`apply_group_holes` / `verify_excluded_families` 改为按能力执行
  （不再被 `strict_dual_stack=1` 跳过）——活动出口缺失的族，其备用出口默认路由会被停靠
  （`defaultroute=0` + 删路由），IPv4/IPv6 不会各自走不同上行。
- **`strict_dual_stack` 语义收敛为「IPv6 探测门禁」（默认 0）**：
  - `0`（新默认）：IPv6 等待超时、探测失败、提交失败、settle 后复核失败都只告警不回滚，
    IPv4 单栈照常切换/对齐；
  - `1`：恢复强双栈——目标出口有 IPv6 成员时必须等待、探测、提交全部通过，否则拒绝并回滚；
    但结构性缺失（根本没有 IPv6 成员）在任何取值下都不再拒绝。
- 前端 `ipv6_not_configured` 文案映射保留（兼容旧状态文件），后端不再产出该原因码。

### 兼容性

- 升级后默认行为变化：此前 `strict_dual_stack` 缺省为 1（无 IPv6 即拒绝切换），现缺省为 0。
  需要「IPv6 必须随行，否则宁可拒绝切换」的部署请在 UCI 显式设 `strict_dual_stack='1'`。
- 设备上已有配置若显式写有 `strict_dual_stack='1'`，升级后仍会生效（conffile 保留），需手工改 0。

## [v1.8.2] — 2026-09-28

本次为缺陷修复版本，修复 Rust 化之后出口切换在实机上不可用等一组问题。

### 修复

- **netlink 报文构造**（`src/network/netlink.rs`）：`struct rtmsg` 此前只写 9 字节
  （实际 12 字节，`rtm_flags` 是 `u32`），且 `rtm_type` 写 0（`RTN_UNSPEC`）。两者都会让
  内核以 `-EINVAL` 拒绝，而本程序没有任何 `ip route` 兜底路径，切换提交/回滚/热备路由
  全部依赖这一条路 —— 实机上切换会直接落到 `FAILED`。现已补齐 12 字节并写
  `RTN_UNICAST(1)`，并在临时网络命名空间内对真实内核逐项验证
  （NEWROUTE / 同 metric 换槽 / DELROUTE）。
- **`route_proto` 语义错误**：原来用 `RTM_GETROUTE` 做单次查询，而它是*查表*语义，
  内核返回的是"到某目的地的最佳路由"（实测返回了本地路由），并非 (设备, 网关, metric)
  指定的那条。改为 `RTM_GETROUTE + NLM_F_DUMP` 后在用户态按 (dst_len=0, oif, gw, metric)
  过滤，并修正 `rtm_protocol` 的读取偏移（rtmsg 偏移 5，原代码读的是偏移 8 的
  `rtm_flags`）。孤儿路由回收的 `is_route_boot` 守卫因此才真正生效。
- **`reap_orphan_routes`**：无网关的默认路由不再删除 —— 设备消失时内核已清掉其路由，
  而仅凭 metric 删除可能命中仍在使用的路由。
- **`uci` 写入补上超时**：`uci_exec` 此前用无界的 `output()`，与它自己的注释相反；
  `uci commit` 持有 UCI 文件锁，卡住会永久占用切换 worker。
- **子进程输出不再被截断**：`run_bounded` 以前先判进程退出再读管道，快命令（ubus）
  的尾部输出会被丢弃；现在退出后把管道切回阻塞并读至 EOF 再判定。
- **热插拔去抖标记不再可能永久残留**：标记目录写入持有者 pid，并在每次事件前清理
  "持有者已死"或"超过 4 倍去抖窗口"的陈旧标记。此前 notify 进程被 SIGKILL 后标记留在
  `/var/run`，后续所有接口事件都会被静默合并跳过，故障切换不再发生。
- **一键 align 真正绕过冷却**：`align_to` 此前把 force 硬编码为 false。
- **锁的 stale 竞态**：两个进程同时判定锁陈旧时会各建一次目录、同时持锁；现在建锁后
  回读 pid 确认持有者是自己，并限制 stale 清理次数。
- **`ubus call iwinfo assoclist`** 的设备名改为 JSON 转义后再拼接。

### 构建 / 工程

- 新增 `scripts/build-rust.sh`：从 `src/` 重建 `root/usr/sbin` 下的两个二进制。
- `scripts/build-release.sh` 在打包前自动执行上述编译，发布包内的后端始终来自当前源码。
- CI 新增一步：逐字节比对编译输出与仓库内的预编译产物，不一致时告警并上传本次产物。
- 健壮性收尾：`uci_exec` 改为组合式写法、`run_bounded` 排空管道时处理读取返回值、
  `acquire_lock` 加竞争上界（避免两个进程同时判定锁陈旧而反复重试）、netlink dump
  设置接收超时并显式忽略 `setsockopt` 返回值 —— 均为通过 `clippy -D warnings` 所需。
- `src/network/sysfs.rs`、`src/network/routes.rs` 新增 `H5000M_SYSFS_NET` /
  `H5000M_PROC_NET` 重定向，供确定性测试注入假内核状态。
- 新增 `tests/netlink_netns.py`：在临时网络命名空间内用真实内核验证 netlink 报文布局
  （新增/换槽/删除/协议号回读，8 项断言），并接入 CI。单元测试只能断言我们"造出的字节"，
  断言不了内核对这些字节的反应 —— 本次两个布局 bug 都属于后者。
- CI 新增 `autofix` 任务：main 分支上若 `src/` 的编译结果与仓库内的预编译产物不一致，
  或 `cargo fmt --check` 有差异，自动在同一提交里修正并回推。产物漂移与格式漂移从此不必
  依赖人工记得跑脚本（若仓库 Actions 令牌为只读则降级为警告）。
- CI 的格式检查不再拦住后续步骤：此前 `cargo fmt --check` 一旦失败，clippy、单元测试、
  真内核 netlink 验证会被全部跳过，一个空白符差异就能把真正的破坏藏住。现在 main 上降级
  为警告（由 `autofix` 修），PR 上仍是硬门禁；三步检查的输出另存为 `ci-logs` 产物。
- 修复 netlink 真内核验证在 GitHub runner 上的假失败：ubuntu-24.04 镜像默认开启
  `apparmor_restrict_unprivileged_userns`，非特权 `unshare -n` 被内核拒绝（EPERM）。
  CI 步骤内先放开该限制使检查真正运行；`tests/netlink_netns.py` 同时修正跳过判定 ——
  `unshare(1)` 自身失败时以非零退出码加 stderr 诊断结束、不抛 Python 异常，原来的
  `except` 接不住，导致"应跳过"的路径以 rc=1 让 CI 失败。现按 stderr 中的 unshare
  诊断识别为跳过，子进程检查真实失败仍照常报错。
- `src/probe/icmp.rs` 改用推断类型填充 `timeval.tv_sec`，移除对已弃用别名
  `libc::time_t` 的引用（musl 目标下的 clippy 告警；musl 1.2.0 起 `time_t` 为 64 位，
  推断写法对 glibc/musl 均正确）。
- 修复 SDK 发布构建（Build Release）失败：Rust 化后 `src/`（Cargo 工程）随打包脚本
  进入 SDK 包目录，命中上游 `luci.mk` 的 `ifneq ($(wildcard ${CURDIR}/src),)` 规则，
  `Package/install` 随之执行 `Build/Install/Default`（`make -C <build_dir> install`），
  而无 Makefile 的 Cargo 工程没有该目标，报 `No rule to make target 'install'`。
  本包按设计仅发布预编译静态产物（`root/usr/sbin`，构建环境无 Rust 工具链），
  现将 `src/`、`Cargo.toml`、`Cargo.lock` 排除出 SDK 包目录，并在 rsync 后加泄漏断言，
  泄漏在 SDK 构建前 1 秒内显式失败。
- `PKG_VERSION` 与 Cargo 版本同步升至 1.8.2。

### 已知遗留

- `root/usr/sbin/h5000m-netmode*` 仍是 v1.8.1 编译的旧产物（本次改动环境无 Rust 工具链，
  无法交叉编译）。它不包含上述任何修复，安装后仍会表现为切换失败。推到 main 后 CI 的
  `autofix` 任务会自动补上；或在本地执行 `scripts/build-rust.sh` 并连同产物一起提交。

## [v1.8.1] — 2026-09-28

版本号提升并触发 CI 编译：GitHub Actions 由 shell 检查改造为 Rust 编译工作流。

### 构建 / CI

- **CI 改为 Rust 编译工作流**：`cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` /
  `cargo test` / `cargo build --release --target aarch64-unknown-linux-musl`，并断言产物为
  `statically linked` 的 aarch64 ELF、`root/usr/sbin/*` 保持 100755。
- 保留前端检查：`node --check`、`jq empty`、`msgfmt`、目录同步检查、exit-card 测试；
  `svg_audit` 的字段契约核对对象由 shell 后端改为 `src/status.rs`。
- 剩余 shell 文件（init.d / hotplug.d / uci-defaults）继续 `sh -n` 检查。
- `PKG_VERSION` 与 Cargo 版本同步升至 1.8.1。

## [v1.8.0] — 2026-09-28

后端整体 **Rust 化重构**：`root/usr/sbin/h5000m-netmode` 与 `h5000m-netmode-status` 由 shell
脚本替换为单个 **aarch64 静态链接 Rust ELF**（musl，不依赖任何运行时），随插件包一次安装/卸载。
**前端零修改**：页面结构、菜单、中文文案、RPC 方法、参数与返回格式全部保持兼容。

### 重构

- **CLI / RPC 契约不变**：全部子命令（status / set / align / switch-worker / align-worker /
  notify / watch / reconcile / health / iface-role / list-devices / get-device-map /
  set-device-map / eth-candidates / eth-fallback / now-cs）行为、stdout 行格式与退出码
  （64 非法、3 忙排队、0 已启动）逐字节对齐原 shell 后端，LuCI 前端无需任何修改。
- **多文件模块架构**：`config / network / probe / health / switch / reconcile / state /
  system / status / rpc` 十个职责模块 + 极薄 `main.rs`，禁止单文件巨型后端。
- **读路径全部原生化**：路由/地址/网关状态直接读 `/proc/net/route`、`/proc/net/ipv6_route`、
  `/proc/net/fib_trie`、`/proc/net/if_inet6` 与 `/sys/class/net/*`；UCI 配置在进程内解析；
  仅剩每接口 up/available/pending、无线状态、关联列表三个 ubus 查询，且带 5s 上限。
- **写路径最小化**：网络指标写入经 `/sbin/uci`（保留 UCI 文件锁与 `config_change` 通知，
  低频）；路由切换走 **netlink**（RTM_NEWROUTE + NLM_F_CREATE|NLM_F_REPLACE 原子替代），
  不再派生 `ip` 命令。
- **worker 模型不变**：`set`/`align` 立即返回并派发后台 worker，LuCI 轮询 `status`；
  并发由 mkdir 锁 + pid + `/proc` 存活判定排他，请求 gen 最多拾取 3 轮。

### 新增

- **六层健康检测**（Rust 原生实现）：Link/carrier、Gateway、ICMP（IPv4 ping socket 与
  IPv6 ICMPv6 echo + 伪头校验和）、TCP connect（默认关闭）、DNS、HTTPS（预留扩展）；
  探测全部 **SO_BINDTODEVICE 绑定目标 WAN**，杜绝流量走错出口导致误判。
- **并行探测**：同族多目标、多族检查在线程池并行执行，任一成功即早退。
- **分层健康评分**（仅报告、不门控切换）：Link+20 / Gateway+20 / ICMP+30 / TCP+15 / DNS+15，
  状态 healthy / degraded / suspect / failed；自动切换仍沿用原连续失败门限语义。
- **探测目标国内化**：默认 `223.5.5.5 119.29.29.29`、IPv6 `2400:3200::1 2402:4e00::`，
  不把 114.114.114.114 作为强制目标。

### 优化

- **消除固定 sleep**：无 `sleep 1` 类秒级固定等待；全部改为条件轮询 + 预算上限
  （IPv4 15s / IPv6 20s / 总预算 60s / 探测 2/3 / 对齐冷却 20s / hotplug 合并 2s）。
- **减少 fork / exec**：状态轮询路径（LuCI 高频调用 `h5000m-netmode-status`）从
  「数百次 fork + 多次 ubus」降为「一次快照文件读取 + 条件命中零子进程」。
- **IPv4 / IPv6 同一事务**：`apply_policy` 双族同步写 metric / defaultroute / auto
  （auto 恒 1，IPv6 永不禁用）；`commit_family` 先提升目标（metric 10）再降旧主
  （metric 50），无空窗；回滚仅当旧主仍可用时才回。
- **快照缓存**：`h5000m-netmode-status` 以 `gen:applied_mode` 为代次标记，切换中直接
  重放 worker 的任务行，TTL 3 秒，冷路径才付一次完整 status。

### 构建 / 打包

- Cargo 工程：`libc` 唯一依赖，release 用 `opt-level=s + lto + panic=abort + strip`。
- 交叉编译验证：`aarch64-unknown-linux-musl`，`file` 显示 `statically linked`，
  `ldd` 显示 `not a dynamic executable`；主后端约 601 KB、状态程序约 397 KB。
- 插件包仍为单包安装：LuCI 前端 + Rust ELF + UCI 配置 + init + hotplug + RPC 一次装完；
  卸载时一并移除。

### 测试

- `cargo check` / `cargo test`（24 用例）/ `cargo clippy --all-targets`（0 警告）/
  `cargo fmt --check` / `cargo build --release` 全部通过；
- 本机冒烟：`now-cs` / `list-devices` / `health` / `status` 正常退出；
- 未连接 OpenWrt 实机，未进行 192.168.10.1 验证；毫秒级为目标，性能数据均基于静态
  分析与模拟，不代表实机实测。

## [v1.7.1] — 2026-09-28

修复主备切换的**锁竞争、完成判定与前端锁定窗口**，并恢复 CI 全绿。

### 修复

- **用户点击的切换可能被静默丢弃**：切换 worker 此前只等 2 秒拿写锁，而切换自身会触发 ifup / hotplug，
  hotplug 路径持有同一把写锁跑 reconcile，用户发起的切换于是被丢（`switch worker skipped: another
  writer holds the lock`），请求 gen 已前进但 `applied_mode` 未变，前端据此误报「切换完成」。
  现把等锁上限提到 10 秒；仍拿不到锁就写终态 `state=FAILED reason=lock_busy` 并刷新 started 时间戳，
  请求「要么执行、要么明确失败」，不再静默消失。
- **前端把 `set` 返回值当完成信号**：rpcd 的 `file.exec` 在延迟回复路径上存在竞态，同一条 `set` 可能
  <1s 返回、也可能吃满 30s 超时，返回值不可作为完成信号，按钮因此被锁住约 19s，同时提前认为任务已
  完成、卡片角色停留在旧模式。现在发出请求后最多等 1.5s 宽限，随后一律回设备读只读状态判定
  （`switch_started` 前进或 mode 已等于目标才算完成，避免把上一个任务的终态误判为本次完成），
  轮询收紧到 400ms，并新增 `lock_busy` 的中文提示。
- **状态快照缓存缺 `switch_*` 字段族**：`h5000m-netmode-status` 的快照缓存未覆盖新状态机的
  `switch_*` 字段，剥旧值后重放会把这些字段清空。现补齐字段族并为缓存标记代次（`gen:applied_mode`）。

### 修复（构建 / CI）

- **po 目录与源码脱节导致 CI 红**：新视图新增的 19 条 msgid（加载中、正在读取策略…、正在读取出口…等）
  未同步进 `po/zh_Hans/h5000m-netmode.po`，且残留 1 条 stale（`对齐失败：`），`sync_po.py --check`
  失败。重新生成目录（179 条）。
- **后端在 dash 上整体不可用**：`section_status_json` 的 memo key 消毒由 `tr -c` 改写成
  `${1//[^a-zA-Z0-9]/_}` 模式替换——这是 bashism：dash 在运行时报 `Bad substitution` 并以 rc=2 退出
  （`sh -n` 看不到），导致 status / switch 全部路径在 CI（dash）上不可用，且要等 po 检查修好后才会
  暴露。改回 POSIX 的 `tr` 写法，dash 与 BusyBox ash 均兼容。

### 测试

- `tests/run_tests.sh` 410 条断言全绿（含「前导零时钟仍能提交」回归）；`sync_po.py --check`、
  `check_catalog.py --self-test`、`msgfmt --check`、`svg_audit`、`test_exit_card`、全量 `sh -n`、
  `node --check`、`jq empty` 全部通过。

## [v1.7.0] — 2026-09-24

本次把「切换网络」从**重建接口**改成**移动默认路由优先级**，并把切换做成后端后台任务：点击立即返回、
页面只读状态轮询进度，中途失败自动回滚，全程不出现「两个出口同时不可用」的窗口。

### 新增

- **切换即换默认路由，不再 ifdown/ifup**：`set <mode>` 用 `ip route replace` / `ip -6 route replace`
  原子地把目标出口提升到活动槽位（metric 10）、旧出口降到热备槽位（metric 50）。旧出口全程可用，
  没有接口重建，没有 `network` / `firewall` 重启，IPv4 与 IPv6 同步移动。
- **强双栈出口门禁（`strict_dual_stack`，默认 1）**：目标出口必须 IPv4 与 IPv6 都通过结构就绪、连通性探测与
  切换后优先级复核才提交；没有 IPv6 的出口会被明确拒绝（`ipv6_not_configured`）并回滚，不再通过关闭
  IPv6 去凑一个单栈出口。
- **后台任务与进度反馈**：`set` 只创建任务后立即返回（rc 0；已有任务在跑时 rc 3 且本次请求排队），真正执行
  在后台完成；LuCI 通过只读 `status` 轮询状态机（切换中自动把轮询收紧到 1 秒），显示阶段、目标出口、
  两族当前出口、耗时与失败原因。HTTP/ubus 调用永远不会被切换阻塞。
- **有限等待 + 自动回滚**：IPv4 等待 15s、IPv6 等待 20s、总预算 60s、探测 2/3 次、失败门限 3 轮、
  对齐冷却 20s、Hotplug 合并窗口 2s，全部有上限。任何超时或提交失败都回到「原来那个完整双栈出口」，
  且只回滚**真正改动过**的部分。
- **显式状态机**：`PREPARING_TARGET → WAIT_IPV4 → WAIT_IPV6 → VERIFY_IPV4 → VERIFY_IPV6 → SWITCHING →
  VERIFY_TARGET → COMMITTED / ROLLBACK → FAILED`，每一步写状态文件（`switch_state` / `switch_message` /
  `switch_phases` / `switch_elapsed` / `switch_reason`），`status` 可直接读取。
- **热备与抑制振荡**：切换后旧出口保留 metric 50 的热备默认路由；健康探测需连续 `probe_fail_streak` 轮失败
  才判定出口降级并整体切换；回切需要 `align_confirm` 次连续确认；切换与对齐都受 `switch_cooldown` 限速，
  避免 WAN ↔ 5G 抖动。

### 修复

- **切换预算可能「开局即过期」，约十分之一的切换误判目标不可达**：`now_cs` 取 `/proc/uptime` 的百分位时，
  `08` 这类以 0 开头的两位字符串被 shell 当作**非法八进制**字面量，整条算术表达式失败、时钟输出为空，
  预算退化成 `$(( + 3000 ))`。此时探测在第一次尝试后就被预算检查打断（2/3 门限永远不满足），健康的目标被
  判「不可达」并回滚 —— 现象与「5G 掉线」一模一样。现按十进制剥离前导零；读不到时钟时取消预算，
  而不是伪造一个。
- **手动接口映射被静默丢弃**：`discover_groups` 应用手工映射后，`read_live_state` 会按 UCI section 重新填一遍
  同名设备字段，覆盖掉刚写入的映射。于是「绑定物理接口」下拉框保存的接口对判定、探测、路由比较全都不生效。
  现在手工映射在读取 section 状态**之后**应用。
- **失败的切换会对正常出口做无谓的路由搬家**：失败路径无条件执行回滚，即使失败发生在提交之前
  （目标未就绪 / 探测不通）也会对活动槽位做两次 replace 并把目标塞进热备槽位。现在只回滚真正提交过的部分，
  提交前失败不动任何路由。
- **只有一个族有默认路由时被误报为「出口分流」**，并把 `ipv6_owner` 写成 `split`；现在这种「另一个族压根没有
  出口」不再算分流，也不会去动它（更不会用删除 IPv6 默认路由的方式「对齐」）。
- **出口降级不切换**：活动出口的必需地址族连续失败达到 `probe_fail_streak` 且备用出口完整可达时，现在会把
  两个族整体切到备用出口；此前只记录健康度，出口一直留在半残链路上。
- 「一键对齐」成功后按实际承载两族的出口回写 `ipv6_owner`，不再残留 `split`。
- `gw_required` / `strict_dual_stack` 两个开关此前恒为真（`cfg_bool` 打印值但不返回状态），配置项形同虚设。

### 测试

- 测试套件重写为 `tests/run_tests.sh`，410 条断言全部通过，新增：切换提交与热备、目标未就绪不动旧出口、
  单栈目标被拒、半提交回滚、点击串行化（第二次点击返回 rc 3 并排队）、worker 死亡恢复、Hotplug 突发合并、
  单次探测失败不切换 / 连续失败才切换、对齐修复分流、时钟解析与「前导零时钟仍能提交」回归、
  以及「源码中不得出现 ifdown / network restart / IPv6 关闭类操作」的机制审计。
- 时钟解析与预算回归用例会钉住上面那个约 10% 的随机失败：它在修复前每次必现。

## [v1.6.5] — 2026-09-22

### 修复

- **重构透明代理 TUN 归属，只管理物理接口**：后端不再围绕 daed 状态文件维护代理出口，也不重载代理服务；
  HomeProxy、sing-box、daed 等创建的虚拟 TUN 只按本轮已判定的 IPv4 物理出口归属，避免把 TUN 当成第三个出口，
  同时确保切换网络 / 对齐出口只操作 WAN 与 5G 模组这些物理接口。兼容字段 `daed_exit_state` 保留并固定输出
  `none`。

## [v1.6.4] — 2026-09-22

### 修复

- **切换网络 / 一键对齐双栈出口时不再强制关闭备用出口 IPv6**：受管 IPv6 接口现在默认保持
  `auto=1`，重启后 `wan6` 与模组 IPv6 都可自动拉起；出口对齐只移动 `defaultroute`，并即时删除
  备用设备上的 IPv6 default route，避免备用 IPv6 接口被 `ifdown` 造成地址、会话和热备状态丢失。

## [v1.6.3] — 2026-09-20

本次修正一处**误报**：装了透明代理但没装 daed 的设备会永久显示「出口分流」，而「一键对齐双栈出口」
点了也毫无反应 —— 因为它要修的分流并不存在，真正出错的是判定本身。

### 修复

- **修复「一键对齐双栈出口」空转、隧道出口恒被误判为 `other`**：`run_watch` 每 10 秒执行一次
  `reconcile`，按钮走的也是同一条路径；但动作分发里的 `reconcile` 分支**提前 `exit 0`**，永远执行
  不到脚本尾部的 `reload_daed_on_exit_change`，而后者是**唯一**会创建
  `/var/run/h5000m-netmode.daed-exit` 的地方。于是该文件在「只跑 sing-box、没装 daed」的设备上从未
  被创建，`route_owner()` 对隧道设备的归因读不到记录，恒返回 `other`；`ACTIVE4=modem` 与
  `ACTIVE6=other` 不等，`split` 便永久为 1。现在把 `reload_daed_on_exit_change` 一并挂到
  `reconcile` 路径上（`run_reconcile()` 尾部）
- 该缺陷与 v1.6.2 同属一类：**逻辑本身正确，但调用路径不存在**，因而从不运行、不报错、也不留日志。
  实测该设备上带源地址的 `ip -6 route get` 显示 IPv6 物理出口本就是 `eth2`，与 IPv4 同路 —— 告警
  纯属误报，用户看到的「一键对齐无效」其实是后端判定无事可做
- 修复后 `active6=modem`、`split=0`、`daed_exit_state=modem`，页面提示由「出口分流异常」变为正常的
  降级说明，对齐按钮按设计自动隐去

### 变更

- **发布包不再压缩前端 JS**：`luci.mk` 的 `CONFIG_LUCI_JSMIN` 默认为开，发布包里的 `netmode.js`
  一直是 jsmin 的压缩产物（设备上 45124 字节 / 122 行，而仓库源码 49765 字节 / 1083 行），两者
  md5 天然不等，线上排障无法用 `diff` 直接比对源码，热更与回滚也只能重新打包。现在
  `scripts/build-release.sh` 同时关掉 `CONFIG_LUCI_JSMIN` 与 `LUCI_MINIFY_JS`，并新增构建后断言：
  在 `luci.mk` 写完、`mkpkg` 收纳之前的 `.pkgdir` 暂存目录里，把 `netmode.js` 与仓库源码逐字节
  比对，不一致即失败。代价是包体积增加约 22 KB
- 前端页面重写为 `.h5net-hero` / `.hero-*` 词汇，页头卡片改为左右两个同构半栏（左半报策略、
  右半报当前出口）

### 修复（构建）

- **压缩断言不再解 apk 容器**：断言最初直接 `tar -xzf` 解最终 `.apk`，而 OpenWrt 25.x 起
  apk-tools 3 的容器已换成 `ADB.pckg`（魔数 `ADBd`）—— 整包是一段 raw deflate，解出来是私有的
  段索引表，既不是 tar、也没有 gzip magic，`tar` 必然报 `gzip: stdin: not in gzip format` 并以
  退出码 2 结束。该失败与 JS 压缩开关无关，是**探测方式绑定了打包器格式**：上游一换格式，整个
  发布流程就整体红掉。现改为在 `.pkgdir` 暂存目录比对，该路径由 `luci.mk` 决定，与容器格式解耦
  （同级的 `ipkg-all` 收尾会被 `make` 清掉，而 `.pkgdir` 被显式保留，是稳定观察点）

### 测试

- `tests/svg_audit.js` 与 `tests/test_exit_card.js` 同步到新的类名体系与状态文案；两条测试各留一条
  守卫，防止已退役的 `eg-via-*` 三态出口动效被半途重新引入
- 后端套件 176 项、前端卡片 149 项、SVG 审计全部通过

### 注意

- 本次前端重写**移除了页头出口图标的 `eg-via-wan` / `eg-via-modem` / `eg-via-both` 三态动效**：
  旧版只有承载流量的那条方向路径在流动，新版两条始终同时流动，出口身份改由右半卡片的文字与
  配色表达。如需恢复该可视化能力，需连同测试断言一起加回

## [v1.6.2] — 2026-09-20

本次修正一处**静默失效**：看门狗服务在任何设备上都从未启动过，因为它的 init 脚本在仓库里没有执行位。

### 修复

- **修复 `/etc/init.d/h5000m-netmode` 缺少执行位导致看门狗从未启动**：该文件在 git 里记录为 `100644`，而 `/etc/rc.d/S95h5000m-netmode` 链接确实存在。procd 启动时 `execve` 失败，服务不启动，**且不产生任何日志**。实测设备上 `uci` 里 `watcher='1'`（配置已启用），但 `ps w | grep '[h]5000m-netmode'` 无进程、`ubus call service list` 里没有该服务、`/etc/init.d/h5000m-netmode status` 返回 `126`（Permission denied）、`h5000m-netmode status` 自报 `watcher=off`——README 与 FIXES 中反复依赖的「看门狗覆盖 hotplug 盲区」在实机上根本不存在。现把 init 脚本（以及同样被记成 `100644` 的 `tests/run_tests.sh`）改为 `100755`
- CI 新增「应可执行的脚本必须带执行位」断言。这类缺陷**无法**由任何「运行它」的检查发现：`sh -n` 与 `sh tests/run_tests.sh` 都是显式调用解释器，会绕过执行位；只能单独断言文件模式

## [v1.6.1] — 2026-09-20

本次修正一处**实机可见的本地化故障**：插件在中文环境下页面标题显示为英文。语言包装机正常、包管理器无告警、`msgfmt` 与既有 CI 检查全绿——故障因此长期隐身。

### 修复

- **修复页面标题在中文环境下显示英文（`Exit Priority`）**：标题文案定义在 `root/usr/share/luci/menu.d/luci-app-h5000m-netmode.json`，是英文 msgid，LuCI 通过 `.lmo` 语言包把它解析成中文。但 `po/zh_Hans/h5000m-netmode.po` 的 112 条条目**全部**是 `msgstr == msgid` 的恒等条目，而 `po2lmo` 会丢弃一切恒等条目（msgid 与 msgstr 同哈希，没有可查表的内容）；当结果目录零条目时它 `fclose()` 之后 `unlink()` 自己的输出并返回 0——语言包装得上、却**一个 `.lmo` 都没带**，`_()` 只能返回原文。v1.6.0 的发布包正是如此：i18n 包仅 980 字节、内容只有 `uci-defaults`，同族的 v1.4.0 是 2526 字节且带 `.lmo`。现在 `Exit Priority` / `Mobile Network` / `Ethernet` 有真实译文（出口优先级 / 移动网络 / 以太网）
- 修复出口卡片链路类型胶囊的英文无法被翻译：`badge` 取值是裸字面量 `'Ethernet'` / `'5G / LTE'`，从未经过 `_()`，因此语言包里写什么都只显示英文；现改为 `_('Ethernet')` / `_('5G / LTE')`（后者是跨语言通用的技术标签，刻意保持原样）

### 变更

- `tools/sync_po.py` 不再把所有条目写成 `msgstr = msgid`：新增显式 `TRANSLATIONS` 表，只有未列入的 msgid 才按恒等写出；`--check` 的比较由「只比 msgid 列表」改为「比 (msgid, msgstr) 对」，译文丢失会让 CI 失败，而不只是目录落后于源码
- 新增 `tools/check_catalog.py`：按 `po2lmo.c` 的实际规则复算哪些条目能在编译后存活，要求存活数不为零（否则产物被编译器自己删掉），并要求每个菜单标题都有非恒等译文；另带 `--self-test` 正负对照，证明这条检查真的会失败

### 测试

- CI 新增「语言包能否被 po2lmo 存活」检查（先跑 `--self-test` 再跑实际目录）；`scripts/build-release.sh` 在下载 SDK 之前跑同一对检查，发布流程因此不可能再产出「装得上、翻不出」的语言包
- 实测修复后编译产物 `h5000m-netmode.zh-cn.lmo` 为 92 字节 / 3 条存活条目（恒等条目按官方规则被丢弃）；实机页面标题由 `Exit Priority - OWRT` 变为 `出口优先级 - OWRT`，浏览器内 `_('Exit Priority')` 返回「出口优先级」
- 回归：设备后端套件 176 项断言 0 失败（BusyBox ash）、`tests/test_exit_card.js` 154 项 0 失败、`tests/svg_audit.js` 全通过

## [v1.6.0] — 2026-09-19

本次发布把界面从「渲染状态」推进到「承载状态」：出口卡片上方新增运行状态总览，8 个动态图标各自绑定一项真实后端字段；链路健康探测改为默认启用。

同一版本还修正一处**实机可见的误报**：设备同时使用拨号出口与透明代理时，页面把代理隧道判成第三个出口，于是 IPv4/IPv6 明明同走 5G 模组却持续显示「分流告警」，并引导用户去点「对齐出口」。同时重建了当时的翻译目录（该次重建本身引入了 v1.6.1 修复的恒等条目问题，详见 [FIXES.md](FIXES.md) 第十章）。

### 新增

- 界面新增**运行状态总览**：8 个动态 SVG 图标分别对应有线 WAN、5G 模组、无线 AP、转发出口、自动切换、IPv6 出口、链路检测与选路方式，图标旁的数值、配色与动画均取自 `status` 的实时输出，不做任何本地推断
- 后端 `status` 新增无线侧运行状态：`wifi_total`、`wifi_up`、`wifi_ssid`、`wifi_clients`（取自 `ubus call network.wireless status` 与 `ubus call iwinfo assoclist`；无无线模块的设备返回 0 且不影响 status 成功）
- 后端 `status` 新增 `watch_interval`，界面据此显示看门狗的真实周期
- 新增 UCI 配置项 `health_probe_interval`（默认 `60` 秒），限定两次健康探测的最小间隔；设为 `0` 表示不节流
- 出口卡片图标改为动态 SVG，并按链路分级状态换色、启停动画
- 页头新增**出口卡片**（取自设计稿 `.status` 组件）：说明当前实际承载流量的链路、真实网卡名与其在策略中的位置；备用链路接管时显式标注「已接管」，无出口或分流时降级为对应状态而不是沿用正常配色

### 变更

- **链路健康探测改为默认启用**（opt-out）：`health_check` 未设置即视为启用，只有显式写入 `0` / `off` / `false` / `no` 才关闭。旧版本写入的默认值 `0` 与「用户主动关闭」无法区分，因此由 `uci-defaults` 一次性迁移为 `1` 并落下迁移标记，迁移后再改回 `0` 不会被重装覆盖
- `health_check_enabled` 与看门狗 `watcher` 采用同一套 opt-out 语义，两个开关不再一个 opt-in、一个 opt-out
- 健康探测结论缓存新增时间戳，间隔内的复算复用缓存并直接返回，不再每 `watch_interval` 秒对蜂窝链路发起探测
- 图标静止成为有效结论：子系统未启用或无数据时转为灰底并停止动画，不再存在「持续转动却什么都没发生」的指示
- 运行状态总览不再渲染页内标题行（含 `<h2>`、说明文案与「随页面每 5 秒刷新」徽标）：每个图标自带名称、数值与说明，标题行只会与「网络出口」标题争夺同一视觉层级；同步从样式表中移除 `.h5net-stat-head` 与 `.h5net-stat-badge` 及其小屏规则
- 页头原有的胶囊状态徽标（`.h5net-active`）由出口卡片取代，其样式规则与 620px 分支一并移除；渲染键补充 `wan6_up` / `modem6_up`，避免 IPv6 侧通断变化时卡片不重绘
- 页头改为**一张合并卡片**：「网络出口」标题与出口卡片原本是两个并排的兄弟节点（标题是纯文本、旁边是卡片），现在是一张卡片的左右两半，共用同一套排版词汇。左半新增一枚按真实出口绘制动效的动态 SVG——左边路由器，右上有线插座、右下蜂窝信号格，两支链路各带一条流动虚线，**只有承载默认路由的那一支虚线流动并点亮对侧图形**，无出口或外部接管时整幅静止
- 两半各自陈述不同结论：左半是策略结论（主备就绪 / 无备用链路 / 单出口运行 / 策略被绕过 / 无默认路由 / 分流告警），右半是链路结论（在线 / 协商中 / 已断开 / 未配置…）。同一张卡片里写两遍「在线」只是同一条信息的副本，而「链路健康但备用已掉」恰恰是这套策略会悄悄失去的东西
- 状态胶囊的配色与卡片强调色解耦：两半的胶囊各带状态类（`is-up` / `is-pending` / `is-down` / `is-off`），映射与原先由 `--accent` 推出的颜色一一对应，右半渲染与合并前完全一致。卡片整体配色仍只由右半（链路）决定——否则「仅用此出口」这种刻意的选择会把整张卡片染成告警色，而那个颜色将不再意味着「此刻有问题」
- **重建 `po/zh_Hans/h5000m-netmode.po`**：原目录 69 条全部描述的是更早一版的界面（`Exit Mode` / `Exit policy` / `Preferred exit` 等），当前界面里已不存在这些字符串，而界面实际使用的 112 条 msgid 一条都没有——即翻译目录对现行页面完全无效。现由 `tools/sync_po.py` 从视图源码与菜单 JSON 直接生成，与代码保持一致（该脚本当时把每条都写成 `msgstr = msgid`，由此引入 v1.6.1 修复的恒等条目问题）
- CI 新增「翻译目录与源码同步」检查：`tools/sync_po.py --check` 在目录落后于源码时失败，避免它再次悄悄腐烂

### 修复

- 修复状态面板每 5 秒整块重建导致 SVG 动画从头重启、图标持续抖动的问题：引入渲染键比较，仅当参与渲染的字段（含未提交的本地编辑状态）发生变化时才重绘
- 修复设计稿中 Wi-Fi 图标缺少 `<svg>` 外层导致该图形完全不渲染的问题
- 修复出口卡片的响应式规则写成弱选择器（`.h5net-ecard{...}`）而被基础规则（`.h5net .h5net-ecard{...}`）压过、900px 以下完全不生效的问题：媒体查询不增加特异性，覆盖规则必须带同样的作用域前缀
- 修正出口卡片 `ring` / `wave` 两个关键帧的重名：本页已为总览图标定义了同名但语义不同的关键帧，直接照搬会静默改写总览图标的动效，卡片改用 `ec-ring` / `ec-wave`，渲染效果不变
- **修复透明代理隧道被误判为「其他路由」导致的分流误报**：状态里的出口归属由 `route_owner()` 判定，而它拿隧道的**网卡名**（如 `singtun0`，内核类型 `ARPHRD_NONE`）去和两条上行链路的物理网卡比对，永远不相等，于是 `active6` 变成 `other`，`split` 随之置 1。实测设备上 IPv4 默认路由走 `eth2`、IPv6 默认路由走 `singtun0`，而代理自身的出口记录（`daed_exit_state`）就是 `modem`——两条协议族实际都从 5G 模组出去。新增 `is_tunnel_device()` 按网卡类型识别隧道，并把隧道归属到代理正在使用的上行链路；不引入按名字硬编码的设备清单，因为设备名可由用户改动
- **修复 `split` 用网卡名而非出口归属作比较**：`split` 的语义是「两个协议族走了不同的上行链路」，因此比较对象应是解析后的归属（`active4` / `active6`），而不是原始网卡名。带代理时把 `egress6` 与 `egress4` 直接比较必然不等，修复前该判定在任何代理环境下都会假报分流。同时排除 `none`，让「完全没有默认路由」由它自己的状态表达，而不是伪装成分流
- 修复「出口已分流」提示文案在两处各写一份、容易互相漂移的问题：合并为 `splitSentence()` 单一来源

### 测试

- 断言数由 135 增至 162，新增无线状态解析、健康探测默认语义与探测节流三组用例
- 测试桩补全 `ubus call network.wireless status` 与 `ubus call iwinfo assoclist`，`jsonfilter` 桩支持通配路径（`@.*.up` 等）
- `tests/run_tests.sh` 支持按用例名过滤（第二个参数），便于单点复跑
- 新增 `tests/svg_audit.js`（Node，无需设备）：校验 14 个 SVG 片段平衡、16 个关键帧、动画类与规则双向绑定、状态色规则齐备、响应式选择器作用域、合并卡片的两半词汇齐备（含四个胶囊状态色规则、`h5net-head` 已彻底移除、窄屏改为纵向堆叠），以及**前后端字段契约**（前端读取的 34 个字段后端全部输出）
- 审计另行校验出口图标的状态绑定不被接错：每一支链路必须由各自的状态类点亮（`eg-via-wan .f-wan` 与 `eg-via-modem .f-modem`），交叉绑定会让图形把流量画向错误的出口；`eg-via-none` 刻意不设规则，默认渲染即「两支都不动」
- 新增 `tests/test_exit_card.js`（Node，无需设备，154 项断言）：出口卡片的四个分支中只有一个对应设备此刻的状态，其余分支若在真机上构造会改动路由，因此以桩加载模块源码逐一覆盖；并断言任何分支都不会复现设计稿「两张卡同时在线」的演示态。卡片合并后新增一节断言「一张卡片、两半」：卡片数为 1、半数为 2、每一半各自恰好一个图标框 / 标题 / 胶囊 / 详情行、两半的结论不得相同、图标标记的链路与 `active4` / `active6` 一致（连线不得交叉），以及策略结论的整条阶梯（分流 → 无默认路由 → 策略被绕过 → 单出口 → 无备用链路 → 主备就绪）
- CI 新增上述两个 Node 步骤，位于后端测试套件之前
- 新增 `tests/render_live.js`：把**实机抓取的 `status` 输出**灌进真实的 `statusPanel()` 渲染路径，逐区域打印页面会显示的文本。原有单测用桩数据，只能证明各分支内部自洽；本工具回答的是另一个问题——同一份设备数据经真实代码渲染后，各区域是否互相矛盾
- 断言数由 162 增至 176，新增 4 组用例覆盖隧道归属（归属到代理出口、不产生误报、真实分流仍被报出、无代理记录时保守归 `other`）与 `split` 判定

### 文档

- README 新增**界面预览**章节：H5000M 实机截图（`docs/preview.png`）配一张区块说明表，说明页头合并卡片、提示条、运行状态总览与出口卡片各自负责什么
- README「工作原理」改为 Mermaid 流程图：从卡片选择到 UCI 写入、看门狗复算、`ip route get` 判定出口、IPv6 收敛与 daed 重载，一条链路读下来；补充徽标（平台 / 前端 / 后端）与文末文档索引
- README 补充页头合并卡片两半的字段来源表，并新增三条相关常见问题（图标虚线在动表示什么、左半「无备用链路」与右半「在线」为何同时成立）
- FIXES 新增 9.9：把标题合并进卡片的动因、两个必须一并处理的连带问题（胶囊配色与卡片强调色解耦、动效由卡片状态类选择而非拼接标记），以及设备侧量了哪些几何值

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
