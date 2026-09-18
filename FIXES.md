# 故障根因与排查手册

本文件记录 v1.5.0 / v1.6.0 及后续修正的缺陷的**根因**、判定依据与排查手法。变更摘要见 [CHANGELOG.md](CHANGELOG.md)，使用说明见 [README.md](README.md)。

写作原则：每条都以源码为准，给出「现象 → 危害 → 修复 → 为什么另一种看起来更简洁的写法是错的 → 可执行验证命令」。最后一项尤其重要：判断出口类问题永远要测**内核实际会从哪里发包**，而不是配置里写了什么。

---

## 目录

- [一、出口判定](#一出口判定)
- [二、IPv4 与 IPv6 的同出口约束](#二ipv4-与-ipv6-的同出口约束)
- [三、触发时机](#三触发时机)
- [四、并发写入](#四并发写入)
- [五、界面交互](#五界面交互)
- [六、接口通断分级](#六接口通断分级)
- [七、确定性测试](#七确定性测试)
- [八、上线自检清单](#八上线自检清单)
- [九、状态呈现](#九状态呈现)

---

## 一、出口判定

### 1.1 默认出口读错了路由表

**危险写法**（v1.4.0 及更早）：

```sh
ip route show default | head -1
```

`ip route show default` 在不带 `table` 参数时只输出 **main 表**。而 main 表里有默认路由，不等于内核会用那条路由。

只要系统上存在策略路由（mwan3、qmodem、VPN、daed 透明代理都会创建），真实出口由 `ip rule` 指向**其他路由表**，main 表那条默认路由只是一条从不被命中的残留记录。

| 场景 | 旧写法读到 | 内核实际使用 | 后果 |
| --- | --- | --- | --- |
| daed 透明代理在跑 | main 表的 WAN 默认路由 | 代理表的另一条 | IPv6 被对齐到 WAN，实际 IPv4 走的是别的出口 |
| mwan3 接管默认路由 | main 表首条 | mwan3 表按权重选出的那条 | 主备判定与实际出口相反 |
| 5G 与 WAN 同 metric | 取决于 dump 顺序 | 内核按规则选 | 判定在两条链路之间抖动 |

**修复**：改用 FIB 查询，问内核「发往这个地址的包会从哪个设备出去」：

```sh
fib_egress() {
	local family="$1" probe line dev
	if [ "$family" = "6" ]; then probe="$PROBE6"; else probe="$PROBE4"; fi
	line="$(ip "-${family}" route get "$probe" 2>/dev/null | head -n 1 || true)"
	dev="$(route_dev "$line")"
	[ -n "$dev" ] || dev="$(route_dev "$(best_default_route "$family")")"
	printf '%s\n' "$dev"
}
```

探针地址是公共 anycast（`PROBE4=1.1.1.1`，`PROBE6=2606:4700:4700::1111`）。`ip route get` 是**纯查找**：不发包、不产生流量，但完整走一遍 `ip rule` 与路由表选择，所以策略路由会被尊重。

> **为什么另一种写法是错的**：`ip route show table all | grep default` 看起来「把所有表都看了」，但它只是把所有表并列列出，**没有解析 `ip rule` 的优先级**，因此仍然回答不了「这个包会从哪出」——列出五条默认路由不会告诉你是哪一条生效。同理 `ip route get` 的回退路径也保留了（当查询失败时回落到最低 metric 的默认路由），所以单栈环境行为不变。

**验证**：

```sh
# 内核实际出口（尊重策略路由）
ip -4 route get 1.1.1.1 | head -1
ip -6 route get 2606:4700:4700::1111 | head -1
# main 表的默认路由（与上面不一致即说明存在策略路由）
ip -4 route show default
# 后端判定结果，egress4/egress6 应与上面第一条命令一致
/usr/sbin/h5000m-netmode status | grep -E '^(egress4|egress6|active4|active6)='
```

### 1.2 路由选择依赖 dump 顺序

**危险写法**：

```sh
ip route show default | head -1
```

除开读错表的问题，「取第一条」本身也不成立。多条默认路由的**输出顺序不属于任何内核接口契约**：metric 相同的多条 default 路由在 dump 中的顺序可以随内核版本、插入顺序变化。

| 现象 | 后果 |
| --- | --- |
| 两条默认路由 metric 相同 | 每次读取可能得到不同结果，`active4` 不稳定 |
| 每次 poll 触发一次重算 | 界面状态来回跳变，看到「主备反复切换」的假象 |

**修复**：显式按最低 metric 选路，不依赖 dump 顺序：

```sh
best_default_route() {
	local family="$1"
	ip "-${family}" route show default 2>/dev/null | awk '
		{
			metric = 0
			for (i = 1; i < NF; i++)
				if ($i == "metric") metric = $(i + 1) + 0
			if (!found || metric < best) { found = 1; best = metric; line = $0 }
		}
		END { if (found) print line }
	'
}
```

**判定逻辑**：未声明 metric 的路由按 `0` 处理（内核语义：无 metric 等价于最高优先级），因此一条无 metric 的默认路由会正确胜过 `metric 10` 的路由。

**验证**：

```sh
# 连续多次读取应完全一致
for i in 1 2 3 4 5; do /usr/sbin/h5000m-netmode status | grep '^active4='; done
```

### 1.3 netifd 的符号设备引用未展开

**现象**：netifd 对**子接口**（典型是 5G 模组的 IPv6 section，如 `2_1v6`）在 `ubus call network.interface.<sec> status` 中返回的 `l3_device` 不是物理设备名，而是**符号引用**：

```json
{ "l3_device": "@2_1", "device": "@2_1", "up": true }
```

**危险写法**：把这个字符串直接拿去和设备名比对。

| 路径 | 后果 |
| --- | --- |
| `route_owner("@2_1")` 与 `eth2` 比对 | 永不相等，返回 `other` |
| `active6` 被算作 `other` | 判定认为 IPv6 走的是「未知出口」 |
| 触发 IPv6 纠正流程 | 在其实一切正常的情况下反复改写 IPv6 配置（churn） |

**修复**：把 `@ref` 展开为真实 netdev 列表，并在 l3_device 为空或为符号引用时回落到 UCI 侧解析：

```sh
	case "$SEC_L3DEV" in
		''|@*)
			resolved="$(resolve_section_devices "$section" 0)"
			for dev in $SEC_L3DEV $resolved; do
				case "$dev" in @*) continue ;; esac
				netdev_exists "$dev" && SEC_DEVS="$(add_word "$SEC_DEVS" "$dev")"
			done
			SEC_L3DEV=""
			;;
		*)
			SEC_DEVS="$SEC_L3DEV"
			;;
	esac
```

> **为什么另一种写法是错的**：`sed 's/^@//'` 去前缀得到的是 **section 名**（`2_1`），不是 netdev（`eth2`）。两者在本机恰好不同名，因此这种「一行搞定」的写法看起来没报错、实际拿到的是错的值。要拿到物理设备必须再解析一次 UCI（`network.<sec>.device`）。此外一个 section 可以挂多个设备，只取第一个会在真实出口是第二个设备时判错，所以这里收集的是**列表**。

**验证**：

```sh
# 1. 直接看 netifd 返回了什么
ubus call network.interface.2_1v6 status | grep -E 'l3_device|device'
# 2. 后端必须把它归到 modem，而不是 other
/usr/sbin/h5000m-netmode status | grep -E '^(active6|modem_device|modem_devices)='
```

### 1.4 透明代理的 TUN 隧道被当成第三个出口

**现象**（实测于 H5000M，同时启用 5G 拨号与 daed 透明代理）：

```sh
/usr/sbin/h5000m-netmode status | grep -E '^(mode|egress4|egress6|active4|active6|split|daed_exit_state)='
# mode=wan_first
# egress4=eth2
# egress6=singtun0
# active4=modem
# active6=other
# split=1
# daed_exit_state=modem
```

界面因此持续显示红色「出口分流 · IPv4 走 5G 模组，IPv6 走 其他路由」并引导用户点击「对齐出口」——而两条协议族的流量实际都从 5G 模组出去。

**根因**：`route_owner()` 的判定方式是把传入的**网卡名**与两条上行链路的物理网卡列表比对：

```sh
	# 修复前
	if device_matches "$device" $WAN_DEVICES; then echo wan
	elif device_matches "$device" $MODEM_DEVICES; then echo modem
	else echo other
	fi
```

`singtun0` 是 daed / sing-box 的 TUN 设备（`cat /sys/class/net/singtun0/type` → `65534`，即 `ARPHRD_NONE`），它的名字不可能出现在上行网卡列表里，于是必然落到 `other`。而 `other` 又被 `split` 判定当作「与 `modem` 不同的出口」，分流就这么被算了出来。

| 环节 | 错误结论 |
| --- | --- |
| `route_owner("singtun0")` | 与 `eth1` / `eth2` 比对永不相等 → `other` |
| `ACTIVE6=other` | 判定认为 IPv6 走的是「未知出口」 |
| `split=1` | 界面报分流并推荐「对齐出口」 |
| 用户点「对齐出口」 | IPv6 被拉到 IPv4 出口，**故障不存在，却改动了用户的网络配置** |

这一条比 1.3 更隐蔽：1.3 是拿到了错的值（`@2_1` 未展开），本条的输入值本身完全正确（`singtun0` 确实是 IPv6 默认路由所在的设备），错的是**对这个值的解释**。

**修复**：按网卡内核类型识别隧道，并把隧道归属到代理实际使用的上行链路。

```sh
is_tunnel_device() {
	local device="$1" type
	[ -n "$device" ] || return 1
	[ -r "${SYS_CLASS_NET}/${device}/type" ] || return 1
	type="$(cat "${SYS_CLASS_NET}/${device}/type" 2>/dev/null || true)"
	case "$type" in
		65534|768|769|776|778) return 0 ;;
		*) return 1 ;;
	esac
}

route_owner() {
	local device="$1"
	[ -n "$device" ] || { echo none; return 0; }
	if device_matches "$device" $WAN_DEVICES; then
		echo wan
	elif device_matches "$device" $MODEM_DEVICES; then
		echo modem
	elif is_tunnel_device "$device"; then
		local proxy_exit
		proxy_exit="$(cat "$DAED_EXIT_STATE_FILE" 2>/dev/null || true)"
		case "$proxy_exit" in
			wan) echo wan ;;
			modem) echo modem ;;
			*) echo other ;;
		esac
	else
		echo other
	fi
}
```

同时 `print_status()` 的 `split` 判定改为比较**归属**而非网卡名：

```sh
	split=0
	if [ -n "$EGRESS4" ] && [ -n "$EGRESS6" ] \
		&& [ "$ACTIVE4" != "none" ] && [ "$ACTIVE6" != "none" ] \
		&& [ "$ACTIVE4" != "$ACTIVE6" ]; then
		split=1
	fi
```

> **为什么另一种写法是错的**：按名字硬编码隧道清单（`case "$device" in singtun0|tun0|wg0) ...`）看起来更直接，但它把判定绑在一个**用户可以改的字符串**上——换个代理、改个 tunnel 名，误报就会原样回来。`ARPHRD_NONE` 是内核给 TUN 的类型，与名字无关。同理，用 `ip -o link | grep tun` 之类的方式也不成立：那是又一次名字匹配。
>
> **为什么排除 `none`**：「完全没有默认路由」与「两个协议族走了不同上行」是两种不同的故障，后端已为前者准备了独立状态。若把它也归入 `split`，界面会同时给出两条互相矛盾的处置建议。这个排除项由 `test_real_family_split_is_still_reported` 与既有用例共同约束。

**验证**（本次修复的实证，含正负对照）：

```sh
# 1. 隧道确实按内核类型被识别
cat /sys/class/net/singtun0/type          # 期望 65534
# 2. 归属与分流判定
/usr/sbin/h5000m-netmode status | grep -E '^(egress4|egress6|active4|active6|split)='
# 修复前：egress6=singtun0 active6=other split=1
# 修复后：egress6=singtun0 active6=modem split=0
# 3. 代理的上行记录（隧道归属的依据）
cat /var/run/h5000m-netmode.daed-exit     # 期望 wan 或 modem

# 4. 回归：测试套件必须全绿，且新用例能抓出旧实现
sh tests/run_tests.sh                                    # 期望 176 checks, 0 failure(s)
sh tests/run_tests.sh /tmp/h5000m-netmode-OLD            # 旧后端：期望 4 failures
#    均为隧道用例：tunnel active6 follows the proxy exit / tunnel is not reported as a split
#    （正负对照的意义：新用例真的在测这件事，而不是恒过）
```

**渲染层复核**：`node tests/render_live.js <netmode.js> <实机 status 输出>` 把真实数据灌进 `statusPanel()`，逐区域打印页面文本，确认各区域不再互相矛盾（修复前 banner 为 alert「出口已分流」，修复后为 warn「有线 WAN 不可用，IPv4 与 IPv6 已一并切换至 5G 模组」）。

**边界说明**：隧道无 `daed_exit_state` 记录时仍归 `other`（例如代理刚启动、状态文件尚未写入）。这是保守选择——宁可短暂显示「其他路由」，也不猜一个上行。对应用例 `test_tunnel_without_a_recorded_exit_stays_other` 固化了这一行为。

---

## 二、IPv4 与 IPv6 的同出口约束

### 2.1 分流：两个协议族走不同上行

**现象**：IPv4 从有线 WAN 出、IPv6 从 5G 出（或反过来）。旧实现分别维护两个协议族的默认路由，没有统一的裁决者。

| 后果 | 说明 |
| --- | --- |
| 应用连接失败 | 双栈应用按 IPv6 优先，服务端看到的来源地址与 IPv4 会话不一致，风控或会话校验会拒绝 |
| 出口 IP 不一致 | 需要固定出口的业务（白名单、对端绑定）行为不可预测 |
| 排障困难 | 「网是通的，但某些站点就是打不开」，从单栈视角完全看不出问题 |

**修复**：把 IPv6 出口收敛为**一个决策函数 + 一个写者**。

> **判定语义（v1.6.0 之后修正）**：`split=1` 的含义是「两个协议族走了**不同的上行链路**」，因此比较对象是解析后的**出口归属** `ACTIVE4` / `ACTIVE6`，而不是原始网卡名 `EGRESS4` / `EGRESS6`。用网卡名比较只在「两个出口都是物理网卡」时才等价；一旦中间有 TUN 隧道（透明代理）、策略路由或 VPN，网卡名就不再等于上行链路。详见 1.4——本设备上 `egress4=eth2` 与 `egress6=singtun0` 不同，而两条族实际都从 5G 模组出去。

决策（`select_ipv6_desired`，取值 `wan` / `modem` / `off` / `keep`）：

```sh
	case "$ACTIVE4" in
		wan|modem) IPV6_DESIRED="$ACTIVE4" ;;
		other)
			case "$ACTIVE6" in
				wan|modem) IPV6_DESIRED="off" ;;
				*) IPV6_DESIRED="keep" ;;
			esac
			;;
		*) ... ;;
	esac
```

- `wan` / `modem`：IPv6 与现役 IPv4 出口一致。
- `off`：**任何**会把 IPv6 放到非现役 IPv4 出口的状态都解析为 `off`，即关闭两个受管 IPv6 默认路由，而不是让 IPv6 从错误的上行泄漏出去。
- `keep`：没有 IPv4 出口可比对时不动 IPv6。这一条是必要的保守分支——在 IPv4 尚未建立时若也解析为 `off`，会出现「插上网线后 IPv6 被永久关掉」的误伤。

写入（`apply_ipv6_desired`）只做一件事：把两个受管 IPv6 section 的 `defaultroute` / `auto` 设成目标值，用 `set_network_option_if_changed` 逐项比较，**只有真的变了才 commit**（`NETWORK_CHANGED`），避免每次 reconcile 都重写 UCI 并触发 netifd 重载。

纠正（`ipv6_diverged`）用于检测实际状态与要求不符：

```sh
	case "$ACTIVE4" in
		wan) [ "$ACTIVE6" = "modem" ] && return 0; return 1 ;;
		modem) [ "$ACTIVE6" = "wan" ] && return 0; return 1 ;;
		other) case "$ACTIVE6" in wan|modem) return 0 ;; *) return 1 ;; esac ;;
		*) return 1 ;;
	esac
```

**验证**：

```sh
# split 必须为 0；active4 与 active6 必须相等（这是「同一上行」的判据）
# 注意：egress4 与 egress6 是原始网卡名，经透明代理承载时二者可以不同而并非分流
/usr/sbin/h5000m-netmode status | grep -E '^(egress4|egress6|active4|active6|split|ipv6_desired)='
# IPv6 归属必须与 IPv4 出口一致
/usr/sbin/h5000m-netmode status | grep -E '^(ipv6_owner|wan6_defaultroute|wan6_auto|usbv6_defaultroute|usbv6_auto)='
```

### 2.2 切换时的中间态

**问题**：IPv6 出口切换若「先起新族、再拆旧族」，中间会存在**两条**受管 IPv6 默认路由。内核按 metric 或插入顺序在两者间选路，于是对端可能在切换瞬间看到两个来源地址。

**修复**：切换时**先拆旧族，再起新族**，保证任一时刻至多一条受管 IPv6 默认路由。

> **判定逻辑**：这不是性能优化，而是把「分流」从「窗口期可能发生」降级为「结构上不可能」。测试 `test_failover_moves_ipv6_and_drops_old_family_first` 断言了这一顺序。

---

## 三、触发时机

### 3.1 Hotplug 分类漂移导致漏触发

**现象**：链路切换后策略没有重算，直到某个无关事件碰巧触发了 Hotplug。

**根因**：旧 Hotplug 脚本自己判断「这个 section 是不是 5G 模组」，判据是 section 带 qmodem 的 `modem_config` 标记，或 `managed_by=mt5700m`。

| section 形态 | 旧脚本判定 | 后果 |
| --- | --- | --- |
| 带 `modem_config`（qmodem 标准布局） | 模组 | 正常触发 |
| 带 `managed_by=mt5700m` | 模组 | 正常触发 |
| 蜂窝 netdev 上的静态接口 | 都不是 | **不触发**，切换被漏掉 |
| 被改名或由其他管理器接管的 section | 都不是 | **不触发** |

同时，后端的状态读取器用的是**另一套**发现逻辑（`discover_modem_interfaces`）。两套逻辑对同一个 section 可以得出不同结论——这就是「漂移」。

**修复**：删除 Hotplug 里的分类逻辑，改为委托后端，让状态读取器成为唯一判据：

```sh
case "$ACTION" in
	ifup|ifdown) ;;
	*) exit 0 ;;
esac
[ -n "$INTERFACE" ] || exit 0

role="$(/usr/sbin/h5000m-netmode iface-role "$INTERFACE" 2>/dev/null || echo other)"
case "$role" in
	wan|modem) ;;
	*) exit 0 ;;
esac

( sleep 1; /usr/sbin/h5000m-netmode reconcile >/dev/null 2>&1 ) &
```

`iface-role` 的取值只来自后端（`wan` / `modem` / `other`），且对输入做了白名单校验（`''|*[!a-zA-Z0-9_.@-]*` 直接 `exit 64`）。

> **为什么另一种写法是错的**：把后端的发现逻辑「复制一份」到 Hotplug 里看似更省一次进程调用，但那正是漂移的来源——两处逻辑一旦有一边更新就会重新分叉。委托的代价是一次短命令，收益是分类永远只有一个定义。
>
> 另外，`reconcile` 放在子 shell 里延迟 1 秒执行是必要的：`ifup` 事件发生时 netifd 还没装好路由，立刻判定会得到「没有默认路由」的结论。延迟 1 秒让 netifd 完成路由安装。

**验证**：

```sh
# 分类必须与状态读取器一致
for s in wan wan6 2_1 2_1v6 lan loopback; do
	printf '%-8s -> %s\n' "$s" "$(/usr/sbin/h5000m-netmode iface-role $s)"
done
# 期望：wan/wan6 -> wan，2_1/2_1v6 -> modem，lan/loopback -> other

# 模拟一次 ifup 事件，稳定态下状态不应改变
ACTION=ifup INTERFACE=2_1 /etc/hotplug.d/iface/95-h5000m-netmode; echo "rc=$?"
sleep 2
/usr/sbin/h5000m-netmode status | grep -E '^(active4|active6|split)='
```

### 3.2 看门狗覆盖 Hotplug 的盲区

Hotplug 只能看到接口 up/down 事件。以下情况它**永远看不到**：

| 盲区 | 例子 |
| --- | --- |
| 锁竞争期间到达的事件 | 另一个 reconcile 正在跑，事件被丢弃 |
| 其他管理器改动默认路由 | mwan3 重算、VPN 重连、daed 切换 |
| 接口 proto-up 但路由消失 | 上游断线，接口仍报 up |
| 配置被手工改动 | `ip route del` 或 UCI 改完没 reload |

**修复**：procd 看门狗服务，固定周期重算：

```sh
	procd_open_instance
	procd_set_param command /usr/sbin/h5000m-netmode watch
	procd_set_param respawn 3600 5 5
	procd_set_param nice 10
	procd_close_instance
```

`run_watch` 先立即 reconcile 一次（补偿启动前的漂移），随后按 `watch_interval`（默认 10s）循环。看门狗可由 `h5000m_netmode.settings.watcher=0` 完全停用（`start_service` 在注册前检查该开关，因此不会留下空跑进程）。`nice 10` 降低优先级，避免与转发路径争抢 CPU。

**「零扰动」这一性质必须验证**：看门狗每分钟跑 6 次 reconcile，如果每次 reconcile 都重写 UCI 或触发 netifd 重载，设备会持续抖动。`apply_ipv6_desired` 的 `set_network_option_if_changed` + `NETWORK_CHANGED` 门控保证稳定态**一次都不写**，所以稳定态的总成本就是「两次短命令、零日志」。

**验证**：

```sh
# 1. 服务在跑
pgrep -f 'h5000m-netmode watch'
/usr/sbin/h5000m-netmode status | grep -E '^watcher='
# 2. 观察一分钟，日志里不应出现任何 IPv6 exit 变更或 reconcile 动作
logread | grep h5000m-netmode | tail -20
# 3. 稳定态下状态不应变化
for i in 1 2 3; do sleep 12; /usr/sbin/h5000m-netmode status | grep '^active4='; done
```

> 停用看门狗：`uci set h5000m_netmode.settings.watcher=0 && uci commit h5000m_netmode && /etc/init.d/h5000m-netmode stop`

---

## 四、并发写入

### 4.1 映射被模式变更覆盖

**根因**：UCI 每次 `uci set` 都会**重写整个配置文件**。旧实现里 `set-device-map` 在锁**外**执行，因此在「模式变更在飞」的时候保存接口映射，可能被模式变更那侧的 UCI 写入覆盖——表现为「点了保存，接口映射过一会儿自己变回去了」。

**修复**：所有写动作（模式、映射、eth-fallback）进入同一把锁：

```sh
if ! acquire_lock "$LOCK_WAIT"; then
	echo "another exit policy update is already running" >&2
	exit 2
fi
trap release_lock EXIT INT TERM
```

`acquire_lock` 支持等待，并且能**回收死锁持有者**：锁目录里的 pid 不存在或不是数字，就清掉锁继续，避免一次异常退出让功能永久失效。`reconcile` 默认等待 5 秒（`DEFAULT_RECONCILE_WAIT=5`），所以 Hotplug 事件撞上正在运行的 reconcile 时是**排队**而不是被丢弃。

前端侧同步修正了写入顺序（见 5.4）：映射先落盘，模式后写，因为模式变更末尾会触发 netifd 重载。

**验证**：

```sh
# 并发发起多个写入，全部应串行完成，且最终映射一致
( /usr/sbin/h5000m-netmode set-device-map wan eth1 ) &
( /usr/sbin/h5000m-netmode set-device-map modem eth2 ) &
wait
/usr/sbin/h5000m-netmode get-device-map
```

---

## 五、界面交互

### 5.1 单击卡片即禁用备用链路

**危险写法**（v1.4.0 前端）：

```js
	if (!this.selecting)
		order = [ kind ];
	else if (order.length === 1 && order[0] !== kind)
		order.push(kind);
	else if (order.length > 1)
		order = [ kind ];
```

`orderMode([kind])` 产出的是 `wan_only` / `modem_only`。也就是说：

| 用户操作 | 用户以为 | 实际结果 |
| --- | --- | --- |
| 单击「5G 模组」卡片 | 把 5G 设为优先 | 变成 `modem_only`，**有线 WAN 被移出路由表** |
| 双出口状态下单击任一卡片 | 调整优先级 | 直接降级为 only，**备用链路消失** |

这与页面自己的说明文字（「第一个为首选出口，第二个为备用出口」）直接矛盾。在本应用的主备场景下这是最危险的缺陷：一次误点就移除了故障接替能力，主链路一断就没有备份。

**修复**：单击 = 设为首选并**保留**备用；把「移出备用」拆成独立控件。

```js
	// A single click on a card sets that uplink as the *preferred* exit and keeps
	// the other one as backup.  It deliberately no longer collapses the policy to
	// an only-mode: that used to remove failover on one stray click.
	selectRoute: function(kind, ev) {
		...
		var next = kind === 'modem' ? 'modem_first' : 'wan_first';
		if (this.pendingMode === next && !this.selecting) return;
		this.selecting = true;
		this.pendingMode = next;
		this.repaint();
	},
```

only 模式改由卡片底部的「仅用此出口」按钮触发（`selectOnly`），并且当该链路已经是唯一出口时按钮显示为「已是唯一出口」且禁用，避免重复点击。

> **判定逻辑**：把「提升一条链路」与「删除一条链路」分成两个操作，是本次交互设计的核心决定。二者在旧代码里由「点击次数」隐式区分（第一次点=选择，第二次点=加回），这种编码方式既不可发现也不可撤销——用户无法从界面判断自己处在哪个状态。分开之后，破坏性操作有了明确的入口和确认感的文案。

**验证**（无头浏览器，脚本见 `tools/verify_ui.py`）：

- 单击 5G 卡片后，5G 变「1 · 首选出口」且 **WAN 仍是「2 · 备用出口」**（回归判据）；
- 单击 WAN 卡片的「仅用此出口」后，WAN 变「唯一出口 · 备用已禁用」；
- 未提交改动存在时「应用设置」变为可用。

### 5.2 通断判定的二值塌缩

**危险写法**：把 netifd 的若干信号压成一个布尔值。

```js
	var state = this.connectionState(present, (up4 === '1' || up6 === '1') ? '1' : '0');
```

**现象**：刚插上网线、接口正在拨号/协商时，`up` 还是 0，界面显示「已断开」。用户看到的是「网线插好了但显示断开」。

| netifd 信号 | 含义 | 旧界面 | 应为 |
| --- | --- | --- | --- |
| `available=1` | 有可用配置 | — | 已连接 |
| `pending=1` | 正在协商/等待 | 已断开 | **协商中** |
| `up=1` | 已建立 | 已连接 | 已连接 |
| `carrier=0` | 物理层无载波 | 忽略 | 仅提示（见 5.3/六） |

**修复**：分级判定，且明确 carrier 的角色（见第六节）。

```js
	connectionState: function(data, kind) {
		...
		if (present !== '1') return { label: _('未配置'), cls: 'idle' };
		if (up4 || up6) return { label: _('已连接'), cls: 'up' };
		if (available === '1' || pending === '1') return { label: _('协商中'), cls: 'pending' };
		if (carrier === '0' && kind === 'wan') return { label: _('网线未接'), cls: '' };
		return { label: _('已断开'), cls: '' };
	},
```

### 5.3 轮询覆盖未提交编辑

**现象**：改了下拉框里的接口但还没点「应用设置」，5 秒后它自己变回去了。

**根因**：状态轮询每 5 秒重置一次待提交状态：

```js
	if (!this.applying) {
		var dm = this.deviceMap || {};
		this.pendingDeviceMap = {};       // 无条件重置，用户的编辑被丢弃
		...
	}
```

**修复**：用 dirty 标记保护未提交编辑。

```js
	if (!this.deviceDirty) {
		var dm = this.deviceMap || {};
		this.pendingDeviceMap = { wan: dm.wan || '', modem: dm.modem || '' };
	}
```

`onDeviceChange` 置 `deviceDirty = true`，应用成功后清零。

> **判定逻辑**：轮询存在的意义是反映**设备真实状态**，不是重置**用户正在编辑的表单**。两者的边界就是「用户是否已经动过这个字段」。

### 5.4 并发写入顺序

**危险写法**：

```js
	return Promise.all(promises);   // set 与两次 set-device-map 同时发出
```

`set` 动作末尾会执行 `/etc/init.d/network reload`。并发发出时，映射写入可能在 reload 之后才落盘，被 reload 的 UCI 提交覆盖（与 4.1 是同一类问题的前端侧表现）。

**修复**：串行执行，且**映射先写、模式后写**：

```js
		var steps = [];
		if (curWanDev !== newWanDev && newWanDev)
			steps.push({ label: _('有线 WAN 接口'), args: [ 'set-device-map', 'wan', newWanDev ] });
		if (curModemDev !== newModemDev && newModemDev)
			steps.push({ label: _('5G 模组接口'), args: [ 'set-device-map', 'modem', newModemDev ] });
		if (modeChanged)
			steps.push({ label: _('出口策略'), args: [ 'set', this.pendingMode ] });

		var chain = Promise.resolve();
		steps.forEach(function(step) {
			chain = chain.then(function() { return fs.exec('/usr/sbin/h5000m-netmode', step.args); });
		});
```

每个步骤带自己的标签，失败时通知文案能指出是哪一步失败。

### 5.5 分流状态不可见

后端一直输出 `split`、`egress4`、`egress6`，但旧前端从未使用。结果是**用户完全无法察觉 IPv4 与 IPv6 走了不同出口**——而这正是本应用要保证的不变量。

**修复**：分流作为最高优先级的提示，并配一个可操作的出口。

```js
		// A split is the one state the user explicitly asked never to happen, so it
		// outranks every other message.
		if (data.split === '1') {
			return {
				text: this.splitSentence(data),
				cls: 'h5net-note alert'
			};
		}
```

文案由 `splitSentence()` 单点提供（`modeLabel()` 旁），因为同一句话在 banner 与 `exitVerdict()` 两处都要用：

```js
	splitSentence: function(data) {
		return _('出口已分流：IPv4 走 %s，IPv6 走 %s。部分应用会因出口不一致而连接失败，建议点击"对齐出口"。')
			.format(this.exitLabel(data.active4), this.exitLabel(data.active6));
	},
```

> **为什么必须抽成单一来源**：这句话原本在两处各写一份字面量。两份副本不会立刻出错，但只要有人改其中一处——哪怕只是补个标点——界面就会出现「同一状态两种说法」，而这类不一致在代码评审里几乎不可能被发现：两个位置相隔 200 行，且都各自读起来完全正确。

同时徽标在分流时变红，并显示「对齐出口」按钮（调用 `h5000m-netmode reconcile`）——它只做状态对齐，不改动用户配置的策略，因此可以安全地作为一键修复入口。状态条还显示了看门狗与健康探测是否在运行，让「自动接替能力是否在线」可见。

> **按钮的边界**：「对齐出口」会改动用户的网络配置，所以它只应在**确为分流**时出现。1.4 那个缺陷的全部危害正在于此——假的 `split=1` 让一个会写配置的按钮出现在不该出现的场合，点了之后 IPv6 真的被改掉。误报比不报更贵，因为不报只是看不见，误报会引导用户去破坏本来正确的配置。

---

## 六、接口通断分级

**目标**：判断一条链路是否**可用**，而不是它是否「看起来在 up」。

后端 `read_section_state` 采集四类信号：

| 变量 | 来源 | 语义 |
| --- | --- | --- |
| `SEC_AVAILABLE` | netifd `available` | 有可用配置 |
| `SEC_PENDING` | netifd `pending` | 正在协商 |
| `SEC_UP` | netifd `up` | 已建立 |
| `SEC_CARRIER` | `/sys/class/net/<dev>/carrier` | 物理层载波，**仅作参考** |

**carrier 为什么不能当断线判据**（实测于 H5000M）：

| 设备 | carrier / operstate | 实际情况 |
| --- | --- | --- |
| `eth0`（LAN 侧） | `carrier = 0` | LAN 完全正常工作 |
| 蜂窝 netdev | `operstate = unknown` | 上行正常转发 |

若把 `carrier=0` 当作「链路断开」，这台设备会持续误报 LAN 断开，并可能据此触发无意义的故障切换。因此 carrier 只在**已确认没有可用配置**之后才作为提示（「网线未接」），且仅用于有线侧。

同时 `WAN4_READY` / `WAN6_READY` 这类「就绪」判定**不**依赖 netifd 的 up，而是直接问内核**这个设备上是否真的存在该协议族的默认路由**：

```sh
	for dev in $WAN_DEVICES; do
		has_default_route 4 "$dev" && WAN4_READY=1
		has_default_route 6 "$dev" && WAN6_READY=1
	done
```

`has_default_route` 还排除了 `prohibit` / `unreachable` / `blackhole` 三种路由类型——它们形式上「有默认路由」，实际上不转发流量。

> **判定逻辑**：「接口 up」与「能用」是两件事。接口 up 但默认路由被删（上游断线、路由被其他管理器撤走）的情况在这台设备上真实存在，所以就绪度以**路由是否存在**为准，接口状态只用于展示分级。

**验证**：

```sh
# 每个受管设备的族就绪度
/usr/sbin/h5000m-netmode status | grep -E '_(ready|up|available|pending|carrier)='
# 该设备上是否真有该族默认路由
ip -4 route show default dev eth1
ip -6 route show default dev eth1
```

---

## 七、确定性测试

`tests/run_tests.sh` 覆盖的正是**不适合在生产路由器上手工制造**的场景：策略路由、metric 冲突、符号设备引用、外来 IPv6 默认路由、模组无 IPv6 接口、Hotplug 事件丢失。

被测脚本**原样运行**，只把 `uci` / `ubus` / `ip` / `jsonfilter` / `pgrep` / `logger` / `ifup` / `ifdown` / `ping` 与 sysfs 根替换为 mock（`tests/mockbin/*`，纯 shell 实现，无 Python 依赖）。所有写入落在临时场景目录内，**在运行中的设备上执行也不会碰到真实配置**。

替换依赖通过后端暴露的环境变量接缝完成，默认值与生产一致，因此生产行为不受影响：

| 接缝 | 默认值 |
| --- | --- |
| `H5000M_LOCK_DIR` | `/var/lock/h5000m-netmode.lock` |
| `H5000M_DAED_STATE` | `/var/run/h5000m-netmode.daed-exit` |
| `H5000M_HEALTH_STATE` | `/var/run/h5000m-netmode.health` |
| `H5000M_SYSFS_NET` | `/sys/class/net` |
| `H5000M_SELF` | `/usr/sbin/h5000m-netmode` |
| `H5000M_LOCK_WAIT` | 空（`reconcile` 时取 5） |

**运行**：

```sh
sh tests/run_tests.sh                                  # 在设备上：测已安装的后端
sh tests/run_tests.sh /tmp/h5net-verify/h5000m-netmode # 测指定路径的后端
```

当前状态：**21 组测试、176 项断言、0 失败**（在目标设备的 BusyBox ash 上实测通过）。

| 测试 | 断言的关键性质 |
| --- | --- |
| `test_fib_oracle_beats_main_table` | 策略路由下 FIB 判定优于 main 表 |
| `test_metric_order_is_not_dump_order` | 选路不依赖 dump 顺序 |
| `test_symbolic_device_reference_is_resolved` | `@2_1` 被展开为真实设备 |
| `test_proxy_tunnel_is_not_a_family_split` | 代理隧道不产生分流误报 |
| `test_proxy_tunnel_attributes_to_the_proxy_exit` | 隧道归属到代理实际使用的上行 |
| `test_real_family_split_is_still_reported` | 真实分流仍被报出（防削弱） |
| `test_tunnel_without_a_recorded_exit_stays_other` | 无代理出口记录时保守归 `other` |
| `test_split_egress_is_repaired_towards_ipv4` | 分流被纠正回 IPv4 出口 |
| `test_failover_moves_ipv6_and_drops_old_family_first` | 切换先拆旧族 |
| `test_no_ipv4_default_keeps_ipv6_untouched` | 无 IPv4 时不误关 IPv6（`keep`） |
| `test_missing_modem_ipv6_disables_ipv6` | 模组无 IPv6 时关闭该族 |
| `test_stable_state_produces_no_churn` | 稳定态零写入 |
| `test_status_is_read_only_and_complete` | `status` 只读且字段完整 |
| `test_iface_role_classification` | 分类与状态读取器一致 |
| `test_eth_fallback_sections_are_not_modem` | fallback section 不被误判为模组 |
| `test_manual_mapping_is_serialised` | 映射写入受锁保护 |
| `test_shared_device_is_reported` | 同一设备被两角色claim 时告警 |
| `test_usage_errors` | 参数校验与退出码 |
| `test_reconcile_lock_behaviour` | 锁等待与死锁持有者回收 |

### 7.1 新用例必须能被旧实现证伪

隧道相关的 4 个用例（`test_proxy_tunnel_*`、`test_tunnel_without_a_recorded_exit_stays_other`）在写完之后，用**旧版后端**跑了一遍：

```sh
sh tests/run_tests.sh /tmp/h5000m-netmode-OLD     # 旧实现：4 failures
sh tests/run_tests.sh                             # 新实现：176 checks, 0 failure(s)
```

```
FAIL tunnel active6 follows the proxy exit: expected [modem] got [other]
FAIL tunnel is not reported as a split: expected [0] got [1]
FAIL tunnel-wan active6 follows the proxy exit: expected [wan] got [other]
FAIL tunnel-wan is not reported as a split: expected [0] got [1]
```

**这一步不是形式**：新增用例最常见的失败形态是「恒过」——夹具搭得恰好让被测代码无需做出任何改动就能通过，于是它只是在增加断言计数。同时 `test_real_family_split_is_still_reported` 在**新旧两版均通过**，证明这次收紧没有把真实告警一并削弱。

---

## 八、上线自检清单

按顺序执行，全部满足才认为出口策略工作正常。

```sh
# 1. 同出口不变量：split 必须为 0，且 active4 与 active6 必须相等
#    注意判据是「归属」而不是网卡名——经透明代理承载时 egress4/egress6 会不同
/usr/sbin/h5000m-netmode status | grep -E '^(egress4|egress6|active4|active6|split)='

# 2. IPv6 归属与 IPv4 出口一致
/usr/sbin/h5000m-netmode status | grep -E '^(ipv6_owner|ipv6_desired)='
#    期望：ipv6_owner/ipv6_desired 等于 active4 的取值（wan 或 modem）

# 3. 内核实际出口与后端判定一致
ip -4 route get 1.1.1.1 | head -1
ip -6 route get 2606:4700:4700::1111 | head -1

# 4. 就绪度：现役出口的对应族就绪位必须为 1
/usr/sbin/h5000m-netmode status | grep -E '_(ready)='

# 5. 看门狗在线
pgrep -f 'h5000m-netmode watch' >/dev/null && /usr/sbin/h5000m-netmode status | grep '^watcher='

# 6. 稳定态零扰动：等待一个完整周期，日志不应新增
logread | grep h5000m-netmode | tail -5
sleep 25
logread | grep h5000m-netmode | tail -5

# 7. Hotplug 分类正确（2_1 是模组 section，因设备而异）
/usr/sbin/h5000m-netmode iface-role 2_1
/usr/sbin/h5000m-netmode iface-role wan

# 8. 状态呈现：页面值必须等于 status 值，图标静止必须等于该子系统无活动
uci show h5000m_netmode | grep health_check
cat /var/run/h5000m-netmode.health

# 9. 若设备同时跑透明代理：隧道不得被算作第三个出口
grep -c is_tunnel_device /usr/sbin/h5000m-netmode    # 期望非 0（后端已含该修复）
cat /var/run/h5000m-netmode.daed-exit                # 隧道归属的依据：wan / modem
#    期望：active6 与 active4 相等、split=0，即使 egress6 是 singtun0 这类隧道

# 10. 测试套件
sh tests/run_tests.sh
```

**故障切换演练**（会短暂影响网络，请在确认可接受中断时执行）：

```sh
# 有线侧断开：把 WAN 的 metric 调大，观察 IPv6 是否一并跟随到 5G
uci set network.wan.metric=100 && uci commit network && /etc/init.d/network reload
sleep 8
/usr/sbin/h5000m-netmode status | grep -E '^(active4|active6|split)='
# 期望：active4=modem 且 active6=modem，split=0
#     （若设备上有透明代理，egress6 可能仍是 singtun0，这是正常的——判据是 active6）

# 还原
uci set network.wan.metric=10 && uci commit network && /etc/init.d/network reload
```

---

## 九、状态呈现

这一章的缺陷都不影响转发，只影响**界面说的是不是真话**。它们值得单独成章，是因为一个说假话的仪表盘比没有仪表盘更危险：它把「看起来正常」当成了「确已正常」，让操作者放弃本该做的核对。

### 9.1 轮询重绘打断动画

**现象**：状态总览里的图标每 5 秒抖动一次——动画播到一半从头开始。

**危害**：动画一旦成为周期性闪烁，用户就会主动忽略它。此时「图标在动」不再意味着任何事，这一层信息等于失效。

**根因**：`repaint()` 用 `replaceChild` 整块重建面板。DOM 节点被替换后，附着其上的 CSSAnimation 从 0% 重新开始。轮询周期（5 秒）与动画周期（1.4–3.5 秒）不整除，于是每次重建都切在动画中间。

**修复**：引入渲染键比较。`renderKey(data)` 把参与渲染的全部字段拼成一个字符串，键未变化直接返回，不重建任何节点。

**为什么另一种看起来更简洁的写法是错的**：渲染键只放后端字段会漏掉本地编辑状态。用户点卡片后 `pendingMode` 变了，但后端数据在保存前完全不变，键不变化 → 界面不重绘 → 点击看起来失灵。所以键里必须同时含 `pendingMode`、`pendingDeviceMap`、`applying`、`deviceDirty` 与设备列表。同理，`render()` 首次挂载后要立即用当前值播种键，否则第一次轮询会多重建一次。

**验证**：

```sh
# 页面打开后静置 20 秒（跨过 4 个轮询周期），图标不应出现周期性抖动
# 再确认真实变化仍会立即反映：把探测关掉，链路检测图标应在一次轮询内变灰静止
uci set h5000m_netmode.settings.health_check=0 && uci commit h5000m_netmode
sleep 8
uci set h5000m_netmode.settings.health_check=1 && uci commit h5000m_netmode
```

### 9.2 图标与真实状态脱钩

**现象**：8 个动态图标无论设备处于什么状态都在播放动画，配色也固定不变。

**危害**：这是「装饰性动画」的典型形态——它持续制造「系统在正常工作」的观感，而这份观感与设备实况无关。链路已经断了，图标照样转。

**修复**：图标分两层。基色层保留每个子系统的固有色调（仅在健康时生效），状态层按真实字段覆盖配色，并用 `is-idle` 停止动画：

- 正常 → 保持基色，动画运行
- 降级（协商中 / 部分射频离线 / 探测异常）→ 琥珀色
- 故障（已断开 / 出口分流 / 无默认路由）→ 红色
- 未启用或无数据 → 灰色，**动画停止**

**为什么另一种写法是错的**：只换颜色、不停动画。用户看到红色但图标仍在流动，会判断为「正在恢复中」——而真相是这条路径当前完全没有活动。反过来，`is-idle` 必须是 `animation:none!important`，因为动画声明在 `.h5net .pulse` 这类两条类选择器上，状态类需要更高的权重才能压住它。

**验证**：页面上把某个子系统关掉，观察它是否既变色又停止。

```sh
uci set h5000m_netmode.settings.health_check=0 && uci commit h5000m_netmode
# 8 秒后：链路检测图标应为灰底、「未启用」，且无任何 CSSAnimation 在跑
uci set h5000m_netmode.settings.health_check=1 && uci commit h5000m_netmode
```

自动化版本见 `tools/verify_tiles.py`：它从设备抓一份 `status`，用与前端相同的规则推导出 8 个图标应有的文案与状态，再逐项与页面比对，并断言「`is-idle` 当且仅当没有动画在跑」。

### 9.3 只放裸 SVG 图形不会渲染

**现象**：8 个图标里 Wi-Fi 那个完全不显示，其余 7 个正常。

**根因**：该图标的 `<path>` / `<circle>` 外面没有 `<svg>` 包裹。这些标签在 HTML 解析器里属于未知元素（`HTMLUnknownElement`），不会被渲染。设计稿自身的 CSS 规则 `.svg-preview svg,.svg-preview>path` 说明原本就该有外层，属于源稿遗漏。

同一个坑在 LuCI 里还有第二个入口：`E()` 内部调用 `document.createElement()`，对 SVG 标签名同样只能得到 `HTMLUnknownElement`。因此 SVG 必须走 `innerHTML` 注入，让 HTML 解析器按 SVG 命名空间构建节点——用 `E('svg', ...)` 拼出来的图形一个都不会显示。

**验证**：注入后检查命名空间，而不是看「好像画出来了」。

```js
// Playwright / 浏览器控制台
[...document.querySelectorAll('#h5net-status svg')]
  .filter(s => s.namespaceURI !== 'http://www.w3.org/2000/svg').length
// 期望 0
```

### 9.4 默认关闭的诊断等于不存在

**现象**：`health_check` 出厂值为 `0`，唯一能发现「接口已 up 但实际不通」的功能从未运行过。

**危害**：这类假连接是最难排查的故障——所有 netifd 字段都报正常，`ip route` 也有默认路由，但流量出不去。为它准备的功能默认不开启，等于没有。

**修复**：改为 opt-out，与看门狗 `watcher` 统一语义：未设置即启用，只有显式 `0` / `off` / `false` / `no` 才关闭。

**为什么另一种写法是错的**：在 `uci-defaults` 里直接 `uci set health_check=1`，会连带覆盖「用户主动关闭过」的意图。`0` 这个值本身无法区分「旧版本的默认值」与「用户的决定」，所以必须带一次性迁移标记：首次迁移写入 `health_check_migrated=1`，此后即便重装也不再改动 `health_check`。

默认开启还引出一个必须一并解决的问题：看门狗每 `watch_interval`（默认 10 秒）复算一次，若每轮都探测，就是对蜂窝链路每 10 秒发一次 ICMP 去重新学一个几乎不变的值。因此结论缓存带时间戳，`health_probe_interval`（默认 60 秒）之内复用缓存直接返回；页面读的也是缓存值，界面照旧实时，链路不被高频占用。

**验证**：

```sh
uci show h5000m_netmode | grep health          # health_check=1 且带 migrated 标记
uci -q delete h5000m_netmode.settings.health_check
/usr/sbin/h5000m-netmode status | grep health_check    # 期望 1（未设置即启用）
uci set h5000m_netmode.settings.health_check=0
/usr/sbin/h5000m-netmode status | grep health_check    # 期望 0（显式关闭）
cat /var/run/h5000m-netmode.health                     # wan/modem/ts 三项，ts 用于节流
```

### 9.5 总览区块的标题行与出口标题争夺层级

**现象**：出口卡片上方的总览区块自带 `<h2>运行状态总览</h2>` 与一行说明，紧邻下方是出口卡片的 `<h2>网络出口</h2>`。两行标题的字号只差 2px（20px 对 22px），视觉上是两个并列的章节标题，读者却分不清哪个是页面主体——总览是读数区，出口卡片才是可操作的主体。

**根因**：把「区块需要一个名字」当成了默认前提。但这里的 8 个卡片各自带有名称、数值与说明文字，区块本身不需要标题来定义自己；标题行存在的唯一效果是与真正的章节标题争夺层级。徽标「随页面每 5 秒刷新」同理——刷新周期是实现细节，对读者没信息量。

**修复**：直接删除整个标题行（`<h2>` + 说明 `<p>` + 徽标 `<span>`），`statusTiles()` 只返回 `<div class="h5net-stat-grid">`。同步从样式表移除 `.h5net-stat-head`、`.h5net-stat-head h2/p`、`.h5net-stat-badge` 及 620px 小屏分支里对应的三条规则——删节点不删规则会留下永远不命中的死样式。

**注意**：`README.md` 与 `CHANGELOG.md` 里「运行状态总览」仍是这个区块的概念名（章节标题、功能表条目），保留；只有页面内的渲染标题被移除。文档与界面在这一层的差异是刻意的：文档需要索引名，界面不需要。

**验证**：

```sh
# 页面内不应再有该标题，样式表不应再引用 head/badge 选择器
grep -c "运行状态总览" htdocs/luci-static/resources/view/h5000m/netmode.js   # 期望 0
grep -c "h5net-stat-head\|h5net-stat-badge" htdocs/luci-static/resources/view/h5000m/netmode.js   # 期望 0
grep -c "运行状态总览" README.md CHANGELOG.md   # 概念名保留，非 0
```

### 9.6 设计稿的组件演示页不是页面规格

**现象**：需求是把页头的状态胶囊换成设计稿里的出口卡片。设计稿给了左右两张卡——有线 WAN 与蜂窝网络——两张都标「在线」。

**根因**：那是组件展示页。组件展示页必须同时陈列两个变体，否则看不出两种配色。真机上同一时刻只有一条链路在承载流量，照搬会印出设备不可能处于的状态——这是本次改版要根除的那类缺陷的另一形态：图标在转但什么都没发生，对应到卡片就是「卡片说在线，但那条链路没在承载」。

**修复**：只渲染一张卡，五个位置全部取自 `status`。四个分支各有明确文案与配色，`split=1` 升级为红色「出口分流」。

**验证**：四分支在真机上不可达，必须在设备之外覆盖——见 9.7。

### 9.7 媒体查询不增加特异性：弱选择器写的响应式覆盖等于静默失效

**现象**：900px 视口下出口卡片没有按预期占满整行。规则写的是 `@media(max-width:900px){.h5net-ecard{flex:1 1 100%}}`，读起来完全正确。

**根因**：基础规则是 `.h5net .h5net-ecard{flex:0 1 auto}`（特异性 0,2,0），媒体查询里的 `.h5net-ecard{...}` 只有 0,1,0。**媒体查询本身不参与特异性计算**，它只是一个生效条件，所以弱选择器无论排得多靠后都会被基础规则压过。表现是：能解析、源码正确、浏览器里什么也不发生。

**修复**：覆盖规则带上与基础规则相同的作用域前缀（`.h5net .h5net-ecard{...}`）。

**排查手法**：这类缺陷在源码里读不出来，只能量实际几何值（卡片宽度、字号、图标尺寸）而不是断言「规则存在」。`tools/diag_ecard.py` 在 6 个断点量卡片宽度与文案是否被截断。

**为什么必须加静态检查**：破例留下一条弱选择器不会有任何报错。`tests/svg_audit.js` 因此增加一条规则——凡是触碰「基础规则中带 `.h5net` 作用域的类」的响应式选择器，必须自带同样的作用域。

**验证**：该检查经过注入回归确认不是空转——把一处作用域去掉后审计立即报 `FAIL ... unscoped: .h5net-ecard`，恢复后通过。

### 9.8 真机上不可达的分支必须离机覆盖

**现象**：出口卡片的蜂窝分支在设备上无法验证——把出口切成蜂窝会切断测试自己依赖的网络。

**根因**：把「设备此刻的状态」当成了「全部被测用例」。四个分支里只有一个是设备当前所处的，另外三个（含故障态与无出口态）如果只在真机上测，就永远不会被测到。

**修复**：`tests/test_exit_card.js` 以桩加载模块源码（原文件不修改），逐一构造四个分支并断言文案、配色、`is-idle`、图标变体。

**验证**：154 项断言 0 失败。这条测试在写的过程中抓出了两处写错的期望值——把「承载链路的角色」误当成「备用链路的角色」——恰好说明它在真的比对，而不是走过场。

### 9.9 把标题合并进卡片：两半必须共用一套词汇，且各说各的

**现象**：页头是「`<h2>网络出口</h2>` + 一行说明」与出口卡片两个并排的兄弟节点。标题侧是 22px 的纯文本，卡片侧是 15px 的标题加图标框，读者看到的是「一个标题旁边放了一张卡片」，而不是一个整体；标题侧也没有任何图形，旁边那张卡片却有。

**根因**：把「页面标题」与「状态读数」当成了两种东西。它们确实是两件事——一件是策略，一件是链路——但**分成两个节点实现**是另一回事：一旦分开，两侧就各自演化（字号 22 对 15、一侧有图标一侧没有、间距靠 `justify-content: space-between` 撑开），而它们描述的是同两条链路。

**修复**：合并为一张 `.h5net-ecard.hero`，内部两个 `.hero-slot`，两半都走同一套子结构（`.svgbox` + `.ec-content` + `.ec-topline`（标题 + 胶囊）+ `.ec-meta`）。左半补一枚按 `active4` / `active6` 绘制的动态 SVG。原来的说明文案移进左半底部（620px 以下隐藏以保住卡片高度），`.h5net-head` 及其两条规则、以及媒体查询里对它的引用一并删除。

**两个必须一并处理的连带问题**：

1. **胶囊颜色与卡片强调色解耦**。基础规则是 `.ec-live{color:var(--accent)}`，而两半的结论可以矛盾（链路在线、备用链路却已掉）。若胶囊继续跟随卡片强调色，左半的告警会被右半的健康态盖掉。改为胶囊自带状态类（`is-up` / `is-pending` / `is-down` / `is-off`），四者与原先由 `--accent` 推出的颜色一一对应，因此右半渲染与合并前逐像素一致。
2. **卡片配色仍只由链路决定**。把策略结论也映射到卡片 `tone-*` 上看似更「完整」，但那会让「仅用此出口」这种刻意的选择把整张卡片长期染成琥珀色，而琥珀从此不再意味着「此刻有问题」。策略的不利状态由左半胶囊自己承担。

**动效由卡片的类选择，不由拼接标记**：图标里哪一支链路在动，取决于卡片上的 `eg-via-wan` / `eg-via-modem` / `eg-via-both` / `eg-via-none`，而不是在 SVG 字符串里插入记号。原因是审计脚本靠**字符串字面量**重建标记：一旦写成 `'class="eg-flow' + (on ? ' on' : '') + '"'`，`class` 属性就被拆成多个字面量，审计再也读不到它，而「类名与规则是否对得上」正是审计存在的理由。代价是必须为两支链路各写一条独立的状态规则（`eg-via-wan .f-wan` 与 `eg-via-modem .f-modem`）——合并写成一条逗号选择器时，审计的 `lastClassOf` 只会取到最后一个选择器的类名，前一支会被判成「无动画规则」。审计另加一条反向断言：`eg-via-wan .f-modem` 这类**交叉绑定**必须不存在，否则图形会把流量画向错误的出口。

**验证**：

```sh
node tests/svg_audit.js htdocs/luci-static/resources/view/h5000m/netmode.js root/usr/sbin/h5000m-netmode
node tests/test_exit_card.js htdocs/luci-static/resources/view/h5000m/netmode.js   # 154 项断言
grep -c h5net-head htdocs/luci-static/resources/view/h5000m/netmode.js              # 期望 0
```

设备侧量的是几何值而不是「规则存在」：`tools/diag_ecard.py` 在 6 个断点确认两半的字号/字重/图标框尺寸与图标左缘一致（1280px 两半同为 564x75、15px/760、50x50；620px 以下同为 14px/760、46x46）、900px 起改为纵向堆叠、620px 以下附注隐藏；`tools/verify_tiles.py` 另行比对两半的每个字段与后端推导值，并核对图标标记的链路与 `active4` / `active6` 一致。

### 9.10 翻译目录整份失效，而代码看起来完全正确

**现象**：`po/zh_Hans/h5000m-netmode.po` 里躺着 69 条条目，内容完整、格式规整、`msgfmt` 校验通过；而页面上该翻译的地方仍然是原文。

**根因**：这 69 条描述的是**更早一版的界面**——`Exit Mode`、`Exit policy`、`Preferred exit`、`Mobile Network`、`Exit Priority` 等等。当前视图里的 112 条 msgid（含菜单 JSON 的 `title`）在目录中**一条都没有**。也就是说，这份目录对现行页面完全没有作用，它不是「部分落后」，而是「整体错位」。

| 检查 | 结果 | 说明 |
| --- | --- | --- |
| `msgfmt --check-format` | 通过 | 格式层毫无问题——这正是它长期没被发现的原因 |
| 条目数 | 69 | 看起来「有内容」 |
| 与当前 msgid 的交集 | 0 | 全部失效 |

**危害**：这类缺陷不会报任何错。构建通过、安装通过、目录文件在该在的位置、每条 msgstr 都非空——唯一的暴露方式是**看着页面读**。它比缺文件更隐蔽：缺文件会让人立刻去查，而一份错位的目录会让人以为「翻译这条路已经做过了」。

**修复**：由 `tools/sync_po.py` 从**视图源码与菜单 JSON** 直接生成目录，而不是靠人工维护。同时把「目录是否落后于源码」变成一条 CI 检查：

```sh
python3 tools/sync_po.py --check   # 落后即 exit 1
```

> **为什么另一种写法是错的**：靠人工在增删字符串时同步更新 `.po` 是行不通的——它要求每个人在改界面的同时记得改一个从不被编译验证的文件。检查必须能自动判定「源码里有而目录里没有」的 msgid，否则它只是在检查一个手工维护的列表与自己是否一致。
>
> **`--check` 要过滤 PO header**：目录开头的 `msgid ""` 是所有 `.po` 的固定头部，收集 msgid 时必须排除，否则它会一直被判为「多余条目」，检查永远失败。

**验证**（正负用例都已跑过）：

```sh
python3 tools/sync_po.py --check                       # 期望 exit 0
# 往视图里插入一条新字符串后
python3 tools/sync_po.py --check                       # 期望 exit 1
# 还原后
python3 tools/sync_po.py --check                       # 期望 exit 0

grep -c '^msgid ' po/zh_Hans/h5000m-netmode.po         # 期望 112（不含 header）
```
