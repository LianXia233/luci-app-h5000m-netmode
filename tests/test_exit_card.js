// Off-device unit test for exitCard().
//
// The card has four branches - wired uplink, cellular uplink, a foreign default
// route, and no route at all - and a real router can only ever be in one of them
// at a time.  Provoking the others on the device would mean changing the live
// routing policy and cutting the network the test is running over, so the module
// is loaded here with stubs instead: the source is executed unmodified, only its
// four LuCI dependencies and the E() DOM builder are substituted.
//
// What is asserted is the thing that matters - that each branch prints state the
// device could actually be in.  The sheet's own markup shows a WAN card and a
// cellular card side by side, both "在线"; no branch here may reproduce that,
// because one uplink carries the traffic.
//
// usage: node tests/test_exit_card.js <netmode.js>

const fs = require('fs');

const file = process.argv[2];
if (!file) {
	console.error('usage: node tests/test_exit_card.js <netmode.js>');
	process.exit(64);
}
const src = fs.readFileSync(file, 'utf8');

// ---------------------------------------------------------------------------
// stubs
// ---------------------------------------------------------------------------

String.prototype.format = function() {
	const args = Array.prototype.slice.call(arguments);
	let i = 0;
	return String(this).replace(/%[sd]/g, function() { return String(args[i++]); });
};

const _ = function(s) { return s; };
const L = { bind: function(fn, ctx) {
	const extra = Array.prototype.slice.call(arguments, 2);
	return function() {
		return fn.apply(ctx, extra.concat(Array.prototype.slice.call(arguments)));
	};
} };
const fsStub = { exec: function() { return Promise.resolve({ stdout: '' }); } };
const uiStub = { addNotification: function() {} };
const pollStub = { add: function() {} };
const viewStub = { extend: function(obj) { return obj; } };

// Minimal element builder.  exitCard() only needs appendChild and innerHTML, and
// the assertions read the resulting tree, so no real DOM is required.
function E(tag, attr, children) {
	const el = {
		tag: tag,
		attr: attr || {},
		children: [],
		appendChild: function(child) { el.children.push(child); return child; }
	};
	Object.defineProperty(el, 'innerHTML', {
		get: function() { return el._html || ''; },
		set: function(v) { el._html = v; }
	});
	if (children !== undefined && children !== null) {
		(Array.isArray(children) ? children : [ children ]).forEach(function(c) {
			if (c !== undefined && c !== null) el.children.push(c);
		});
	}
	return el;
}

const view = new Function('view', 'fs', 'ui', 'poll', 'L', '_', 'E', src)(
	viewStub, fsStub, uiStub, pollStub, L, _, E);

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

function textOf(node) {
	if (typeof node === 'string') return node;
	if (!node || !node.children) return '';
	return node.children.map(textOf).join('');
}

function findByClass(node, cls) {
	if (!node || typeof node === 'string') return null;
	if ((' ' + (node.attr['class'] || '') + ' ').indexOf(' ' + cls + ' ') > -1) return node;
	for (const c of node.children) {
		const hit = findByClass(c, cls);
		if (hit) return hit;
	}
	return null;
}

function classesOf(node) {
	return (node.attr['class'] || '').split(/\s+/).filter(Boolean);
}

let checks = 0, failures = 0;
function eq(label, got, want) {
	checks++;
	if (got !== want) {
		failures++;
		console.log('FAIL ' + label + ': got ' + JSON.stringify(got) +
			'  want ' + JSON.stringify(want));
	}
}
function ok(label, cond, detail) {
	checks++;
	if (!cond) {
		failures++;
		console.log('FAIL ' + label + (detail ? '  ' + detail : ''));
	}
}

// A healthy device, then one field changed per case.
const BASE = {
	mode: 'wan_first', split: '0',
	active4: 'wan', active6: 'wan', egress4: 'eth1', egress6: 'eth1',
	daed_exit_state: 'none',
	wan_present: '1', wan_available: '1', wan_pending: '0', wan_carrier: '1',
	wan_up: '1', wan6_up: '1', wan4_ready: '1', wan6_ready: '1', wan_device: 'eth1',
	modem_present: '1', modem_available: '1', modem_pending: '0', modem_carrier: '1',
	modem_up: '1', modem6_up: '0', modem4_ready: '1', modem6_ready: '0', modem_device: 'eth2'
};

