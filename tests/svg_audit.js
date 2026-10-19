// Audit the view/status field contract and the remaining animated SVGs.
//
// The simplified view keeps six SVGs (the two uplink card icons, the three
// exit-card variants and the egress glyph) and dropped the 8-tile status
// matrix with its decorative animation layer.  What is still audited here:
//
//   * the SVG string literals stay balanced and their animated classes stay
//     bound to CSS rules (a missing rule renders a frozen icon);
//   * every `animation:` target resolves to a declared @keyframes;
//   * the lead card's class vocabulary and state tones keep their rules;
//   * the responsive overrides carry the .h5net scope they override;
//   * and, most importantly, every field the view reads is emitted by the
//     backend status emitter - a field the backend stopped printing silently
//     becomes `undefined` in the UI.
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

// The 8-tile status matrix must stay gone: its data is carried by the hero
// card and the two uplink cards now.  (h5net-state / h5net-status are
// different, still-live class names - the character class keeps them legal.)
report(!/h5net-stat[-']/.test(src), 'the status tile matrix stays removed');
report(src.indexOf('statusTiles') < 0, 'statusTiles() stays removed');

// ---------------------------------------------------------------------------
// SVG fragments
// ---------------------------------------------------------------------------

const literals = [ ...src.matchAll(/'((?:[^'\\\n]|\\.)*)'/g) ].map(m => m[1]);
const flat = literals.join('\n');
const svgs = [ ...flat.matchAll(/<svg[^>]*>[\s\S]*?<\/svg>/g) ].map(m => m[0]);

console.log('\nSVG fragments found: ' + svgs.length +
	' (2 uplink card icons + 3 exit-card variants + 1 egress glyph expected)');
report(svgs.length === 6, 'SVG count is 6', 'found ' + svgs.length);
report(svgs.every(s => (s.match(/<svg/g) || []).length === (s.match(/<\/svg>/g) || []).length),
	'every SVG is balanced');

// The skeleton tile icon (an empty <svg></svg> frame) belonged to the removed
// matrix; no empty frame may come back.
report(svgs.every(s => !/^<svg[^>]*><\/svg>$/.test(s)),
	'no empty placeholder SVG frame', 'found ' +
	svgs.filter(s => /^<svg[^>]*><\/svg>$/.test(s)).length);

// ---------------------------------------------------------------------------
// CSS: keyframes, animation classes, tone and state classes
// ---------------------------------------------------------------------------

const keyframes = [ ...src.matchAll(/@keyframes\s+([A-Za-z0-9_-]+)/g) ].map(m => m[1]);
console.log('\n@keyframes declared (' + keyframes.length + '): ' + keyframes.join(', '));

const lastClassOf = function(selector) {
	const names = [ ...selector.matchAll(/\.([A-Za-z0-9_-]+)/g) ].map(m => m[1])
		.filter(n => n !== 'h5net');
	return names.length ? names[names.length - 1] : null;
};

const scopedRules = [ ...src.matchAll(/(\.h5net[^{}]*)\{([^{}]*)\}/g) ];
const animClasses = [ ...new Set(scopedRules
	.filter(m => m[2].indexOf('animation') > -1)
	.map(m => lastClassOf(m[1]))
	.filter(Boolean)) ].sort();
console.log('scoped animation classes (' + animClasses.length + '): ' + animClasses.join(', '));

// `animation:` may name a keyframe other than the class (sk -> h5net-sk), so
// check every referenced name resolves to a declared @keyframes.
const referenced = [ ...new Set([ ...src.matchAll(/animation:\s*([A-Za-z0-9_-]+)\s/g) ].map(m => m[1])) ];
const dangling = referenced.filter(k => keyframes.indexOf(k) < 0);
report(!dangling.length, 'every animation: target is declared',
	dangling.length ? 'dangling ' + dangling.join(', ') : '');

// The core animated vocabulary the remaining icons rely on.
const expectedClasses = [ 'svg-dash', 'svg-dash-fast', 'svg-pulse', 'svg-wave',
	'svg-wave2', 'svg-wave3' ];
const missingClasses = expectedClasses.filter(c => animClasses.indexOf(c) < 0);
report(!missingClasses.length, 'every core animation rule exists',
	missingClasses.length ? 'missing ' + missingClasses.join(', ') : '');

// Every animation class used in the markup must have a rule, otherwise the
// icon silently renders frozen.
const usedClasses = [ ...new Set(svgs.flatMap(s =>
	[ ...s.matchAll(/class="([^"]+)"/g) ].flatMap(m => m[1].split(/\s+/)))) ].sort();
const unbound = usedClasses.filter(c => animClasses.indexOf(c) < 0);
report(!unbound.length, 'every animated class used in markup is bound',
	unbound.length ? 'unbound ' + unbound.join(', ') : '');

// Retired animation vocabulary of the removed tile matrix must stay gone.
const retiredAnim = [ 'svg-spin-rev', 'svg-flow-dot' ].filter(c =>
	src.indexOf(c) > -1);
report(!retiredAnim.length, 'the retired tile animation classes stay gone',
	retiredAnim.join(', '));

// tone-live is deliberately absent (a healthy uplink keeps the card's own
// colour); the string still appears in iconTone().
report(src.indexOf('.h5net-icon.tone-live') < 0 && src.indexOf('tone-live') > -1,
	'tone-live intentionally has no rule (card colour is kept)');

// 旧版的 st-warn / st-bad / st-off 三态类已被 tone-* 体系取代,不再存在;
// is-idle 仍然需要一条规则,它负责把一个失效图标的动画停住。
report(src.indexOf('.h5net .is-idle') > -1, 'CSS defines .is-idle');
report(src.indexOf('st-warn') < 0 && src.indexOf('st-bad') < 0 && src.indexOf('st-off') < 0,
	'the retired .st-* state classes are gone',
	'either keep them absent or restore them with their own assertions');

// The exit card's structure and its state tones.  A missing tone rule means a
// degraded exit would keep the healthy green.
for (const ec of [ 'h5net-hero', 'hero-svgbox', 'hero-info', 'hero-topline', 'hero-title',
	'hero-badge', 'hero-pulse-dot', 'hero-meta', 'pill', 'iface', 'hero-hint' ]) {
	report(src.indexOf('.' + ec) > -1, 'CSS defines .' + ec);
}
// 旧版的 .h5net-ecard / .ec-* / .svgbox 词汇已被 .h5net-hero / .hero-* 取代。
report(src.indexOf('.h5net-ecard') < 0 && src.indexOf('.ec-title') < 0 &&
	src.indexOf('.svgbox') < 0, 'the retired .h5net-ecard / .ec-* vocabulary is gone');
for (const tn of [ 'tone-pending', 'tone-down', 'tone-off' ]) {
	report(src.indexOf('.h5net .hero-svgbox.' + tn) > -1, 'lead card defines .' + tn);
}

// ---------------------------------------------------------------------------
// The lead card: two halves, one card, and the egress glyph
// ---------------------------------------------------------------------------
for (const cls of [ 'hero-slot', 'hero-div', 'hero-hint' ]) {
	report(src.indexOf('.' + cls) > -1, 'CSS defines .' + cls);
}
for (const st of [ 'is-up', 'is-pending', 'is-down', 'is-off' ]) {
	report(src.indexOf('.h5net .hero-badge.' + st) > -1,
		'CSS gives .hero-badge a ' + st + ' colour');
}
report(src.indexOf('h5net-head') < 0, 'the two-sibling header block is gone',
	'the header must be one card, not a heading beside one');
report(src.indexOf('.h5net .h5net-hero{grid-template-columns:1fr') > -1,
	'the lead card stacks instead of squeezing on narrow screens');

const egressSvg = svgs.filter(s => s.indexOf('M22 28c12 0 10-14 24-14') > -1);
report(egressSvg.length === 1, 'exactly one egress glyph', 'found ' + egressSvg.length);
if (egressSvg.length === 1) {
	const g = egressSvg[0];
	report((g.match(/class="svg-dash"/g) || []).length === 2,
		'egress glyph draws one flow path per direction');
	report((g.match(/class="svg-pulse"/g) || []).length === 1,
		'egress glyph marks the local socket');
	report(g.indexOf('viewBox="0 0 64 64"') > -1, 'egress glyph uses the shared 64x64 box');
}
report(src.indexOf('eg-via-') < 0,
	'the retired eg-via-* branch selection is gone',
	'either keep it absent, or restore it together with its own assertions');
report(src.indexOf('.h5net .is-idle svg *') > -1,
	'a frozen card stops its icons as well');

// ---------------------------------------------------------------------------
// CSS scope: a media query adds no specificity
// ---------------------------------------------------------------------------
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
	// belong to other subcommands.  The backend may be the legacy POSIX script
	// (`echo "key=...`) or the Rust snapshot emitter (`line(&mut out, "key", ...`)
	// in src/status.rs); both are accepted so the audit keeps working after the
	// Rust rewrite.
	const start = backendSrc.indexOf('print_status() {');
	if (start >= 0) {
		const end = backendSrc.indexOf('\n}\n', start);
		return new Set([ ...backendSrc.slice(start, end < 0 ? backendSrc.length : end)
			.matchAll(/echo\s+"([a-z0-9_]+)=/g) ].map(m => m[1]));
	}
	const fstart = backendSrc.indexOf('fn print_status(');
	if (fstart < 0) throw new Error('print_status() not found in ' + backend);
	// Rust functions cannot be delimited by a `\n}\n` search (inner blocks end
	// the same way), so harvest the emitter pattern across the whole file.
	// rustfmt may fold the call onto one line or split it; `\s*` covers both.
	return new Set([ ...backendSrc.matchAll(/line\(\s*&mut out,\s*"([a-z0-9_]+)"/g) ].map(m => m[1]));
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

// New-backend adaptations the view must keep: the one-click align goes through
// the dedicated `align` command, and the split alert carries split_detail.
report(src.indexOf("[ 'align' ]") > -1, 'split repair calls the `align` command');
report(/data\.split_detail/.test(src), 'the split alert surfaces split_detail');
report(src.indexOf('reconcileExits') < 0,
	'the retired synchronous `reconcile` UI action stays gone');

const neverRead = [ ...emitted ].filter(f => !consumed.has(f)).sort();
console.log('emitted but not read by the view (' + neverRead.length + '): ' + (neverRead.join(', ') || '(none)'));

console.log('\n' + (failed ? failed + ' check(s) failed' : 'all checks passed'));
process.exit(failed ? 1 : 0);
