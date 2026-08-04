'use strict';
'require view';
'require fs';
'require ui';
'require poll';

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
		return Promise.all([this.statusCommand(), this.loadDeviceMap()]).then(L.bind(function(results) {
			return results[0];
		}, this));
	},

	loadDeviceMap: function() {
		return fs.exec('/usr/sbin/h5000m-netmode', ['list-devices']).then(L.bind(function(res) {
			var devices = [];
			(res.stdout || '').trim().split(/\n/).forEach(function(name) {
				name = name.trim();
				if (name && devices.indexOf(name) < 0)
					devices.push(name);
			});
			this.availableDevices = devices;
		}, this)).catch(L.bind(function() {
			this.availableDevices = ['eth0', 'eth1', 'eth2'];
		}, this)).then(L.bind(function() {
			return fs.exec('/usr/sbin/h5000m-netmode', ['get-device-map']);
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
			'.h5net-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:12px}',
			'.h5net-card{position:relative;padding:15px;border:1px solid var(--border-color-medium,#ddd);border-radius:11px;background:var(--background-color-high,#fff);cursor:pointer;user-select:none;transition:border-color .18s,box-shadow .18s,transform .18s}',
			'.h5net-card:hover{border-color:rgba(79,143,247,.6);transform:translateY(-1px)}.h5net-card:focus{outline:2px solid rgba(79,143,247,.35);outline-offset:2px}',
			'.h5net-card.selected{border-color:rgba(79,143,247,.72);box-shadow:0 0 0 2px rgba(79,143,247,.08)}.h5net-card.active{border-color:rgba(49,185,133,.65);box-shadow:0 0 0 2px rgba(49,185,133,.08)}.h5net-card.unselected{opacity:.62}',
			'.h5net-cardtop{display:flex;align-items:flex-start;justify-content:space-between;gap:10px}.h5net-name{display:flex;align-items:center;gap:10px}',
			'.h5net-icon{display:flex;align-items:center;justify-content:center;width:38px;height:38px;border-radius:10px;background:rgba(79,143,247,.10);color:var(--net-blue);font-size:12px;font-weight:700}.h5net-card.modem .h5net-icon{background:rgba(49,185,133,.10);color:var(--net-green)}',
			'.h5net-name h3{margin:0 0 2px;font-size:16px}.h5net-role{color:var(--text-color-medium,#777);font-size:12px}.h5net-card.selected .h5net-role{color:var(--net-blue);font-weight:600}',
			'.h5net-state{display:inline-flex;align-items:center;gap:6px;font-size:12px;font-weight:600;color:var(--net-red);white-space:nowrap}.h5net-state:before{content:"";width:7px;height:7px;border-radius:50%;background:currentColor}.h5net-state.up{color:var(--net-green)}.h5net-state.idle{color:var(--text-color-medium,#888)}',
			'.h5net-protos{display:flex;flex-wrap:wrap;gap:7px;margin-top:15px}.h5net-proto{padding:5px 8px;border-radius:7px;background:var(--background-color-low,#f5f5f5);font-size:12px;color:var(--text-color-medium,#666)}.h5net-proto.current{background:rgba(49,185,133,.11);color:var(--net-green);font-weight:600}',
			'.h5net-device-row{display:flex;align-items:center;gap:8px;margin-top:10px;padding-top:10px;border-top:1px dashed var(--border-color-low,#e8e8e8)}',
			'.h5net-device-row label{font-size:12px;color:var(--text-color-medium,#777);white-space:nowrap}',
			'.h5net-device-row select{flex:1;padding:6px 8px;border:1px solid var(--border-color-medium,#ccc);border-radius:6px;background:var(--background-color-high,#fff);font-size:13px;color:var(--text-color,#333);cursor:pointer;outline:none;transition:border-color .15s}',
			'.h5net-device-row select:focus{border-color:var(--net-blue);box-shadow:0 0 0 2px rgba(79,143,247,.08)}',
			'.h5net-actions{display:flex;justify-content:flex-end;align-items:center;margin-top:14px;padding-top:14px;border-top:1px solid var(--border-color-low,#e8e8e8)}.h5net-actions .cbi-button{min-width:112px}',
			'@media(max-width:620px){.h5net-head{display:block}.h5net-active{margin-top:11px}.h5net-grid{grid-template-columns:1fr}.h5net-actions .cbi-button{width:100%}}'
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
		if (order.length === 1) return _('唯一出口');
		return position === 0 ? '1 · ' + _('首选出口') : '2 · ' + _('备用出口');
	},

	connectionState: function(present, up) {
		if (present !== '1') return { label: _('未配置'), cls: 'idle' };
		if (up === '1') return { label: _('已连接'), cls: 'up' };
		return { label: _('已断开'), cls: '' };
	},

	selectRoute: function(kind, ev) {
		var order;
		if (ev) ev.preventDefault();
		if (this.applying) return;

		order = this.modeOrder(this.pendingMode);
		if (!this.selecting)
			order = [ kind ];
		else if (order.length === 1 && order[0] !== kind)
			order.push(kind);
		else if (order.length > 1)
			order = [ kind ];

		this.selecting = true;
		this.pendingMode = this.orderMode(order);
		this.repaint();
	},

	cardKeydown: function(kind, ev) {
		if (ev.key === 'Enter' || ev.key === ' ') this.selectRoute(kind, ev);
	},

	onDeviceChange: function(role, ev) {
		if (ev) ev.stopPropagation();
		var dev = ev.target.value;
		if (!dev || this.applying) return;

		// Swap device assignments if needed
		var otherRole = role === 'wan' ? 'modem' : 'wan';
		if (this.pendingDeviceMap[otherRole] === dev)
			this.pendingDeviceMap[otherRole] = this.pendingDeviceMap[role];

		this.pendingDeviceMap[role] = dev;
		this.repaint();
	},

	deviceDropdown: function(role) {
		var self = this;
		var devices = this.availableDevices || [];
		var currentDev = (this.pendingDeviceMap || {})[role] || '';

		return E('div', {
			'class': 'h5net-device-row',
			'click': function(ev) { ev.stopPropagation(); }
		}, [
			E('label', {}, role === 'wan' ? '接口：' : '接口：'),
			E('select', {
				'change': L.bind(this.onDeviceChange, this, role),
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
		var present = modem ? data.modem_present : data.wan_present;
		var up4 = modem ? data.modem_up : data.wan_up;
		var up6 = modem ? data.modem6_up : data.wan6_up;
		var ready4 = modem ? (data.modem4_ready || up4) : (data.wan4_ready || up4);
		var ready6 = modem ? (data.modem6_ready || up6) : (data.wan6_ready || up6);
		var order = this.modeOrder(this.pendingMode);
		var selected = order.indexOf(kind) > -1;
		var active4 = data.active4 === kind;
		var active6 = data.active6 === kind;
		var state = this.connectionState(present, (up4 === '1' || up6 === '1') ? '1' : '0');
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
				E('span', { 'class': 'h5net-proto' + (active4 ? ' current' : '') }, active4 ? _('IPv4 使用中') : (ready4 === '1' ? _('IPv4 就绪') : _('IPv4 不可用'))),
				E('span', { 'class': 'h5net-proto' + (active6 ? ' current' : '') }, active6 ? _('IPv6 使用中') : (ready6 === '1' ? _('IPv6 就绪') : _('IPv6 不可用')))
			]),
			this.deviceDropdown(kind)
		]);
	},

	statusMessage: function(data) {
		var mode = data.mode;
		var preferred = mode === 'modem_first' || mode === 'modem_only' ? 'modem' : 'wan';
		var fallback = preferred === 'wan' ? 'modem' : 'wan';
		var active = data.active4 !== 'none' ? data.active4 : data.active6;

		if (active === 'none') return _('当前无默认路由可用。请检查网线或 5G 连接。');
		if (mode === 'wan_only' || mode === 'modem_only')
			return _('当前策略仅启用了 %s。').format(this.exitLabel(preferred));
		if (active === fallback && data.active6 === 'none')
			return _('%s 不可用，IPv4 已切换至 %s。IPv6 保持禁用状态以避免流量分散到两个出口。').format(this.exitLabel(preferred), this.exitLabel(fallback));
		if (active === fallback)
			return _('%s 不可用，流量已切换至 %s。').format(this.exitLabel(preferred), this.exitLabel(fallback));
		if (active === preferred && data.active6 === 'none')
			return _('IPv4 正在使用 %s。首选出口上 IPv6 不可用，备用 IPv6 已禁用以避免流量分裂。').format(this.exitLabel(preferred));
		if (active === preferred)
			return _('IPv4 和 IPv6 正在使用首选出口。备用出口将在需要时接替。');
		return _('当前流量正在使用其他默认路由。');
	},

	applySelection: function() {
		if (this.applying) return;

		var modeChanged = this.pendingMode !== this.liveData.mode;
		var deviceChanged = false;

		var curWanDev = this.deviceMap ? (this.deviceMap.wan || '') : '';
		var curModemDev = this.deviceMap ? (this.deviceMap.modem || '') : '';
		var newWanDev = (this.pendingDeviceMap || {}).wan || curWanDev;
		var newModemDev = (this.pendingDeviceMap || {}).modem || curModemDev;
		if (curWanDev !== newWanDev || curModemDev !== newModemDev)
			deviceChanged = true;

		if (!modeChanged && !deviceChanged) return;

		this.applying = true;
		this.repaint();

		var promises = [];

		if (modeChanged) {
			promises.push(
				fs.exec('/usr/sbin/h5000m-netmode', ['set', this.pendingMode]).catch(function(err) {
					throw new Error('Mode: ' + (err.message || _('未知错误')));
				})
			);
		}

		if (deviceChanged) {
			if (curWanDev !== newWanDev && newWanDev) {
				promises.push(
					fs.exec('/usr/sbin/h5000m-netmode', ['set-device-map', 'wan', newWanDev]).catch(function(err) {
						throw new Error('WAN: ' + (err.message || _('未知错误')));
					})
				);
			}
			if (curModemDev !== newModemDev && newModemDev) {
				promises.push(
					fs.exec('/usr/sbin/h5000m-netmode', ['set-device-map', 'modem', newModemDev]).catch(function(err) {
						throw new Error('5G: ' + (err.message || _('未知错误')));
					})
				);
			}
		}

		return Promise.all(promises).then(L.bind(function() {
			ui.addNotification(null, E('p', _('设置已应用成功')));
			this.selecting = false;
			return new Promise(L.bind(function(resolve) {
				window.setTimeout(L.bind(function() {
					this.applying = false;
					this.refreshStatus().then(resolve);
				}, this), 1200);
			}, this));
		}, this), L.bind(function(err) {
			this.applying = false;
			this.repaint();
			ui.addNotification(null, E('p', _('设置应用失败：') + ' ' + (err.message || _('未知错误'))), 'danger');
		}, this));
	},

	statusPanel: function(data) {
		var same, active, badgeText, badgeClass, changed, deviceChanged;
		data.mode = data.mode || 'wan_first';
		data.active4 = data.active4 || 'none';
		data.active6 = data.active6 || 'none';
		same = data.active4 === data.active6 && data.active4 !== 'none';
		active = data.active4 !== 'none' ? data.active4 : data.active6;
		badgeText = same ? _('当前出口：%s').format(this.exitLabel(active)) : _('IPv4：%s · IPv6：%s').format(this.exitLabel(data.active4), this.exitLabel(data.active6));
		badgeClass = 'h5net-active' + (active === 'none' ? ' fail' : (active === 'other' ? ' warn' : ''));
		changed = this.pendingMode !== data.mode;

		var curWanDev = this.deviceMap ? (this.deviceMap.wan || '') : '';
		var curModemDev = this.deviceMap ? (this.deviceMap.modem || '') : '';
		var newWanDev = (this.pendingDeviceMap || {}).wan || curWanDev;
		var newModemDev = (this.pendingDeviceMap || {}).modem || curModemDev;
		deviceChanged = curWanDev !== newWanDev || curModemDev !== newModemDev;

		return E('div', { 'class': 'h5net', id: 'h5net-status' }, [
			this.styleNode(),
			E('div', { 'class': 'h5net-head' }, [
				E('div', {}, [ E('h2', {}, _('网络出口')), E('p', {}, _('点击连接卡片设置优先级顺序。第一个为首选出口，第二个为备用出口。')) ]),
				E('div', { 'class': badgeClass }, badgeText)
			]),
			E('div', { 'class': 'h5net-note' }, this.statusMessage(data)),
			E('div', { 'class': 'h5net-grid' }, [ this.routeCard('wan', data), this.routeCard('modem', data) ]),
			E('div', { 'class': 'h5net-actions' }, [
				E('button', {
					'class': 'cbi-button cbi-button-apply',
					'disabled': (!changed && !deviceChanged || this.applying) ? 'disabled' : null,
					'click': L.bind(this.applySelection, this)
				}, this.applying ? _('应用中…') : _('应用设置'))
			])
		]);
	},

	repaint: function() {
		var old = document.getElementById('h5net-status');
		if (old && this.liveData)
			old.parentNode.replaceChild(this.statusPanel(this.liveData), old);
	},

	refreshStatus: function() {
		return Promise.all([this.statusCommand(), this.loadDeviceMap()]).then(L.bind(function(results) {
			var res = results[0];
			this.liveData = this.parseStatus(res);
			if (!this.selecting && !this.applying)
				this.pendingMode = this.liveData.mode || 'wan_first';
			if (!this.applying) {
				var dm = this.deviceMap || {};
				this.pendingDeviceMap = {};
				this.pendingDeviceMap.wan = dm.wan || '';
				this.pendingDeviceMap.modem = dm.modem || '';
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

		var dm = this.deviceMap || {};
		this.pendingDeviceMap = {};
		this.pendingDeviceMap.wan = dm.wan || '';
		this.pendingDeviceMap.modem = dm.modem || '';

		poll.add(L.bind(this.refreshStatus, this), 5);
		return this.statusPanel(this.liveData);
	}
});
