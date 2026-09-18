// Renders the real device's live status through the real panel code and dumps
// what each surface would print.  The point is to read the page the way a user
// reads it: the same fields, the same branches, no stubs for the parts under
// test.  A fixture module would only re-state my assumptions, so this reads the
// captured status file verbatim.
//
// usage: node tests/render_live.js <netmode.js> <status.txt>

const fs = require('fs');

const file = process.argv[2];
const statusFile = process.argv[3];
if (!file || !statusFile) {
	console.error('usage: node tests/render_live.js <netmode.js> <status.txt>');
	process.exit(64);
}
const src = fs.readFileSync(file, 'utf8');

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

function E(tag, attr, children) {
	const el = {
		tag: tag, attr: attr || {}, children: [],
		appendChild: function(c) { el.children.push(c); return c; }
	};
	Object.defineProperty(el, 'innerHTML', {
		get: function() { return el._html || ''; },
		set: function(v) { el._html = v; }
	});
	if (children !== undefined && children !== null)
		(Array.isArray(children) ? children : [ children ]).forEach(function(c) {
			if (c !== undefined && c !== null) el.children.push(c);
		});
	return el;
}

const view = new Function('view', 'fs', 'ui', 'poll', 'L', '_', 'E', src)(
	viewStub, fsStub, uiStub, pollStub, L, _, E);

function textOf(node) {
	if (typeof node === 'string') return node;
	if (!node || !node.children) return '';
	return node.children.map(textOf).join('');
}
function findByClass(node, cls) {
	if (!node || typeof node === 'string') return null;
	if ((' ' + (node.attr['class'] || '') + ' ').indexOf(' ' + cls + ' ') > -1) return node;
	for (const c of node.children) { const h = findByClass(c, cls); if (h) return h; }
	return null;
}
function findAllByClass(node, cls, out) {
	out = out || [];
	if (!node || typeof node === 'string') return out;
	if ((' ' + (node.attr['class'] || '') + ' ').indexOf(' ' + cls + ' ') > -1) out.push(node);
	for (const c of node.children) findAllByClass(c, cls, out);
	return out;
}
function classesOf(node) { return (node.attr['class'] || '').split(/\s+/).filter(Boolean); }

// Parse the captured status output exactly the way the page does.
const raw = fs.readFileSync(statusFile, 'utf8');
const data = {};
raw.trim().split(/\n/).forEach(function(line) {
	const pos = line.indexOf('=');
	if (pos > -1) data[line.substring(0, pos)] = line.substring(pos + 1);
});

console.log('=== live status fields (from device) ===');
[ 'mode', 'active4', 'active6', 'egress4', 'egress6', 'split',
  'wan_up', 'wan4_ready', 'wan6_ready', 'wan_carrier', 'wan_available', 'wan_pending',
  'modem_up', 'modem4_ready', 'modem6_ready',
  'watcher', 'health_check', 'wan_health', 'modem_health',
  'daed_exit_state', 'ipv6_owner', 'ipv6_desired' ].forEach(function(k) {
	console.log('  ' + k + ' = ' + (data[k] === undefined ? '(absent)' : data[k]));
});

const panel = view.statusPanel(data);

console.log('\n=== banner / note ===');
const note = findByClass(panel, 'h5net-note');
console.log('  class: ' + (note ? note.attr['class'] : '(none)'));
console.log('  text : ' + textOf(note));

console.log('\n=== lead card ===');
const card = findByClass(panel, 'h5net-ecard');
console.log('  card classes: ' + classesOf(card).join(' '));
const halves = findAllByClass(card, 'hero-slot');
[ 'plan (hero-egress)', 'exit (hero-exit)' ].forEach(function(label, i) {
	const half = halves[i];
	if (!half) { console.log('  ' + label + ': MISSING'); return; }
	console.log('  ' + label + ':');
	console.log('    title: ' + textOf(findByClass(half, 'ec-title')));
	console.log('    live : ' + textOf(findByClass(half, 'ec-live')) +
		'  [' + classesOf(findByClass(half, 'ec-live')).join(' ') + ']');
	console.log('    meta : ' + textOf(findByClass(half, 'ec-meta')));
	const hint = findByClass(half, 'ec-hint');
	if (hint) console.log('    hint : ' + textOf(hint));
});

console.log('\n=== status tiles ===');
findAllByClass(panel, 'h5net-stat-item').forEach(function(item) {
	const icon = findByClass(item, 'h5net-stat-icon');
	const b = item.children[1];
	const val = findByClass(item, 'h5net-stat-value');
	const hint = findByClass(item, 'h5net-stat-hint');
	console.log('  ' + (b ? textOf(b) : '?'));
	console.log('    value: ' + (val ? textOf(val) : ''));
	console.log('    hint : ' + (hint ? textOf(hint) : ''));
	console.log('    icon : ' + classesOf(icon).join(' '));
});

console.log('\n=== route cards ===');
findAllByClass(panel, 'h5net-card').forEach(function(c) {
	if (classesOf(c).indexOf('h5net-icon') > -1) return;
	const name = findByClass(c, 'h5net-name');
	const state = findByClass(c, 'h5net-state');
	const protos = findAllByClass(c, 'h5net-proto');
	const foot = findByClass(c, 'h5net-cardfoot');
	console.log('  ' + (name ? textOf(name) : '?') +
		'  |  state: ' + (state ? textOf(state) + ' [' + classesOf(state).join(' ') + ']' : ''));
	console.log('    classes: ' + classesOf(c).join(' '));
	console.log('    protos : ' + protos.map(function(p) {
		return textOf(p) + '[' + classesOf(p).join(' ') + ']';
	}).join('  '));
	if (foot) console.log('    button : ' + textOf(foot) +
		'  disabled=' + (findByClass(foot, 'h5net-link') ?
			String(findByClass(foot, 'h5net-link').attr['disabled'] !== null &&
			findByClass(foot, 'h5net-link').attr['disabled'] !== undefined) : '?'));
});

console.log('\n=== meta bar + buttons ===');
console.log('  ' + textOf(findByClass(panel, 'h5net-meta')));
findAllByClass(panel, 'cbi-button').forEach(function(b) {
	console.log('  button: ' + textOf(b) + '  disabled=' +
		(b.attr['disabled'] !== null && b.attr['disabled'] !== undefined));
});
