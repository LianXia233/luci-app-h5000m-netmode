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
// cellular card side by side, both up; no branch here may reproduce that,
// because one uplink carries the traffic.
//
// The card was rewritten around .h5net-hero / .hero-* (the old .h5net-ecard /
// .ec-* vocabulary and the sheet's eg-via-* three-state glyph binding are gone),
// and the state wording changed with it.  The expectations below are the new
// mapping; every structural and mapping assertion is kept, and the retired
// mechanisms are guarded separately in tests/svg_audit.js.
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

// The card's last element of a meta row is a bare span with no class of its own
// (the role / the policy order), so it has to be read positionally.
function lastSpanOf(meta) {
	if (!meta) return null;
	const spans = meta.children.filter(function(c) { return typeof c !== 'string'; });
	return spans.length ? spans[spans.length - 1] : null;
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

// The tone lives on the icon box, not on the card: .h5net-hero carries only the
// card-level is-idle flag.
function toneOf(node) {
	const half = findByClass(node, 'hero-exit');
	const box = half && findByClass(half, 'hero-svgbox');
	return box ? classesOf(box) : [];
}

function card(over) {
	const data = Object.assign({}, BASE, over);
	const node = view.exitCard(data);
	const half = findByClass(node, 'hero-exit');
	if (!half) throw new Error('the lead card has no hero-exit half');
	const icon = findByClass(half, 'hero-svgbox');
	return {
		node: node,
		cls: classesOf(node),
		tone: toneOf(node),
		title: textOf(findByClass(half, 'hero-title')),
		live: textOf(findByClass(half, 'hero-badge')),
		liveCls: classesOf(findByClass(half, 'hero-badge')),
		badge: textOf(findByClass(half, 'pill')),
		iface: textOf(findByClass(half, 'iface')),
		role: textOf(lastSpanOf(findByClass(half, 'hero-meta'))),
		svg: icon ? icon.innerHTML : ''
	};
}

function expect(label, c, want) {
	eq(label + ' title', c.title, want.title);
	eq(label + ' live', c.live, want.live);
	eq(label + ' badge', c.badge, want.badge);
	eq(label + ' iface', c.iface, want.iface);
	eq(label + ' role', c.role, want.role);
	ok(label + ' tone ' + want.tone, c.tone.indexOf(want.tone) > -1,
		'tone=' + c.tone.join(' '));
	// The pill's colour does not come from the card's accent - the two halves of
	// one card state different verdicts, so each pill carries its own state class.
	// The mapping therefore has to be asserted rather than assumed: each tone
	// still implies exactly one pill class, which is what keeps the readout
	// identical to what it was before the halves were merged.  A healthy uplink
	// is tone-green (wired) or cell (cellular), both of which read as is-up.
	const pillOfTone = { 'tone-green': 'is-up', 'cell': 'is-up', 'tone-pending': 'is-pending',
		'tone-down': 'is-down', 'tone-off': 'is-off' };
	ok(label + ' pill ' + pillOfTone[want.tone],
		c.liveCls.indexOf(pillOfTone[want.tone]) > -1, 'cls=' + c.liveCls.join(' '));
	eq(label + ' idle', c.cls.indexOf('is-idle') > -1, !!want.idle);
	ok(label + ' icon', c.svg.indexOf(want.icon) > -1, 'missing ' + want.icon);
	if (want.cell !== undefined)
		eq(label + ' cell variant', c.tone.indexOf('cell') > -1, !!want.cell);
}

// The exit icon paths that identify each variant inside the SVG string literal.
const ICON_WIRED = 'M26 38v10M38 38v10M20 48h24';
const ICON_CELL = 'M32 24v30M22 54h20';
const ICON_ROUTE = 'M14 46C24 46 20 18 32 18s8 28 18 28';

// ---------------------------------------------------------------------------
// 1. wired uplink, the state this device is in
// ---------------------------------------------------------------------------
const wan = card({});
expect('wan', wan, {
	title: '当前出口：有线 WAN', live: '在线中', badge: '有线宽带', iface: 'eth1',
	role: '首选出口', tone: 'tone-green', idle: false, icon: ICON_WIRED, cell: false
});
ok('wan icon carries its animated flow class', wan.svg.indexOf('svg-dash-fast') > -1);
ok('wan icon carries the status lamp class', wan.svg.indexOf('svg-pulse') > -1);

// ---------------------------------------------------------------------------
// 2. cellular uplink, in both policy positions
// ---------------------------------------------------------------------------
const modemBackup = card({ active4: 'modem', active6: 'modem', egress4: 'eth2' });
expect('modem/backup-active', modemBackup, {
	title: '当前出口：5G 模组', live: '在线中', badge: '5G / LTE', iface: 'eth2',
	role: '备用出口 · 已接管', tone: 'cell', idle: false, icon: ICON_CELL, cell: true
});
eq('modem icon has three waves',
	(modemBackup.svg.match(/class="svg-wave/g) || []).length, 3);
ok('modem icon carries the pulsing core class', modemBackup.svg.indexOf('svg-pulse') > -1);

const modemFirst = card({ mode: 'modem_first', active4: 'modem', active6: 'modem',
	egress4: 'eth2' });
expect('modem/primary', modemFirst, {
	title: '当前出口：5G 模组', live: '在线中', badge: '5G / LTE', iface: 'eth2',
	role: '首选出口', tone: 'cell', idle: false, icon: ICON_CELL, cell: true
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
	title: '当前出口：有线 WAN', live: '网络握手中', badge: '有线宽带', iface: 'eth1',
	role: '首选出口', tone: 'tone-pending', idle: false, icon: ICON_WIRED, cell: false
});
const down = card({ wan_up: '0', wan6_up: '0', wan_available: '0', wan_pending: '0' });
expect('wan/down', down, {
	title: '当前出口：有线 WAN', live: '链路离线', badge: '有线宽带', iface: 'eth1',
	role: '首选出口', tone: 'tone-down', idle: true, icon: ICON_WIRED, cell: false
});
const unplugged = card({ wan_up: '0', wan6_up: '0', wan_available: '0',
	wan_pending: '0', wan_carrier: '0' });
eq('wan/unplugged label', unplugged.live, '物理网线断开');
ok('unplugged card is idle', unplugged.cls.indexOf('is-idle') > -1);
const absent = card({ wan_present: '0', wan_up: '0', wan6_up: '0',
	wan_available: '0', wan_device: '' });
expect('wan/absent', absent, {
	title: '当前出口：有线 WAN', live: '硬件未配置', badge: '有线宽带', iface: '未指定',
	role: '首选出口', tone: 'tone-off', idle: true, icon: ICON_WIRED, cell: false
});

// ---------------------------------------------------------------------------
// 4. a family split - the invariant this app exists to protect
// ---------------------------------------------------------------------------
const split = card({ split: '1', active4: 'wan', active6: 'modem', egress6: 'eth2' });
expect('split', split, {
	title: '出口分流告警', live: '分流故障', badge: 'IPv4 有线 WAN', iface: 'eth1',
	role: 'IPv6 5G 模组', tone: 'tone-down', idle: false, icon: ICON_WIRED, cell: false
});
const splitModemFirst = card({ split: '1', active4: 'modem', active6: 'wan', egress6: 'eth1' });
expect('split/modem-ipv4', splitModemFirst, {
	title: '出口分流告警', live: '分流故障', badge: 'IPv4 5G 模组', iface: 'eth2',
	// A split fixes the tone to the alert colour regardless of which family is
	// where, so the cellular variant shows up in the icon path (asserted above)
	// rather than as a `cell` tone.  The card still must not be painted as a
	// healthy cellular uplink, which is what this pins.
	role: 'IPv6 有线 WAN', tone: 'tone-down', idle: false, icon: ICON_CELL, cell: false
});

// ---------------------------------------------------------------------------
// 5. a foreign default route, and no route at all
// ---------------------------------------------------------------------------
const other = card({ active4: 'other', active6: 'none', egress4: 'eth3',
	daed_exit_state: 'wan' });
expect('other/daed', other, {
	title: '当前出口：第三方路由', live: '外部接管', badge: 'daed 接管', iface: 'eth3',
	role: '未在本策略管辖内', tone: 'tone-pending', idle: false, icon: ICON_ROUTE, cell: false
});
const otherPlain = card({ active4: 'other', active6: 'none', egress4: 'eth3' });
eq('other badge without daed', otherPlain.badge, '外部路由');

const none = card({ active4: 'none', active6: 'none', egress4: 'none', egress6: 'none' });
expect('none', none, {
	title: '无可用网络出口', live: '离线', badge: '无出口', iface: '—',
	role: '策略：有线 WAN 优先', tone: 'tone-off', idle: true, icon: ICON_ROUTE, cell: false
});
eq('none honours the configured policy label',
	card({ active4: 'none', active6: 'none', mode: 'modem_only' }).role, '策略：仅用 5G 模组');

// ---------------------------------------------------------------------------
// 6. the sheet's demo state must not be reproducible
// ---------------------------------------------------------------------------
const everyCase = [ wan, modemBackup, modemFirst, pending, down, unplugged, absent,
	split, splitModemFirst, other, none, otherPlain ];
ok('no card ever shows the sheet\'s two-uplink demo pairing',
	everyCase.filter(c => c.live === '在线中').length <= 1 ||
	everyCase.every(c => c.title !== '当前出口：蜂窝网络'),
	'cards claiming 在线中: ' + everyCase.filter(c => c.live === '在线中').length);
ok('a card is rendered even when nothing is up',
	none.title === '无可用网络出口' && none.svg.length > 0);

// ---------------------------------------------------------------------------
// 7. the lead card: the plan and the readout in one card
// ---------------------------------------------------------------------------
const lead = view.exitCard(Object.assign({}, BASE));
const slots = findAllByClass(lead, 'hero-slot');
const viaOf = function(over) {
	return toneOf(view.exitCard(Object.assign({}, BASE, over)));
};
const planHalf = function(over) {
	const node = view.exitCard(Object.assign({}, BASE, over));
	const half = findAllByClass(node, 'hero-egress')[0];
	return {
		node: node,
		live: textOf(findByClass(half, 'hero-badge')),
		liveCls: classesOf(findByClass(half, 'hero-badge')),
		badge: textOf(findByClass(half, 'pill')),
		order: textOf(lastSpanOf(findByClass(half, 'hero-meta'))),
		glyph: findByClass(half, 'hero-svgbox').innerHTML
	};
};

ok('the lead card is one card', classesOf(lead).indexOf('h5net-hero') > -1);
ok('the retired .h5net-ecard wrapper is not reused', classesOf(lead).indexOf('h5net-ecard') < 0);
eq('the lead card holds exactly two halves', slots.length, 2);
eq('exactly one card is built, not a card per half',
	findAllByClass(lead, 'h5net-hero').length, 1);

const planSlot = slots.filter(s => classesOf(s).indexOf('hero-egress') > -1)[0];
const exitSlot = slots.filter(s => classesOf(s).indexOf('hero-exit') > -1)[0];
ok('the plan half exists', !!planSlot);
ok('the readout half exists', !!exitSlot);

// "One card" is a claim about the markup: both halves are drawn with the same
// vocabulary, so neither is styled as an exception to the other.
for (const pair of [ [ 'plan', planSlot ], [ 'readout', exitSlot ] ]) {
	const name = pair[0], half = pair[1];
	eq(name + ' half has one icon box', findAllByClass(half, 'hero-svgbox').length, 1);
	eq(name + ' half has one title', findAllByClass(half, 'hero-title').length, 1);
	eq(name + ' half has one state pill', findAllByClass(half, 'hero-badge').length, 1);
	eq(name + ' half has one detail row', findAllByClass(half, 'hero-meta').length, 1);
}

const plan = planHalf({});
eq('plan half title', textOf(findByClass(planSlot, 'hero-title')), '多出口调度策略');
eq('plan half badge states the policy', plan.badge, '有线 WAN 优先');
eq('plan half states the policy order', plan.order, '1. 有线 WAN ➔ 2. 5G 模组');
eq('plan half verdict', plan.live, '双路由就绪');
ok('plan half pill is green', plan.liveCls.indexOf('is-up') > -1);
ok('plan half keeps the interaction hint',
	textOf(findByClass(planSlot, 'hero-hint')).length > 0);

// A plan verdict that repeats the link verdict would be a second copy of the same
// fact in the same card, which is the noise the merge was meant to remove.
ok('the two halves state different facts',
	plan.live !== textOf(findByClass(exitSlot, 'hero-badge')));

// The egress glyph.  It is a fixed two-direction diagram now: both direction
// paths always carry a flow animation and the glyph no longer switches a branch
// on according to which uplink is live (that was the sheet's eg-via-* binding).
// What is pinned here is that the glyph stays self-contained - the animated
// classes live inside the SVG string literal - and that it does not quietly
// reuse the exit card's own icon vocabulary.
eq('glyph draws one flow path per direction',
	(plan.glyph.match(/class="svg-dash"/g) || []).length, 2);
ok('glyph marks the local socket', plan.glyph.indexOf('class="svg-pulse"') > -1);
ok('glyph uses the shared 64x64 box', plan.glyph.indexOf('viewBox="0 0 64 64"') > -1);
ok('the plan half does not reuse the exit icon',
	plan.glyph.indexOf('svg-dash-fast') < 0 && plan.glyph.indexOf('svg-wave') < 0);
ok('the plan half carries no per-branch state class',
	classesOf(planSlot).every(c => c.indexOf('eg-via-') < 0) &&
	!plan.glyph.match(/class="[^"]*eg-via-/));

// The plan verdict ladder, which is a different ladder from the link verdict: it
// is exhaustive over the states status can report.
eq('plan verdict on a split',
	planHalf({ split: '1', active4: 'wan', active6: 'modem' }).live, '分流异常');
ok('a split plan pill is red',
	planHalf({ split: '1', active4: 'wan', active6: 'modem' }).liveCls.indexOf('is-down') > -1);
eq('plan verdict with no route',
	planHalf({ active4: 'none', active6: 'none' }).live, '无默认网关');
eq('plan verdict behind a foreign route',
	planHalf({ active4: 'other', active6: 'none' }).live, '策略旁路');
eq('plan verdict in an only-mode policy', planHalf({ mode: 'wan_only' }).live, '单出口运行');

// The case the two verdicts exist for: the link is healthy while the plan is not.
const noBackup = { wan_up: '0', wan6_up: '0', wan_available: '0', wan_pending: '0',
	wan_carrier: '0', active4: 'modem', active6: 'modem', egress4: 'eth2' };
eq('plan verdict when the backup has gone away', planHalf(noBackup).live, '备用链路未就绪');
ok('that plan pill is amber', planHalf(noBackup).liveCls.indexOf('is-pending') > -1);
ok('the card still reports the healthy link', viaOf(noBackup).indexOf('cell') > -1);
ok('the readout half still says the link is up',
	card(noBackup).live === '在线中' && card(noBackup).liveCls.indexOf('is-up') > -1);

// A deliberate only-mode choice is a plan state, not a link fault, so it must not
// repaint the card: colour that no longer means "wrong right now" is worse than no
// colour, and the plan pill already states it.
ok('an only-mode policy keeps the link tone',
	viaOf({ mode: 'wan_only' }).indexOf('tone-green') > -1);
ok('an only-mode policy does not tone the card amber',
	viaOf({ mode: 'wan_only' }).indexOf('tone-pending') < 0);

console.log('\n' + checks + ' checks, ' + failures + ' failure(s)');
process.exit(failures ? 1 : 0);
