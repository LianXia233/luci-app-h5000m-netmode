// Audit the animated SVGs and the front-end/back-end field contract.
//
// The animations live in two places - the SVG class attributes inside JS string
// literals and the CSS rules injected by styleNode() - and a mistake in either
// one is invisible to a syntax check: a class with no rule renders a frozen icon,
// a rule with no keyframes animates nothing, and a field the view reads but the
// backend does not print silently becomes `undefined` in the UI.
//
// The SVGs themselves live inside JS string literals, so they cannot be parsed as
// XML directly.  This script harvests every string literal, concatenates them and
// matches the result, which reconstructs each SVG exactly as the browser receives
// it from innerHTML.
//
// usage: node tests/svg_audit.js <netmode.js> <h5000m-netmode>

const fs = require('fs');

const file = process.argv[2];
const backend = process.argv[3];
if (!file || !backend) {
	console.error('usage: node tests/svg_audit.js <netmode.js> <h5000m-netmode>');
	process.exit(64);
}

const src = fs.readFileSync(file, 'utf8');
const backendSrc = fs.readFileSync(backend, 'utf8');

let failed = 0;
function report(ok, label, detail) {
	console.log((ok ? 'ok   ' : 'FAIL ') + label + (detail ? '  ' + detail : ''));
	if (!ok) failed++;
}

try {
	new Function(src);
	console.log('JS syntax: OK');
} catch (err) {
	console.log('JS syntax: ERROR ' + err.message);
	process.exit(1);
}

// ---------------------------------------------------------------------------
// SVG fragments
// ---------------------------------------------------------------------------

const literals = [ ...src.matchAll(/'((?:[^'\\\n]|\\.)*)'/g) ].map(m => m[1]);
const flat = literals.join('\n');
const svgs = [ ...flat.matchAll(/<svg[^>]*>[\s\S]*?<\/svg>/g) ].map(m => m[0]);

console.log('\nSVG fragments found: ' + svgs.length +
	' (2 uplink card icons + 8 tiles + 3 exit-card variants + 1 egress glyph expected)');
svgs.forEach((svg, i) => {
	const viewBox = (svg.match(/viewBox="([^"]+)"/) || [])[1];
	const classes = [ ...svg.matchAll(/class="([^"]+)"/g) ].map(m => m[1]).sort();
	const balanced = (svg.match(/<svg/g) || []).length === (svg.match(/<\/svg>/g) || []).length;
	console.log('  #' + (i + 1) + '  viewBox=' + viewBox + '  len=' + svg.length +
		'  balanced=' + balanced + '  classes=[' + classes.join(', ') + ']');
});
report(svgs.length === 14, 'SVG count is 14', 'found ' + svgs.length);
report(svgs.every(s => (s.match(/<svg/g) || []).length === (s.match(/<\/svg>/g) || []).length),
	'every SVG is balanced');

// ---------------------------------------------------------------------------
// CSS: keyframes, animation classes, tone and state classes
// ---------------------------------------------------------------------------

const keyframes = [ ...src.matchAll(/@keyframes\s+([A-Za-z0-9_-]+)/g) ].map(m => m[1]);
console.log('\n@keyframes declared (' + keyframes.length + '): ' + keyframes.join(', '));

// A rule's selector is not always ".h5net .name".  The exit card nests one level
// deeper (.h5net .h5net-ecard .wan-flow), and some rules carry a compound or a
// descendant part (.cell-wave.w2, ".tone-down svg *").  Take the last class name
// in the selector, ignoring the .h5net scope token itself.
const lastClassOf = function(selector) {
	const names = [ ...selector.matchAll(/\.([A-Za-z0-9_-]+)/g) ].map(m => m[1])
		.filter(n => n !== 'h5net');
	return names.length ? names[names.length - 1] : null;
};