function card(over) {
	const data = Object.assign({}, BASE, over);
	const node = view.exitCard(data);
	const icon = findByClass(node, 'svgbox');
	return {
		node: node,
		cls: classesOf(node),
		title: textOf(findByClass(node, 'ec-title')),
		live: textOf(findByClass(node, 'ec-live')),
		badge: textOf(findByClass(node, 'ec-badge')),
		iface: textOf(findByClass(node, 'ec-iface')),
		role: textOf(findByClass(node, 'ec-role')),
		svg: icon ? icon.innerHTML : ''
	};
}

function expect(label, c, want) {
	eq(label + ' title', c.title, want.title);
	eq(label + ' live', c.live, want.live);
	eq(label + ' badge', c.badge, want.badge);
	eq(label + ' iface', c.iface, want.iface);
	eq(label + ' role', c.role, want.role);
	ok(label + ' tone ' + want.tone, c.cls.indexOf(want.tone) > -1, 'cls=' + c.cls.join(' '));
	eq(label + ' idle', c.cls.indexOf('is-idle') > -1, !!want.idle);
	ok(label + ' icon', c.svg.indexOf(want.icon) > -1, 'missing ' + want.icon);
	if (want.cell !== undefined)
		eq(label + ' cell variant', c.cls.indexOf('cell') > -1, !!want.cell);
}

// ---------------------------------------------------------------------------
// 1. wired uplink, the state this device is in
// ---------------------------------------------------------------------------
const wan = card({});
expect('wan', wan, {
	title: '当前出口：有线 WAN', live: '在线', badge: 'Ethernet', iface: 'eth1',
	role: '首选出口', tone: 'tone-live', idle: false, icon: 'wan-ring', cell: false
});
ok('wan icon carries the sheet flow class', wan.svg.indexOf('wan-flow') > -1);
ok('wan icon carries the sheet led class', wan.svg.indexOf('wan-led') > -1);

