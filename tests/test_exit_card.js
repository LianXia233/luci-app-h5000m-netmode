// Off-device unit test for exitCard(): the lead card, which carries the plan in
// one half and the live exit in the other.
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

// A node tree walk, not a single find: the lead card holds two halves, and every
// assertion about the readout has to be scoped to its half or it would read the
// plan's title / pill / meta row instead.
function findAllByClass(node, cls, out) {
	out = out || [];
	if (!node || typeof node === 'string') return out;
	if ((' ' + (node.attr['class'] || '') + ' ').indexOf(' ' + cls + ' ') > -1) out.push(node);
	for (const c of node.children) findAllByClass(c, cls, out);
	return out;
}

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
	const half = findByClass(node, 'hero-exit');
	if (!half) throw new Error('the lead card has no hero-exit half');
	const icon = findByClass(half, 'svgbox');
	return {
		node: node,
		cls: classesOf(node),
		title: textOf(findByClass(half, 'ec-title')),
		live: textOf(findByClass(half, 'ec-live')),
		liveCls: classesOf(findByClass(half, 'ec-live')),
		badge: textOf(findByClass(half, 'ec-badge')),
		iface: textOf(findByClass(half, 'ec-iface')),
		role: textOf(findByClass(half, 'ec-role')),
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
	// The pill's colour no longer comes from the card's accent - the two halves of
	// one card state different verdicts, so each pill carries its own state class.
	// The mapping therefore has to be asserted rather than assumed: each tone still
	// implies exactly one pill class, which is what keeps the readout identical to
	// what it was before the halves were merged.
	const pillOfTone = { 'tone-live': 'is-up', 'tone-pending': 'is-pending',
		'tone-down': 'is-down', 'tone-off': 'is-off' };
	ok(label + ' pill ' + pillOfTone[want.tone],
		c.liveCls.indexOf(pillOfTone[want.tone]) > -1, 'cls=' + c.liveCls.join(' '));
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

// ---------------------------------------------------------------------------
// 7. the lead card: the plan and the readout in one card
// ---------------------------------------------------------------------------
const lead = view.exitCard(Object.assign({}, BASE));
const slots = findAllByClass(lead, 'hero-slot');
const viaOf = function(over) {
	return classesOf(view.exitCard(Object.assign({}, BASE, over)));
};
const planHalf = function(over) {
	const node = view.exitCard(Object.assign({}, BASE, over));
	const half = findAllByClass(node, 'hero-egress')[0];
	return {
		node: node,
		live: textOf(findByClass(half, 'ec-live')),
		liveCls: classesOf(findByClass(half, 'ec-live')),
		badge: textOf(findByClass(half, 'ec-badge')),
		role: textOf(findByClass(half, 'ec-role')),
		glyph: findByClass(half, 'svgbox').innerHTML
	};
};

ok('the lead card is one card', classesOf(lead).indexOf('h5net-ecard') > -1);
ok('the lead card carries the hero class', classesOf(lead).indexOf('hero') > -1);
eq('the lead card holds exactly two halves', slots.length, 2);
eq('exactly one card is built, not a card per half',
	findAllByClass(lead, 'h5net-ecard').length, 1);

const planSlot = slots.filter(s => classesOf(s).indexOf('hero-egress') > -1)[0];
const exitSlot = slots.filter(s => classesOf(s).indexOf('hero-exit') > -1)[0];
ok('the plan half exists', !!planSlot);
ok('the readout half exists', !!exitSlot);

// "One card" is a claim about the markup: both halves are drawn with the same
// vocabulary, so neither is styled as an exception to the other.
for (const pair of [ [ 'plan', planSlot ], [ 'readout', exitSlot ] ]) {
	const name = pair[0], half = pair[1];
	eq(name + ' half has one icon box', findAllByClass(half, 'svgbox').length, 1);
	eq(name + ' half has one title', findAllByClass(half, 'ec-title').length, 1);
	eq(name + ' half has one state pill', findAllByClass(half, 'ec-live').length, 1);
	eq(name + ' half has one detail row', findAllByClass(half, 'ec-meta').length, 1);
}

const plan = planHalf({});
eq('plan half title', textOf(findByClass(planSlot, 'ec-title')), '网络出口');
eq('plan half badge states the policy', plan.badge, '有线优先');
eq('plan half states the policy order', plan.role, '1 有线 WAN · 2 5G 模组');
eq('plan half verdict', plan.live, '主备就绪');
ok('plan half pill is green', plan.liveCls.indexOf('is-up') > -1);
ok('plan half keeps the interaction hint',
	textOf(findByClass(planSlot, 'ec-hint')).length > 0);

// A plan verdict that repeats the link verdict would be a second copy of the same
// fact in the same card, which is the noise the merge was meant to remove.
ok('the two halves state different facts',
	plan.live !== textOf(findByClass(exitSlot, 'ec-live')));

// The egress glyph, and the state class that selects its branch.
eq('glyph draws one overlay per branch',
	(plan.glyph.match(/class="eg-flow f-wan"/g) || []).length, 1);
ok('glyph draws the cellular overlay',
	(plan.glyph.match(/class="eg-flow f-modem"/g) || []).length === 1);
ok('glyph draws the wired socket', plan.glyph.indexOf('class="eg-port"') > -1);
eq('glyph draws three cellular bars',
	(plan.glyph.match(/class="eg-bar"/g) || []).length, 3);
ok('the plan half does not reuse the exit icon',
	plan.glyph.indexOf('wan-ring') < 0 && plan.glyph.indexOf('cell-wave') < 0);

ok('a wired carrier marks the wired branch', viaOf({}).indexOf('eg-via-wan') > -1);
ok('a cellular carrier marks the cellular branch',
	viaOf({ active4: 'modem', active6: 'modem' }).indexOf('eg-via-modem') > -1);
ok('a split marks both branches',
	viaOf({ split: '1', active4: 'wan', active6: 'modem' }).indexOf('eg-via-both') > -1);
ok('a foreign route marks neither branch',
	viaOf({ active4: 'other', active6: 'none' }).indexOf('eg-via-none') > -1);
ok('no route marks neither branch',
	viaOf({ active4: 'none', active6: 'none' }).indexOf('eg-via-none') > -1);

// The plan verdict ladder, which is a different ladder from the link verdict: it
// is exhaustive over the states status can report.
eq('plan verdict on a split',
	planHalf({ split: '1', active4: 'wan', active6: 'modem' }).live, '分流告警');
ok('a split plan pill is red',
	planHalf({ split: '1', active4: 'wan', active6: 'modem' }).liveCls.indexOf('is-down') > -1);
eq('plan verdict with no route',
	planHalf({ active4: 'none', active6: 'none' }).live, '无默认路由');
eq('plan verdict behind a foreign route',
	planHalf({ active4: 'other', active6: 'none' }).live, '策略被绕过');
eq('plan verdict in an only-mode policy', planHalf({ mode: 'wan_only' }).live, '单出口运行');

// The case the two verdicts exist for: the link is healthy while the plan is not.
const noBackup = { wan_up: '0', wan6_up: '0', wan_available: '0', wan_pending: '0',
	wan_carrier: '0', active4: 'modem', active6: 'modem', egress4: 'eth2' };
eq('plan verdict when the backup has gone away', planHalf(noBackup).live, '无备用链路');
ok('that plan pill is amber', planHalf(noBackup).liveCls.indexOf('is-pending') > -1);
ok('the card still reports the healthy link',
	viaOf(noBackup).indexOf('tone-live') > -1);
ok('the readout half still says the link is up',
	card(noBackup).live === '在线' && card(noBackup).liveCls.indexOf('is-up') > -1);

// A deliberate only-mode choice is a plan state, not a link fault, so it must not
// repaint the card: colour that no longer means "wrong right now" is worse than no
// colour, and the plan pill already states it.
ok('an only-mode policy keeps the link tone',
	viaOf({ mode: 'wan_only' }).indexOf('tone-live') > -1);
ok('an only-mode policy does not tone the card amber',
	viaOf({ mode: 'wan_only' }).indexOf('tone-pending') < 0);

console.log('\n' + checks + ' checks, ' + failures + ' failure(s)');
process.exit(failures ? 1 : 0);