// An animation declaration is not always the first property in a rule: several of
// them (.wave / .ring / .signal / .wan-ring / .cell-wave) open with transform-origin,
// and the delay-only modifiers carry nothing but animation-delay.  Scan the whole
// declaration block for "animation" instead of anchoring on it.
const scopedRules = [ ...src.matchAll(/(\.h5net[^{}]*)\{([^{}]*)\}/g) ];
const animClasses = [ ...new Set(scopedRules
	.filter(m => m[2].indexOf('animation') > -1)
	.map(m => lastClassOf(m[1]))
	.filter(Boolean)) ].sort();
console.log('scoped animation classes (' + animClasses.length + '): ' + animClasses.join(', '));

const expectedKeyframes = [ 'pulse', 'flow', 'wave', 'ring', 'signal', 'travel', 'progress',
	'upload', 'scan', 'node',
	// the exit card's own set, from the design sheet
	'breath', 'led', 'dash', 'ec-ring', 'ec-wave', 'packet' ];
const missingKeyframes = expectedKeyframes.filter(k => keyframes.indexOf(k) < 0);
report(!missingKeyframes.length, 'all sheet keyframes present',
	missingKeyframes.length ? 'missing ' + missingKeyframes.join(', ') : '');

// `animation:` may name a keyframe other than the class (dash -> flow, blink ->
// pulse, orbit -> ring, route -> flow), so check every referenced name resolves.
const referenced = [ ...new Set([ ...src.matchAll(/animation:\s*([A-Za-z0-9_-]+)\s/g) ].map(m => m[1])) ];
const dangling = referenced.filter(k => keyframes.indexOf(k) < 0);
report(!dangling.length, 'every animation: target is declared',
	dangling.length ? 'dangling ' + dangling.join(', ') : '');

const expectedClasses = [ 'pulse', 'flow', 'wave', 'ring', 'signal', 'dash', 'blink', 'wave2',
	'orbit', 'travel', 'progress', 'upload', 'scan', 'scan2', 'scan3', 'route', 'node',
	// exit card
	'wan-flow', 'wan-led', 'wan-ring', 'cell-wave', 'cell-core', 'cell-signal', 'cell-packet' ];
const missingClasses = expectedClasses.filter(c => animClasses.indexOf(c) < 0);
report(!missingClasses.length, 'every declared animation rule exists',
	missingClasses.length ? 'missing ' + missingClasses.join(', ') : '');

