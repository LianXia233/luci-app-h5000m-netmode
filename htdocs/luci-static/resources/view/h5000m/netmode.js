'use strict';
'require view';
'require fs';
'require ui';
'require poll';

// Exit policy UI for the H5000M netmode backend.
//
// Three invariants drive every decision in this file:
//   1. IPv4 and IPv6 must leave through the same uplink.  A family split is a
//      fault to be surfaced, never a state to be presented as normal.
//   2. The backup uplink is only useful while it stays installed.  A single
//      click must never silently degrade a dual-exit policy into an only-mode
//      policy, because that removes failover with no confirmation.
//   3. Liveness is graded, not binary.  netifd distinguishes "available",
//      "pending" and "up"; carrier is advisory only.  Collapsing all of those
//      into connected/disconnected is what produced the false "disconnected"
//      reports this page used to show for a link that was still negotiating.

return view.extend({
	handleSave: null,
	handleSaveApply: null,
	handleReset: null,

	statusCommand: function() {
		return fs.exec('/usr/sbin/h5000m-netmode-status').catch(function() {
			return { stdout: '' };
		});
	},

	load: function() {
		return Promise.all([ this.statusCommand(), this.loadDeviceMap() ]).then(L.bind(function(results) {
			return results[0];
		}, this));
	},

	loadDeviceMap: function() {
		return fs.exec('/usr/sbin/h5000m-netmode', [ 'list-devices' ]).then(L.bind(function(res) {
			var devices = [];
			(res.stdout || '').trim().split(/\n/).forEach(function(name) {
				name = name.trim();
				if (name && devices.indexOf(name) < 0)
					devices.push(name);
			});
			this.availableDevices = devices;
		}, this)).catch(L.bind(function() {
			this.availableDevices = [ 'eth0', 'eth1', 'eth2' ];
		}, this)).then(L.bind(function() {
			return fs.exec('/usr/sbin/h5000m-netmode', [ 'get-device-map' ]);
		}, this)).then(L.bind(function(res) {
			var map = {};
			(res.stdout || '').trim().split(/\n/).forEach(function(line) {
				var pos = line.indexOf('=');
				if (pos > -1)
					map[line.substring(0, pos)] = line.substring(pos + 1);
			});
			this.deviceMap = map;
		}, this)).catch(L.bind(function() {
			this.deviceMap = { wan: 'eth1', modem: 'eth2' };
		}, this));
	},

	parseStatus: function(res) {
		var data = {};

		(res.stdout || '').trim().split(/\n/).forEach(function(line) {
			var pos = line.indexOf('=');
			if (pos > -1)
				data[line.substring(0, pos)] = line.substring(pos + 1);
		});

		return data;
	},

	styleNode: function() {
		return E('style', {}, [
			'.h5net{--net-blue:#4f8ff7;--net-green:#31b985;--net-amber:#e7a33e;--net-red:#e45f5f}',
			'.h5net-head{display:flex;justify-content:space-between;align-items:center;gap:18px;padding:2px 2px 14px;margin-bottom:14px;border-bottom:1px solid var(--border-color-low,#e8e8e8)}',
			'.h5net-head h2{margin:0 0 4px;font-size:22px;line-height:1.3}.h5net-head p{margin:0;color:var(--text-color-medium,#666);font-size:13px}',
			'.h5net-active{display:inline-flex;align-items:center;gap:7px;padding:6px 10px;border-radius:999px;background:rgba(49,185,133,.11);color:var(--net-green);font-size:12px;font-weight:600;white-space:nowrap}',
			'.h5net-active:before{content:"";width:7px;height:7px;border-radius:50%;background:currentColor}.h5net-active.warn{color:var(--net-amber);background:rgba(231,163,62,.11)}.h5net-active.fail{color:var(--net-red);background:rgba(228,95,95,.11)}',
			'.h5net-note{margin:0 0 14px;padding:10px 12px;border-left:3px solid var(--net-blue);border-radius:4px;background:rgba(79,143,247,.07);color:var(--text-color-medium,#555);font-size:13px}',
			'.h5net-note.alert{border-left-color:var(--net-red);background:rgba(228,95,95,.08);color:var(--net-red);font-weight:500}',
			'.h5net-note.warn{border-left-color:var(--net-amber);background:rgba(231,163,62,.08)}',
			'.h5net-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:12px}',
			'.h5net-card{position:relative;display:flex;flex-direction:column;padding:15px;border:1px solid var(--border-color-medium,#ddd);border-radius:11px;background:var(--background-color-high,#fff);cursor:pointer;user-select:none;transition:border-color .18s,box-shadow .18s,transform .18s}',
			'.h5net-card:hover{border-color:rgba(79,143,247,.6);transform:translateY(-1px)}.h5net-card:focus{outline:2px solid rgba(79,143,247,.35);outline-offset:2px}',
			'.h5net-card.selected{border-color:rgba(79,143,247,.72);box-shadow:0 0 0 2px rgba(79,143,247,.08)}.h5net-card.active{border-color:rgba(49,185,133,.65);box-shadow:0 0 0 2px rgba(49,185,133,.08)}.h5net-card.unselected{opacity:.66}',
			'.h5net-cardtop{display:flex;align-items:flex-start;justify-content:space-between;gap:10px}.h5net-name{display:flex;align-items:center;gap:10px}',
			'.h5net-icon{display:flex;align-items:center;justify-content:center;width:38px;height:38px;border-radius:10px;background:rgba(79,143,247,.10);color:var(--net-blue);font-size:12px;font-weight:700}.h5net-card.modem .h5net-icon{background:rgba(49,185,133,.10);color:var(--net-green)}',
			'.h5net-name h3{margin:0 0 2px;font-size:16px}.h5net-role{color:var(--text-color-medium,#777);font-size:12px}.h5net-card.selected .h5net-role{color:var(--net-blue);font-weight:600}',
			'.h5net-state{display:inline-flex;align-items:center;gap:6px;font-size:12px;font-weight:600;color:var(--net-red);white-space:nowrap}.h5net-state:before{content:"";width:7px;height:7px;border-radius:50%;background:currentColor}.h5net-state.up{color:var(--net-green)}.h5net-state.pending{color:var(--net-amber)}.h5net-state.idle{color:var(--text-color-medium,#888)}',
			'.h5net-protos{display:flex;flex-wrap:wrap;gap:7px;margin-top:15px}.h5net-proto{padding:5px 8px;border-radius:7px;background:var(--background-color-low,#f5f5f5);font-size:12px;color:var(--text-color-medium,#666)}.h5net-proto.current{background:rgba(49,185,133,.11);color:var(--net-green);font-weight:600}.h5net-proto.dead{background:rgba(228,95,95,.10);color:var(--net-red)}',
			'.h5net-device-row{display:flex;align-items:center;gap:8px;margin-top:10px;padding-top:10px;border-top:1px dashed var(--border-color-low,#e8e8e8)}',
			'.h5net-device-row label{font-size:12px;color:var(--text-color-medium,#777);white-space:nowrap}',
			'.h5net-device-row select{flex:1;padding:6px 8px;border:1px solid var(--border-color-medium,#ccc);border-radius:6px;background:var(--background-color-high,#fff);font-size:13px;color:var(--text-color,#333);cursor:pointer;outline:none;transition:border-color .15s}',
			'.h5net-device-row select:focus{border-color:var(--net-blue);box-shadow:0 0 0 2px rgba(79,143,247,.08)}',
			'.h5net-cardfoot{margin-top:11px;padding-top:10px;border-top:1px dashed var(--border-color-low,#e8e8e8);display:flex;justify-content:flex-end}',
			'.h5net-link{border:0;background:none;padding:3px 2px;color:var(--net-blue);font-size:12px;cursor:pointer;border-radius:4px}.h5net-link:hover{text-decoration:underline}.h5net-link[disabled]{color:var(--text-color-medium,#999);cursor:default;text-decoration:none}',
			'.h5net-foot{display:flex;justify-content:space-between;align-items:center;gap:14px;margin-top:14px;padding-top:14px;border-top:1px solid var(--border-color-low,#e8e8e8)}',
			'.h5net-meta{display:flex;flex-wrap:wrap;gap:12px;color:var(--text-color-medium,#777);font-size:12px}',
			'.h5net-meta b{font-weight:600;color:var(--text-color-medium,#555)}.h5net-meta .off{color:var(--net-amber)}',
			'.h5net-buttons{display:flex;gap:9px;flex-shrink:0}.h5net-buttons .cbi-button{min-width:104px}',
			'@media(max-width:620px){.h5net-head{display:block}.h5net-active{margin-top:11px}.h5net-grid{grid-template-columns:1fr}.h5net-foot{display:block}.h5net-buttons{margin-top:12px;flex-direction:column}.h5net-buttons .cbi-button{width:100%}}'
		].join(''));
	},

	exitLabel: function(exit) {
		if (exit === 'wan') return _('有线 WAN');
		if (exit === 'modem') return _('5G 模组');
		if (exit === 'other') return _('其他路由');
		return _('无可用出口');
	},

	modeOrder: function(mode) {
		if (mode === 'modem_first') return [ 'modem', 'wan' ];
		if (mode === 'wan_only') return [ 'wan' ];
		if (mode === 'modem_only') return [ 'modem' ];
		return [ 'wan', 'modem' ];
	},

	orderMode: function(order) {
		if (order.length === 1) return order[0] === 'modem' ? 'modem_only' : 'wan_only';
		return order[0] === 'modem' ? 'modem_first' : 'wan_first';
	},

	roleLabel: function(mode, kind) {
		var order = this.modeOrder(mode);
		var position = order.indexOf(kind);
		if (position < 0) return _('未选择');
		if (order.length === 1) return _('唯一出口 · 备用已禁用');
		return position === 0 ? '1 · ' + _('首选出口') : '2 · ' + _('备用出口');
	},

	// Graded liveness.  netifd's `available`/`pending` flags and the kernel
	// carrier flag say different things, and only the first two are authoritative
	// for "is this uplink usable".  Carrier is advisory: this hardware reports
	// carrier=0 on eth0 while the LAN is fully functional, and the cellular
	// netdev reports operstate=unknown, so carrier alone must never be reported
	// as a hard disconnect.
	connectionState: function(data, kind) {
		var present = data[kind + '_present'];
		var available = data[kind + '_available'];
		var pending = data[kind + '_pending'];
		var carrier = data[kind + '_carrier'];
		var up4 = data[kind + '_up'] === '1';
		var up6 = data[kind + '6_up'] === '1';

		if (present !== '1') return { label: _('未配置'), cls: 'idle' };
		if (up4 || up6) return { label: _('已连接'), cls: 'up' };
		if (available === '1' || pending === '1') return { label: _('协商中'), cls: 'pending' };
		if (carrier === '0' && kind === 'wan') return { label: _('网线未接'), cls: '' };
		return { label: _('已断开'), cls: '' };
	},

	// A single click on a card sets that uplink as the *preferred* exit and keeps
	// the other one as backup.  It deliberately no longer collapses the policy to
	// an only-mode: that used to remove failover on one stray click.
	selectRoute: function(kind, ev) {
		if (ev) ev.preventDefault();
		if (this.applying) return;

		var next = kind === 'modem' ? 'modem_first' : 'wan_first';
		if (this.pendingMode === next && !this.selecting) return;

		this.selecting = true;
		this.pendingMode = next;
		this.repaint();
	},

	// Explicit, separate control for the destructive case.  Taking one uplink out
	// of the routing table must be a deliberate act, so it lives on its own
	// button instead of hiding behind a card click.
	selectOnly: function(kind, ev) {
		if (ev) { ev.preventDefault(); ev.stopPropagation(); }
		if (this.applying) return;

		var next = kind === 'modem' ? 'modem_only' : 'wan_only';
		if (this.pendingMode === next) return;

		this.selecting = true;
		this.pendingMode = next;
		this.repaint();
	},

	cardKeydown: function(kind, ev) {
		if (ev.key === 'Enter' || ev.key === ' ') this.selectRoute(kind, ev);
	},

	onDeviceChange: function(role, ev) {
		if (ev) ev.stopPropagation();
		var dev = ev.target.value;
		if (!dev || this.applying) return;

		var otherRole = role === 'wan' ? 'modem' : 'wan';
		if (this.pendingDeviceMap[otherRole] === dev)
			this.pendingDeviceMap[otherRole] = this.pendingDeviceMap[role];

		this.pendingDeviceMap[role] = dev;
		// Remember that the user has an unapplied edit so the 5s status poll does
		// not silently revert the dropdown.
		this.deviceDirty = true;
		this.repaint();
	},

	deviceDropdown: function(kind) {
		var devices = this.availableDevices || [];
		var currentDev = (this.pendingDeviceMap || {})[kind] || '';

		return E('div', {
			'class': 'h5net-device-row',
			'click': function(ev) { ev.stopPropagation(); }
		}, [
			E('label', {}, _('接口')),
			E('select', {
				'change': L.bind(this.onDeviceChange, this, kind),
				'click': function(ev) { ev.stopPropagation(); },
				'mousedown': function(ev) { ev.stopPropagation(); },
				'disabled': this.applying ? 'disabled' : null
			}, devices.map(function(dev) {
				return E('option', {
					'value': dev,
					'selected': dev === currentDev ? 'selected' : null
				}, dev);
			}))
		]);
	},

	routeCard: function(kind, data) {
		var modem = kind === 'modem';
		var present = data[kind + '_present'];
		var up4 = data[kind + '_up'] === '1';
		var up6 = data[kind + '6_up'] === '1';
		// Readiness means "a default route for this family actually exists on one
		// of this role's devices", which is stronger than netifd's `up` flag and
		// is what the backend computes.  Only fall back to `up` when the backend
		// did not report the field at all.
		var ready4raw = data[kind + '4_ready'];
		var ready6raw = data[kind + '6_ready'];
		var ready4 = (ready4raw === undefined || ready4raw === '') ? (up4 ? '1' : '0') : ready4raw;
		var ready6 = (ready6raw === undefined || ready6raw === '') ? (up6 ? '1' : '0') : ready6raw;
		var order = this.modeOrder(this.pendingMode);
		var selected = order.indexOf(kind) > -1;
		var isOnly = order.length === 1 && selected;
		var active4 = data.active4 === kind;
		var active6 = data.active6 === kind;
		var state = this.connectionState(data, kind);
		var cls = 'h5net-card ' + (modem ? 'modem' : 'wan') + (selected ? ' selected' : ' unselected') + ((active4 || active6) ? ' active' : '');

		return E('div', {
			'class': cls,
			'role': 'button',
			'tabindex': '0',
			'aria-pressed': selected ? 'true' : 'false',
			'click': L.bind(this.selectRoute, this, kind),
			'keydown': L.bind(this.cardKeydown, this, kind)
		}, [
			E('div', { 'class': 'h5net-cardtop' }, [
				E('div', { 'class': 'h5net-name' }, [
					E('div', { 'class': 'h5net-icon' }, modem ? '5G' : 'WAN'),
					E('div', {}, [
						E('h3', {}, modem ? _('5G 模组') : _('有线 WAN')),
						E('div', { 'class': 'h5net-role' }, this.roleLabel(this.pendingMode, kind))
					])
				]),
				E('div', { 'class': 'h5net-state ' + state.cls }, state.label)
			]),
			E('div', { 'class': 'h5net-protos' }, [
				E('span', {
					'class': 'h5net-proto' + (active4 ? ' current' : (ready4 === '1' ? '' : ' dead'))
				}, active4 ? _('IPv4 使用中') : (ready4 === '1' ? _('IPv4 就绪') : _('IPv4 不可用'))),
				E('span', {
					'class': 'h5net-proto' + (active6 ? ' current' : (ready6 === '1' ? '' : ' dead'))
				}, active6 ? _('IPv6 使用中') : (ready6 === '1' ? _('IPv6 就绪') : _('IPv6 不可用')))
			]),
			this.deviceDropdown(kind),
			E('div', { 'class': 'h5net-cardfoot' }, [
				E('button', {
					'class': 'h5net-link',
					'type': 'button',
					'title': _('将该链路设为唯一出口，另一条链路会被移出路由表，故障时不会自动接替'),
					'disabled': (this.applying || isOnly) ? 'disabled' : null,
					'click': L.bind(this.selectOnly, this, kind)
				}, isOnly ? _('已是唯一出口') : _('仅用此出口'))
			])
		]);
	},

	statusMessage: function(data) {
		var mode = data.mode || 'wan_first';
		var preferred = (mode === 'modem_first' || mode === 'modem_only') ? 'modem' : 'wan';
		var fallback = preferred === 'wan' ? 'modem' : 'wan';
		var active4 = data.active4 || 'none';
		var active6 = data.active6 || 'none';
		var active = active4 !== 'none' ? active4 : active6;

		// A split is the one state the user explicitly asked never to happen, so it
		// outranks every other message.
		if (data.split === '1') {
			return {
				text: _('出口已分流：IPv4 走 %s，IPv6 走 %s。部分应用会因出口不一致而连接失败，建议点击"对齐出口"。')
					.format(this.exitLabel(active4), this.exitLabel(active6)),
				cls: 'h5net-note alert'
			};
		}

		if (active === 'none') {
			return {
				text: _('当前无默认路由可用。请检查网线或 5G 连接。'),
				cls: 'h5net-note alert'
			};
		}

		if (mode === 'wan_only' || mode === 'modem_only') {
			return {
				text: _('当前策略仅启用 %s，另一条链路已从路由中移除，故障时不会自动接替。')
					.format(this.exitLabel(preferred)),
				cls: 'h5net-note warn'
			};
		}

		if (active === fallback && active6 === 'none') {
			return {
				text: _('%s 不可用，IPv4 已切换至 %s；IPv6 保持禁用，避免流量分散到两个出口。')
					.format(this.exitLabel(preferred), this.exitLabel(fallback)),
				cls: 'h5net-note warn'
			};
		}

		if (active === fallback) {
			return {
				text: _('%s 不可用，IPv4 与 IPv6 已一并切换至 %s。')
					.format(this.exitLabel(preferred), this.exitLabel(fallback)),
				cls: 'h5net-note warn'
			};
		}

		if (active === preferred && active6 === 'none') {
			return {
				text: _('IPv4 正在使用 %s。该出口的 IPv6 不可用，备用 IPv6 已禁用以避免分流。')
					.format(this.exitLabel(preferred)),
				cls: 'h5net-note'
			};
		}

		if (active === preferred) {
			return {
				text: _('IPv4 与 IPv6 正一同使用首选出口 %s，备用出口将在需要时接替。')
					.format(this.exitLabel(preferred)),
				cls: 'h5net-note'
			};
		}

		return {
			text: _('当前流量走其他默认路由（可能由 mwan3、VPN 或 QModem 接管）。'),
			cls: 'h5net-note warn'
		};
	},

	metaBar: function(data) {
		return E('div', { 'class': 'h5net-meta' }, [
			E('span', {}, [
				_('自动看门狗：'),
				E('b', { 'class': data.watcher === 'on' ? '' : 'off' }, data.watcher === 'on' ? _('运行中') : _('已停止'))
			]),
			E('span', {}, [
				_('链路健康探测：'),
				E('b', { 'class': data.health_check === '1' ? '' : 'off' }, data.health_check === '1' ? _('已启用') : _('未启用'))
			]),
			E('span', {}, [
				_('IPv6 对齐策略：'),
				E('b', {}, _('跟随 IPv4 出口'))
			])
		]);
	},

	// Writes are serialised and ordered.  The device mapping is persisted first,
	// because applying a mode ends with a netifd reload, and a mapping written
	// while that reload is in flight can be lost when the reload commits UCI.
	applySelection: function() {
		if (this.applying) return;

		var self = this;
		var curWanDev = (this.deviceMap || {}).wan || '';
		var curModemDev = (this.deviceMap || {}).modem || '';
		var newWanDev = (this.pendingDeviceMap || {}).wan || curWanDev;
		var newModemDev = (this.pendingDeviceMap || {}).modem || curModemDev;
		var modeChanged = this.pendingMode !== ((this.liveData || {}).mode || 'wan_first');
		var deviceChanged = curWanDev !== newWanDev || curModemDev !== newModemDev;

		if (!modeChanged && !deviceChanged) return;

		var steps = [];
		if (curWanDev !== newWanDev && newWanDev)
			steps.push({ label: _('有线 WAN 接口'), args: [ 'set-device-map', 'wan', newWanDev ] });
		if (curModemDev !== newModemDev && newModemDev)
			steps.push({ label: _('5G 模组接口'), args: [ 'set-device-map', 'modem', newModemDev ] });
		if (modeChanged)
			steps.push({ label: _('出口策略'), args: [ 'set', this.pendingMode ] });

		this.applying = true;
		this.repaint();

		var chain = Promise.resolve();
		steps.forEach(function(step) {
			chain = chain.then(function() {
				return fs.exec('/usr/sbin/h5000m-netmode', step.args).catch(function(err) {
					throw new Error(step.label + '：' + (err.message || _('未知错误')));
				});
			});
		});

		return chain.then(L.bind(function() {
			ui.addNotification(null, E('p', _('设置已应用，正在等待网络重新收敛…')));
			this.selecting = false;
			this.deviceDirty = false;
			return new Promise(L.bind(function(resolve) {
				window.setTimeout(L.bind(function() {
					this.applying = false;
					this.refreshStatus().then(resolve);
				}, this), 2000);
			}, this));
		}, this)).catch(L.bind(function(err) {
			this.applying = false;
			this.repaint();
			ui.addNotification(null, E('p', _('设置应用失败：') + ' ' + (err.message || _('未知错误'))), 'danger');
		}, this));
	},

	// Escape hatch for a split state: ask the backend to re-align the IPv6 default
	// route with the live IPv4 exit without touching the configured policy.
	reconcileExits: function() {
		if (this.applying) return;

		var self = this;
		this.applying = true;
		this.repaint();

		return fs.exec('/usr/sbin/h5000m-netmode', [ 'reconcile' ]).then(function() {
			ui.addNotification(null, E('p', _('已请求重新对齐 IPv4 与 IPv6 出口。')));
		}).catch(function(err) {
			ui.addNotification(null, E('p', _('对齐失败：') + ' ' + (err.message || _('未知错误'))), 'danger');
		}).then(function() {
			return new Promise(function(resolve) { window.setTimeout(resolve, 1500); });
		}).then(L.bind(function() {
			this.applying = false;
			return this.refreshStatus();
		}, this));
	},

	statusPanel: function(data) {
		data.mode = data.mode || 'wan_first';
		data.active4 = data.active4 || 'none';
		data.active6 = data.active6 || 'none';

		var active4 = data.active4;
		var active6 = data.active6;
		var split = data.split === '1';
		var same = active4 === active6 && active4 !== 'none';
		var active = active4 !== 'none' ? active4 : active6;
		var message = this.statusMessage(data);
		var badgeText, badgeClass;

		if (split)
			badgeText = _('IPv4 %s · IPv6 %s').format(this.exitLabel(active4), this.exitLabel(active6));
		else if (same)
			badgeText = _('当前出口：%s').format(this.exitLabel(active4));
		else
			badgeText = _('IPv4：%s · IPv6：%s').format(this.exitLabel(active4), this.exitLabel(active6));

		if (split || active === 'none')
			badgeClass = 'h5net-active fail';
		else if (active === 'other')
			badgeClass = 'h5net-active warn';
		else
			badgeClass = 'h5net-active';

		var curWanDev = (this.deviceMap || {}).wan || '';
		var curModemDev = (this.deviceMap || {}).modem || '';
		var newWanDev = (this.pendingDeviceMap || {}).wan || curWanDev;
		var newModemDev = (this.pendingDeviceMap || {}).modem || curModemDev;
		var changed = this.pendingMode !== data.mode;
		var deviceChanged = curWanDev !== newWanDev || curModemDev !== newModemDev;
		var dirty = changed || deviceChanged;

		var buttons = [];
		if (split) {
			buttons.push(E('button', {
				'class': 'cbi-button cbi-button-action',
				'disabled': this.applying ? 'disabled' : null,
				'click': L.bind(this.reconcileExits, this)
			}, _('对齐出口')));
		}
		buttons.push(E('button', {
			'class': 'cbi-button cbi-button-apply',
			'disabled': (!dirty || this.applying) ? 'disabled' : null,
			'click': L.bind(this.applySelection, this)
		}, this.applying ? _('应用中…') : _('应用设置')));

		return E('div', { 'class': 'h5net', id: 'h5net-status' }, [
			this.styleNode(),
			E('div', { 'class': 'h5net-head' }, [
				E('div', {}, [
					E('h2', {}, _('网络出口')),
					E('p', {}, _('点击连接卡片把该链路设为首选出口，另一条自动作为备用。"仅用此出口"会移除备用链路。'))
				]),
				E('div', { 'class': badgeClass }, badgeText)
			]),
			E('div', { 'class': message.cls }, message.text),
			E('div', { 'class': 'h5net-grid' }, [
				this.routeCard('wan', data),
				this.routeCard('modem', data)
			]),
			E('div', { 'class': 'h5net-foot' }, [
				this.metaBar(data),
				E('div', { 'class': 'h5net-buttons' }, buttons)
			])
		]);
	},

	repaint: function() {
		var old = document.getElementById('h5net-status');
		if (old && this.liveData)
			old.parentNode.replaceChild(this.statusPanel(this.liveData), old);
	},

	refreshStatus: function() {
		return Promise.all([ this.statusCommand(), this.loadDeviceMap() ]).then(L.bind(function(results) {
			this.liveData = this.parseStatus(results[0]);
			if (!this.applying) {
				if (!this.selecting)
					this.pendingMode = this.liveData.mode || 'wan_first';
				// An unapplied device edit must survive the poll, otherwise the
				// dropdown reverts under the user every five seconds.
				if (!this.deviceDirty) {
					var dm = this.deviceMap || {};
					this.pendingDeviceMap = { wan: dm.wan || '', modem: dm.modem || '' };
				}
			}
			this.repaint();
		}, this));
	},

	render: function(res) {
		this.liveData = this.parseStatus(res);
		this.liveData.mode = this.liveData.mode || 'wan_first';
		this.pendingMode = this.liveData.mode;
		this.selecting = false;
		this.applying = false;
		this.deviceDirty = false;

		var dm = this.deviceMap || {};
		this.pendingDeviceMap = { wan: dm.wan || '', modem: dm.modem || '' };

		poll.add(L.bind(this.refreshStatus, this), 5);
		return this.statusPanel(this.liveData);
	}
});