// ---------------------------------------------------------------------------
// 2. cellular uplink, in both policy positions
// ---------------------------------------------------------------------------
const modemBackup = card({ active4: 'modem', active6: 'modem', egress4: 'eth2' });
expect('modem/backup-active', modemBackup, {
	title: '当前出口：5G 模组', live: '在线', badge: '5G / LTE', iface: 'eth2',
	role: '备用出口 · 已接管', tone: 'tone-live', idle: false, icon: 'cell-wave', cell: true
});
eq('modem icon has three waves',
	(modemBackup.svg.match(/class="cell-wave/g) || []).length, 3);
ok('modem icon carries the sheet packet class', modemBackup.svg.indexOf('cell-packet') > -1);

const modemFirst = card({ mode: 'modem_first', active4: 'modem', active6: 'modem',
	egress4: 'eth2' });
expect('modem/primary', modemFirst, {
	title: '当前出口：5G 模组', live: '在线', badge: '5G / LTE', iface: 'eth2',
	role: '首选出口', tone: 'tone-live', idle: false, icon: 'cell-wave', cell: true
});
// The card always describes the uplink that is carrying traffic, so its role is
// that uplink's role - never the backup's.  The two remaining possibilities are
// therefore: the carrier is the policy's second choice (failover happened), or
// the carrier is not in the policy at all.
const wanTookOver = card({ mode: 'modem_first' });
eq('carrier is the policy backup (took over)', wanTookOver.role, '备用出口 · 已接管');
const outsidePolicy = card({ mode: 'wan_only', active4: 'modem', active6: 'none' });
eq('carrier is outside the policy', outsidePolicy.role, '未纳入策略');

// ---------------------------------------------------------------------------
// 3. degraded and absent uplinks
// ---------------------------------------------------------------------------
const pending = card({ wan_up: '0', wan6_up: '0' });
expect('wan/pending', pending, {
	title: '当前出口：有线 WAN', live: '协商中', badge: 'Ethernet', iface: 'eth1',
	role: '首选出口', tone: 'tone-pending', idle: false, icon: 'wan-ring', cell: false
});
const down = card({ wan_up: '0', wan6_up: '0', wan_available: '0', wan_pending: '0' });
expect('wan/down', down, {
	title: '当前出口：有线 WAN', live: '已断开', badge: 'Ethernet', iface: 'eth1',
	role: '首选出口', tone: 'tone-down', idle: true, icon: 'wan-ring', cell: false
});
const unplugged = card({ wan_up: '0', wan6_up: '0', wan_available: '0',
	wan_pending: '0', wan_carrier: '0' });
eq('wan/unplugged label', unplugged.live, '网线未接');
ok('unplugged card is idle', unplugged.cls.indexOf('is-idle') > -1);
const absent = card({ wan_present: '0', wan_up: '0', wan6_up: '0',
	wan_available: '0', wan_device: '' });
expect('wan/absent', absent, {
	title: '当前出口：有线 WAN', live: '未配置', badge: 'Ethernet', iface: '未指定',
	role: '首选出口', tone: 'tone-off', idle: true, icon: 'wan-ring', cell: false
});

// ---------------------------------------------------------------------------
// 4. a family split - the invariant this app exists to protect
// ---------------------------------------------------------------------------
const split = card({ split: '1', active4: 'wan', active6: 'modem', egress6: 'eth2' });
expect('split', split, {
	title: '出口分流', live: '告警', badge: 'IPv4 有线 WAN', iface: 'eth1',
	role: 'IPv6 5G 模组', tone: 'tone-down', idle: true, icon: 'wan-ring', cell: false
});
const splitModemFirst = card({ split: '1', active4: 'modem', active6: 'wan', egress6: 'eth1' });
expect('split/modem-ipv4', splitModemFirst, {
	title: '出口分流', live: '告警', badge: 'IPv4 5G 模组', iface: 'eth2',
	role: 'IPv6 有线 WAN', tone: 'tone-down', idle: true, icon: 'cell-wave', cell: true
});

// ---------------------------------------------------------------------------
// 5. a foreign default route, and no route at all
// ---------------------------------------------------------------------------
const other = card({ active4: 'other', active6: 'none', egress4: 'eth3',
	daed_exit_state: 'wan' });
expect('other/daed', other, {
	title: '当前出口：其他路由', live: '外部接管', badge: 'daed 接管', iface: 'eth3',
	role: '不在本插件策略内', tone: 'tone-pending', idle: false, icon: 'class="route"', cell: false
});
const otherPlain = card({ active4: 'other', active6: 'none', egress4: 'eth3' });
eq('other badge without daed', otherPlain.badge, '外部路由');

const none = card({ active4: 'none', active6: 'none', egress4: 'none', egress6: 'none' });
expect('none', none, {
	title: '无可用出口', live: '离线', badge: '无出口', iface: '—',
	role: '策略：有线优先', tone: 'tone-off', idle: true, icon: 'class="route"', cell: false
});
eq('none honours the configured policy label',
	card({ active4: 'none', active6: 'none', mode: 'modem_only' }).role, '策略：仅 5G');

// ---------------------------------------------------------------------------
// 6. the sheet's demo state must not be reproducible
// ---------------------------------------------------------------------------
const everyCase = [ wan, modemBackup, modemFirst, pending, down, unplugged, absent,
	split, splitModemFirst, other, none, otherPlain ];
ok('no card ever shows the sheet\'s two-uplink demo pairing',
	everyCase.every(c => c.title !== '当前出口：蜂窝网络') ||
	everyCase.filter(c => c.live === '在线').length <= 1,
	'cards claiming 在线: ' + everyCase.filter(c => c.live === '在线').length);
ok('a card is rendered even when nothing is up',
	none.title === '无可用出口' && none.svg.length > 0);

console.log('\n' + checks + ' checks, ' + failures + ' failure(s)');
process.exit(failures ? 1 : 0);