// Every animation class used in the markup must have a rule, otherwise the icon
// silently renders frozen.  Purely presentational classes are excluded by design,
// not by accident: `wan-port` only sets fill / stroke / linecap on the card's
// paths and is expected to be static.
const NON_ANIMATED = new Set([
	// the exit card's own static geometry: it sets fill / stroke / linecap only
	'wan-port',
	// the egress glyph's static geometry.  None of it carries state: which branch
	// is live is decided by the card's eg-via-* rule, which switches that branch's
	// .eg-flow overlay on, so the sockets, the bars and the base paths stay still.
	'eg-dev', 'eg-link', 'eg-port', 'eg-bar'
]);
const usedClasses = [ ...new Set(svgs.flatMap(s =>
	[ ...s.matchAll(/class="([^"]+)"/g) ].flatMap(m => m[1].split(/\s+/)))) ].sort();
const unbound = usedClasses.filter(c => animClasses.indexOf(c) < 0 && !NON_ANIMATED.has(c));
report(!unbound.length, 'every animated class used in markup is bound',
	unbound.length ? 'unbound ' + unbound.join(', ') : '');

// Tone classes: 8 base colours from the sheet, and tone-live is deliberately
// absent (a healthy uplink keeps the card's own colour).
const tones = [ ...new Set([ ...src.matchAll(/tone:\s*'([a-z]+)'/g) ].map(m => m[1])) ].sort();
const missingTones = tones.filter(t => src.indexOf('.h5net-stat-icon.tone-' + t + '{') < 0);
report(tones.length === 8, 'status tiles declare 8 base tones', 'found ' + tones.length);
report(!missingTones.length, 'every base tone has a colour rule',
	missingTones.length ? 'missing ' + missingTones.join(', ') : '');
report(src.indexOf('.h5net-icon.tone-live') < 0 && src.indexOf('tone-live') > -1,
	'tone-live intentionally has no rule (card colour is kept)');

for (const st of [ 'st-warn', 'st-bad', 'st-off', 'is-idle' ]) {
	report(src.indexOf('.h5net-stat-icon.' + st) > -1, 'CSS defines .' + st);
}

// The exit card's structure and its state tones.  A missing tone rule means a
// degraded exit would keep the healthy green, which is the same class of defect
// the tile tones exist to prevent.
for (const ec of [ 'h5net-ecard', 'svgbox', 'ec-content', 'ec-topline', 'ec-title',
	'ec-live', 'ec-dot', 'ec-meta', 'ec-badge', 'ec-iface', 'ec-role' ]) {
	report(src.indexOf('.' + ec) > -1, 'CSS defines .' + ec);
}
for (const tn of [ 'tone-pending', 'tone-down', 'tone-off' ]) {
	report(src.indexOf('.h5net-ecard.' + tn) > -1, 'exit card defines .' + tn);
}

// ---------------------------------------------------------------------------
// The lead card: two halves, one card, and the egress glyph
// ---------------------------------------------------------------------------
// The heading half of the card is not bare text next to a card any more, so the
// vocabulary both halves are built from has to exist, and the sibling header
// block it replaced has to be gone.
for (const cls of [ 'hero-slot', 'hero-div', 'ec-hint' ]) {
	report(src.indexOf('.' + cls) > -1, 'CSS defines .' + cls);
}
for (const st of [ 'is-up', 'is-pending', 'is-down', 'is-off' ]) {
	report(src.indexOf('.h5net .h5net-ecard .ec-live.' + st) > -1,
		'CSS gives .ec-live a ' + st + ' colour');
}
report(src.indexOf('h5net-head') < 0, 'the two-sibling header block is gone',
	'the header must be one card, not a heading beside one');
report(src.indexOf('.h5net .h5net-ecard.hero{flex-direction:column') > -1,
	'the lead card stacks instead of squeezing on narrow screens');

// The egress glyph.  Its selection class lives on the card rather than inside the
// markup, which is what keeps every class attribute in the SVG string literal and
// therefore readable here; these checks pin the other half of that contract - that
// each branch is switched on by its own state class, and that the two are never
// crossed, since a crossed binding would draw the traffic leaving the wrong way.
const egressSvg = svgs.filter(s => s.indexOf('eg-led') > -1);
report(egressSvg.length === 1, 'exactly one egress glyph', 'found ' + egressSvg.length);
if (egressSvg.length === 1) {
	const g = egressSvg[0];
	report((g.match(/class="eg-flow f-wan"/g) || []).length === 1 &&
		(g.match(/class="eg-flow f-modem"/g) || []).length === 1,
		'egress glyph draws one flow overlay per branch');
	report(g.indexOf('class="eg-port"') > -1, 'egress glyph draws the wired socket');
	report((g.match(/class="eg-bar"/g) || []).length === 3,
		'egress glyph draws three cellular bars');
	report(g.indexOf('viewBox="0 0 64 64"') > -1, 'egress glyph uses the shared 64x64 box');
}
for (const via of [ 'eg-via-wan', 'eg-via-modem', 'eg-via-both' ]) {
	report(src.indexOf('.h5net .h5net-ecard.' + via + ' ') > -1,
		'egress glyph binds ' + via);
}
report(src.indexOf('.h5net .h5net-ecard.eg-via-wan .f-wan{') > -1 &&
	src.indexOf('.h5net .h5net-ecard.eg-via-modem .f-modem{') > -1,
	'each branch animates under its own state class');
report(src.indexOf('eg-via-wan .f-modem') < 0 && src.indexOf('eg-via-modem .f-wan') < 0,
	'no crossed branch binding');
report(src.indexOf('.h5net .h5net-ecard.eg-via-none') < 0,
	'eg-via-none is left unbound on purpose: nothing carrying means nothing moves');
report(src.indexOf('.h5net .h5net-ecard.is-idle svg,.h5net .h5net-ecard.is-idle svg *') > -1,
	'a frozen lead card stops the glyph as well');

// ---------------------------------------------------------------------------
// CSS scope: a media query adds no specificity
// ---------------------------------------------------------------------------
// The base sheet scopes nested rules as ".h5net .h5net-ecard{...}" (0,2,0).  A
// responsive override written as ".h5net-ecard{...}" inside @media scores only
// 0,1,0 and therefore loses outright, however late it appears - it parses
// cleanly, reads correctly, and never applies.  Every selector that touches a
// nested-scoped class must carry the same .h5net ancestor.
const scopeToken = function(selector) {
	return /(^|[\s,>+~])\.h5net(?![-A-Za-z0-9_])/.test(selector);
};
const nestedClasses = new Set([ ...src.matchAll(/\.h5net\s[^{}]*/g) ]
	.flatMap(m => [ ...m[0].matchAll(/\.([A-Za-z0-9_-]+)/g) ].map(x => x[1]))
	.filter(n => n !== 'h5net'));

const mediaSelectors = literals
	.filter(l => l.indexOf('@media') > -1)
	.flatMap(l => [ ...l.matchAll(/([^{}]+)\{([^{}]*)\}/g) ].map(m => m[1]))
	.flatMap(sel => sel.split(','))
	.map(sel => sel.split('@').pop().trim())
	.filter(sel => sel.charAt(0) === '.');

const unscoped = mediaSelectors.filter(sel => !scopeToken(sel) &&
	[ ...sel.matchAll(/\.([A-Za-z0-9_-]+)/g) ].some(m => nestedClasses.has(m[1])));
console.log('\nresponsive selectors (' + mediaSelectors.length + '), nested-scoped classes (' +
	nestedClasses.size + ')');
report(!unscoped.length, 'every responsive selector carries the .h5net scope it overrides',
	unscoped.length ? 'unscoped: ' + unscoped.join(', ') : '');

// ---------------------------------------------------------------------------
// Front-end / back-end field contract
// ---------------------------------------------------------------------------

const emitted = (function() {
	// Only the status branch is the contract; other echo statements in the script
	// belong to other subcommands.
	const start = backendSrc.indexOf('print_status() {');
	if (start < 0) throw new Error('print_status() not found in ' + backend);
	const end = backendSrc.indexOf('\n}\n', start);
	return new Set([ ...backendSrc.slice(start, end < 0 ? backendSrc.length : end)
		.matchAll(/echo\s+"([a-z0-9_]+)=/g) ].map(m => m[1]));
})();

const consumed = new Set([ ...src.matchAll(/\bdata\.([a-z][a-z0-9_]*)/g) ].map(m => m[1]));
// connectionState()/routeCard() read the per-uplink families through a computed
// key, so expand those explicitly.  Note the two naming shapes: the plain flags
// are joined with an underscore, while the family flags are not (wan6_up,
// wan4_ready) - a single expansion rule would silently invent wan_6_up.
[ 'present', 'available', 'pending', 'carrier', 'up', 'device' ].forEach(function(suffix) {
	[ 'wan', 'modem' ].forEach(function(kind) { consumed.add(kind + '_' + suffix); });
});
[ '6_up', '4_ready', '6_ready' ].forEach(function(suffix) {
	[ 'wan', 'modem' ].forEach(function(kind) { consumed.add(kind + suffix); });
});

console.log('\nbackend emits ' + emitted.size + ' status keys, view reads ' + consumed.size + ' fields');
const notEmitted = [ ...consumed ].filter(f => !emitted.has(f)).sort();
report(!notEmitted.length, 'every field the view reads is emitted by status',
	notEmitted.length ? 'not emitted: ' + notEmitted.join(', ') : '');

const neverRead = [ ...emitted ].filter(f => !consumed.has(f)).sort();
console.log('emitted but not read by the view (' + neverRead.length + '): ' + (neverRead.join(', ') || '(none)'));

console.log('\n' + (failed ? failed + ' check(s) failed' : 'all checks passed'));
process.exit(failed ? 1 : 0);
