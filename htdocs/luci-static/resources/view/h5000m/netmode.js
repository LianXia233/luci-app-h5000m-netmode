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
			// --- the lead card -----------------------------------------------------
			// The page heading and the live exit readout are one card.  An earlier
			// revision kept them as two siblings with a rule between them, which left
			// the heading as bare text while the value beside it was a card.  Both
			// halves now carry the same .svgbox / .ec-topline / .ec-title / .ec-live /
			// .ec-meta vocabulary, so one set of rules styles both of them.
			//
			// The halves report different verdicts on purpose: the heading half says
			// whether the configured plan is still intact, the readout half says
			// which uplink is live.  Two pills both reading "online" inside one card
			// would be noise, and the plan verdict is the thing a dual-exit policy
			// can lose silently.
			'.h5net .h5net-ecard.hero{flex-wrap:nowrap;align-items:stretch;gap:0;margin:0 0 14px;padding:12px 18px 12px 13px}',
			'.h5net .h5net-ecard.hero .hero-slot{display:flex;align-items:center;gap:13px;flex:1 1 0;min-width:0}',
			'.h5net .h5net-ecard.hero .hero-div{flex:none;align-self:stretch;width:1px;margin:5px 16px;background:linear-gradient(180deg,transparent,rgba(139,149,161,.34),transparent)}',
			'.h5net .h5net-ecard .ec-hint{margin-top:6px;color:#929aa5;font-size:11px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}',
			// --- live egress card --------------------------------------------------
			// The design sheet's `.status` card, carrying the uplink that is actually
			// holding the default route.  Only one is rendered: the sheet's demo shows
			// a WAN card and a cellular card side by side, both labelled "在线", and
			// copying that markup as-is would print a state the device may not be in.
			//
			// The SVG markup inside the card, including its own class attributes
			// (wan-port / wan-flow / wan-led / wan-ring / cell-wave / cell-core /
			// cell-signal / cell-packet), is copied unchanged.  The sheet's wrapper
			// classes that are generic inside a LuCI page - .content / .topline /
			// .title / .live / .live-dot / .meta / .badge / .separator / .interface -
			// carry an ec- prefix so they can neither be caught by theme rules nor
			// leak onto other pages.  Every property value is the sheet's.
			'.h5net .h5net-ecard{--accent:var(--net-green);--soft:#e8f8f2;position:relative;min-width:0;overflow:hidden;display:flex;align-items:center;gap:13px;min-height:72px;padding:10px 17px 10px 11px;border:1px solid var(--border-color-low,#e8edf3);border-radius:18px;background:rgba(255,255,255,.82);box-shadow:0 12px 35px rgba(28,45,66,.07),0 2px 7px rgba(28,45,66,.025);flex:0 1 auto}',
			'.h5net .h5net-ecard.cell{--accent:#4f8dff;--soft:#edf4ff}',
			'.h5net .h5net-ecard.tone-pending{--accent:var(--net-amber);--soft:#fff5e9}',
			'.h5net .h5net-ecard.tone-down{--accent:var(--net-red);--soft:#fff0f2}',
			'.h5net .h5net-ecard.tone-off{--accent:#8b95a3;--soft:#f1f4f7}',
			'.h5net .h5net-ecard:before{content:"";position:absolute;left:0;top:13px;bottom:13px;width:3px;border-radius:0 4px 4px 0;background:var(--accent);opacity:.7}',
			'.h5net .h5net-ecard:after{content:"";position:absolute;width:130px;height:130px;right:-60px;top:-70px;border-radius:50%;background:var(--soft);opacity:.7;pointer-events:none}',
			'.h5net .h5net-ecard .svgbox{position:relative;z-index:1;width:50px;height:50px;flex:none;display:grid;place-items:center;border-radius:15px;background:var(--soft);color:var(--accent)}',
			'.h5net .h5net-ecard .svgbox svg{width:43px;height:43px;overflow:visible}',
			'.h5net .h5net-ecard .ec-content{position:relative;z-index:1;min-width:0;flex:1}',
			'.h5net .h5net-ecard .ec-topline{display:flex;align-items:center;gap:8px;min-width:0}',
			'.h5net .h5net-ecard .ec-title{margin:0;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:15px;font-weight:760;letter-spacing:.05px}',
			'.h5net .h5net-ecard .ec-live{display:inline-flex;align-items:center;gap:5px;flex:none;color:var(--accent);font-size:11px;font-weight:700;white-space:nowrap}',
			// Each half of the lead card states its own verdict, so the pill colour
			// cannot come from the card's accent alone: an intact plan can sit next to
			// a degraded link, and a pill has to keep meaning what it says.  The four
			// classes below reproduce exactly what the accent used to imply
			// (tone-live / tone-pending / tone-down / tone-off), so the readout half
			// renders as it did before.
			'.h5net .h5net-ecard .ec-live.is-up{color:var(--net-green)}',
			'.h5net .h5net-ecard .ec-live.is-pending{color:var(--net-amber)}',
			'.h5net .h5net-ecard .ec-live.is-down{color:var(--net-red)}',
			'.h5net .h5net-ecard .ec-live.is-off{color:#8b95a3}',
			'.h5net .h5net-ecard .ec-dot{width:6px;height:6px;border-radius:50%;background:currentColor;box-shadow:0 0 0 3px rgba(0,0,0,.05);animation:breath 1.8s ease-in-out infinite}',
			'.h5net .h5net-ecard .ec-meta{display:flex;align-items:center;gap:7px;margin-top:6px;min-width:0;color:#8b95a1;font-size:11px}',
			'.h5net .h5net-ecard .ec-badge{padding:4px 7px;border-radius:7px;background:var(--soft);color:var(--accent);font-weight:700;white-space:nowrap}',
			'.h5net .h5net-ecard .ec-sep{opacity:.35}',
			'.h5net .h5net-ecard .ec-iface{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;opacity:.8;white-space:nowrap}',
			'.h5net .h5net-ecard .ec-role{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}',
			// The sheet rings the live dot with color-mix(in srgb,currentColor 10%,
			// transparent).  That syntax is 2023+ only, and where it is unsupported the
			// declaration is dropped outright, which would cost the halo entirely - so
			// the ring is a flat shadow instead, which degrades to a plain dot.
			// A fault is still, here as everywhere else on this page: red and grey
			// cards stop animating, so motion keeps meaning "traffic is flowing".
			'.h5net .h5net-ecard.tone-down svg,.h5net .h5net-ecard.tone-down svg *,.h5net .h5net-ecard.tone-off svg,.h5net .h5net-ecard.tone-off svg *{animation:none!important}',
			'.h5net .h5net-ecard.is-idle svg,.h5net .h5net-ecard.is-idle svg *,.h5net .h5net-ecard.is-idle .ec-dot{animation:none!important}',
			'.h5net .h5net-ecard .wan-port{fill:none;stroke:currentColor;stroke-linecap:round;stroke-linejoin:round}',
			'.h5net .h5net-ecard .wan-flow{stroke-dasharray:4 4;animation:dash 1s linear infinite}',
			'.h5net .h5net-ecard .wan-led{animation:led 1.5s ease-in-out infinite}',
			'.h5net .h5net-ecard .wan-ring{transform-box:fill-box;transform-origin:center;animation:ec-ring 2.4s ease-out infinite}',
			'.h5net .h5net-ecard .cell-wave{fill:none;stroke:currentColor;stroke-linecap:round;transform-box:fill-box;transform-origin:center;animation:ec-wave 1.9s ease-out infinite}',
			'.h5net .h5net-ecard .cell-wave.w2{animation-delay:.63s}',
			'.h5net .h5net-ecard .cell-wave.w3{animation-delay:1.26s}',
			'.h5net .h5net-ecard .cell-core{animation:breath 1.5s ease-in-out infinite}',
			'.h5net .h5net-ecard .cell-signal{stroke-dasharray:3 5;animation:dash 1.15s linear infinite}',
			'.h5net .h5net-ecard .cell-packet{animation:packet 1.7s ease-in-out infinite}',
			'.h5net .h5net-ecard .cell-packet.p2{animation-delay:.85s}',
			// The egress glyph in the lead card's heading half: the router and the two
			// uplinks it can leave through.  Which branch animates is decided by the
			// card's eg-via-* state class, so that half states which way the traffic
			// is leaving instead of only naming the page.  A branch that is not
			// carrying keeps a still, faint line.  Each state rule is written on its
			// own so that every class in the markup is matched by the rule that
			// animates it, which is what the audit script reads.
			//
			// eg-via-none deliberately has no rule: no carrying branch is already the
			// default rendering, which is "neither line is moving".
			'.h5net .h5net-ecard .eg-dev{fill:none;stroke:currentColor;stroke-linecap:round;stroke-linejoin:round}',
			'.h5net .h5net-ecard .eg-port{fill:none;stroke:currentColor;stroke-linecap:round;stroke-linejoin:round}',
			'.h5net .h5net-ecard .eg-link{fill:none;stroke:currentColor;stroke-linecap:round;opacity:.3}',
			'.h5net .h5net-ecard .eg-flow{fill:none;stroke:currentColor;stroke-linecap:round;stroke-width:2.4;stroke-dasharray:4 4;opacity:0}',
			'.h5net .h5net-ecard .eg-bar{fill:currentColor;opacity:.3}',
			'.h5net .h5net-ecard .eg-led{animation:led 1.5s ease-in-out infinite}',
			'.h5net .h5net-ecard.eg-via-wan .f-wan{opacity:1;animation:dash 1.05s linear infinite}',
			'.h5net .h5net-ecard.eg-via-modem .f-modem{opacity:1;animation:dash 1.05s linear infinite}',
			'.h5net .h5net-ecard.eg-via-both .eg-flow{opacity:1;animation:dash 1.05s linear infinite}',
			'.h5net .h5net-ecard.eg-via-wan .eg-port{fill:currentColor;fill-opacity:.18}',
			'.h5net .h5net-ecard.eg-via-both .eg-port{fill:currentColor;fill-opacity:.18}',
			'.h5net .h5net-ecard.eg-via-modem .eg-bar{opacity:1}',
			'.h5net .h5net-ecard.eg-via-both .eg-bar{opacity:1}',
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
			// --- animated SVG icons ------------------------------------------------
			// Reproduced verbatim from the supplied design sheet.  The keyframe names
			// (pulse / flow / wave / ring / signal / travel / progress / upload /
			// scan / node / breath / led / dash / packet) are kept exactly as written:
			// the LuCI theme only defines
			// aurora-fade-in, divider-in, sidebar-run-* and spin, so there is no
			// collision.  Only the *class selectors* carry a .h5net scope, because
			// generic names such as .node / .progress / .scan must not leak into
			// other LuCI pages.  The SVG markup and its own class attributes are
			// untouched.
			'.h5net .h5net-icon svg{width:38px;height:38px;overflow:visible}',
			'.h5net .pulse{animation:pulse 1.8s ease-in-out infinite}',
			'.h5net .flow{animation:flow 1.4s linear infinite}',
			'.h5net .ring{transform-origin:50% 50%;animation:ring 2.4s linear infinite}',
			'.h5net .signal{transform-origin:18px 18px;animation:signal 1.5s ease-in-out infinite}',
			'.h5net .dash{animation:flow 1.25s linear infinite}',
			'.h5net .blink{animation:pulse 1.45s ease-in-out infinite}',
			'.h5net .wave{transform-origin:32px 38px;animation:wave 1.5s ease-in-out infinite}',
			'.h5net .wave2{animation-delay:.25s}',
			'.h5net .orbit{transform-origin:32px 32px;animation:ring 3.5s linear infinite}',
			'.h5net .travel{animation:travel 1.7s ease-in-out infinite}',
			'.h5net .progress{transform-origin:32px 32px;animation:progress 2s ease-in-out infinite}',
			'.h5net .upload{animation:upload 1.35s ease-in-out infinite}',
			'.h5net .scan{animation:scan 1.7s ease-in-out infinite}',
			'.h5net .scan2{animation-delay:.18s}.h5net .scan3{animation-delay:.36s}',
			'.h5net .route{animation:flow 1.1s linear infinite}',
			'.h5net .node{animation:node 1.4s ease-in-out infinite}',
			'@keyframes pulse{0%,100%{opacity:1}50%{opacity:.42}}',
			'@keyframes flow{from{stroke-dashoffset:0}to{stroke-dashoffset:-24}}',
			'@keyframes wave{0%,100%{opacity:.25;transform:scale(.94)}50%{opacity:1;transform:scale(1)}}',
			'@keyframes ring{to{transform:rotate(360deg)}}',
			'@keyframes signal{0%,100%{opacity:.45}50%{opacity:1}}',
			'@keyframes travel{0%,100%{transform:translateX(0);opacity:.35}50%{transform:translateX(29px);opacity:1}}',
			'@keyframes progress{0%{stroke-dashoffset:0;transform:rotate(-90deg)}50%{stroke-dashoffset:35;transform:rotate(90deg)}100%{stroke-dashoffset:0;transform:rotate(270deg)}}',
			'@keyframes upload{0%,100%{transform:translateY(3px);opacity:.45}50%{transform:translateY(-3px);opacity:1}}',
			'@keyframes scan{0%,100%{transform:translateX(0);opacity:.35}50%{transform:translateX(29px);opacity:1}}',
			'@keyframes node{0%,100%{opacity:.35}50%{opacity:1}}',
			// The exit card's keyframes, transcribed from the sheet.  breath / led / dash /
			// packet keep the sheet's names.  ring and wave had to be renamed: this page
			// already defines both for its tile icons with a different body (ring =
			// rotate, wave = a slow opacity breathe), and a second definition under the
			// same name would silently replace the tiles' motion.  The card's rules
			// above bind the renamed keyframes, so the rendered motion is the sheet's.
			'@keyframes breath{0%,100%{opacity:.55;transform:scale(.9)}50%{opacity:1;transform:scale(1.06)}}',
			'@keyframes led{0%,100%{opacity:.35}50%{opacity:1}}',
			'@keyframes dash{to{stroke-dashoffset:-16}}',
			'@keyframes ec-ring{0%{transform:scale(.55);opacity:.55}100%{transform:scale(1.5);opacity:0}}',
			'@keyframes ec-wave{0%{transform:scale(.72);opacity:.7}70%,100%{transform:scale(1.08);opacity:0}}',
			'@keyframes packet{0%,100%{transform:translateY(4px);opacity:.15}45%{opacity:1}50%{transform:translateY(-7px)}85%{opacity:.9}}',
			// The icon is part of the readout, not decoration: colour and motion are
			// driven by the same fields the cards use, so an icon that is moving
			// always means "this path is live right now".  A link that is down or
			// not configured stops animating entirely - a permanently spinning
			// indicator would claim activity that is not happening.
			'.h5net .h5net-icon.tone-pending{background:rgba(231,163,62,.12);color:var(--net-amber)}',
			'.h5net .h5net-icon.tone-down{background:rgba(228,95,95,.12);color:var(--net-red)}',
			'.h5net .h5net-icon.tone-off{background:rgba(120,132,148,.14);color:#8b95a3}',
			'.h5net .h5net-icon.tone-pending svg *{animation-duration:2.6s}',
			'.h5net .h5net-icon.tone-down svg,.h5net .h5net-icon.tone-down svg *{animation:none!important}',
			'.h5net .h5net-icon.tone-off svg,.h5net .h5net-icon.tone-off svg *{animation:none!important}',
			// --- live status tiles -------------------------------------------------
			// Each tile is one real subsystem.  The base tone is the sheet's colour
			// for that subsystem; the st-* / is-idle layer overrides it with the
			// live state, so colour means "state" and the sheet's palette only
			// survives while that subsystem is healthy.
			'.h5net-stat{margin:0 0 18px}',
			'.h5net-stat-grid{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:12px}',
			'.h5net-stat-item{min-width:0;padding:15px 13px 13px;background:rgba(255,255,255,.72);border:1px solid var(--border-color-low,#e8edf3);border-radius:15px;box-shadow:0 5px 18px rgba(33,48,73,.035)}',
			'.h5net-stat-icon{height:74px;display:grid;place-items:center;border-radius:12px;margin-bottom:11px;transition:color .3s,background .3s}',
			'.h5net-stat-icon svg{width:54px;height:54px}',
			'.h5net-stat-icon.tone-green{color:#2bb889;background:#edf9f5}',
			'.h5net-stat-icon.tone-blue{color:#4d8dff;background:#eef5ff}',
			'.h5net-stat-icon.tone-mint{color:#30b995;background:#eaf9f4}',
			'.h5net-stat-icon.tone-orange{color:#ed9b43;background:#fff5e9}',
			'.h5net-stat-icon.tone-purple{color:#8d79e8;background:#f3f0ff}',
			'.h5net-stat-icon.tone-cyan{color:#36aeca;background:#edf9fc}',
			'.h5net-stat-icon.tone-red{color:#e56f7b;background:#fff0f2}',
			'.h5net-stat-icon.tone-gray{color:#778392;background:#f1f4f7}',
			'.h5net-stat-icon.st-warn{color:#d08324;background:#fff5e9}',
			'.h5net-stat-icon.st-bad{color:#d9534f;background:#fff0f2}',
			'.h5net-stat-icon.st-off{color:#9aa4b1;background:#f2f4f7}',
			'.h5net-stat-icon.is-idle svg,.h5net-stat-icon.is-idle svg *{animation:none!important}',
			'.h5net-stat-item>b{display:block;font-size:14px;margin-bottom:6px}',
			'.h5net-stat-value{display:block;font-size:12.5px;font-weight:600;color:var(--text-color,#39424e);overflow-wrap:anywhere}',
			'.h5net-stat-hint{display:block;margin-top:4px;color:#929aa5;font-size:11px;overflow-wrap:anywhere}',
			'@media(max-width:900px){.h5net-stat-grid{grid-template-columns:repeat(2,minmax(0,1fr))}.h5net .h5net-ecard.hero{flex-direction:column;gap:11px}.h5net .h5net-ecard.hero .hero-div{align-self:stretch;width:auto;height:1px;margin:0;background:linear-gradient(90deg,transparent,rgba(139,149,161,.34),transparent)}}',
			'@media(max-width:620px){.h5net .h5net-ecard .ec-hint{display:none}.h5net .h5net-ecard .hero-slot{gap:11px}.h5net-grid{grid-template-columns:1fr}.h5net-foot{display:block}.h5net-buttons{margin-top:12px;flex-direction:column}.h5net-buttons .cbi-button{width:100%}.h5net-stat-grid{gap:9px}.h5net-stat-item{padding:12px 9px;border-radius:12px}.h5net-stat-icon{height:64px;margin-bottom:9px;border-radius:10px}.h5net-stat-icon svg{width:46px;height:46px}.h5net-stat-item>b{font-size:13px}.h5net-stat-hint{font-size:10px}.h5net .h5net-ecard{min-height:68px;border-radius:16px;padding-right:14px}.h5net .h5net-ecard .svgbox{width:46px;height:46px;border-radius:13px}.h5net .h5net-ecard .svgbox svg{width:39px;height:39px}.h5net .h5net-ecard .ec-title{font-size:14px}.h5net .h5net-ecard .ec-meta,.h5net .h5net-ecard .ec-live{font-size:10px}}'
		].join(''));
	},

	// Animated icon for an uplink card, transcribed verbatim from the design sheet
	// (48x48 viewBox, animation classes flow / wave / pulse / signal).
	//
	// The markup is injected through innerHTML rather than built with E(): LuCI's
	// E() calls document.createElement(), which yields an HTMLUnknownElement for
	// SVG tag names, so an E()-built <svg> renders nothing.  Going through
	// innerHTML lets the source markup be copied unchanged.
	iconSvg: function(kind) {
		if (kind === 'modem') {
			return '<svg viewBox="0 0 48 48" aria-hidden="true">'
				+ '<path d="M10 30a15 15 0 0 1 28 0" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" opacity=".18"/>'
				+ '<path d="M15 30a10 10 0 0 1 18 0" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" opacity=".38" class="wave"/>'
				+ '<path d="M20 30a5 5 0 0 1 8 0" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" class="wave"/>'
				+ '<circle cx="24" cy="31" r="2.8" fill="currentColor" class="pulse"/>'
				+ '<path d="M13 13h10v6H17v5h6" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linejoin="round"/>'
				+ '<path d="M28 24v-5l7-7M35 12v7h-7" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"/>'
				+ '<circle cx="35" cy="12" r="2" fill="currentColor" class="signal"/>'
				+ '</svg>';
		}

		return '<svg viewBox="0 0 48 48" aria-hidden="true">'
			+ '<rect x="8" y="8" width="32" height="22" rx="5" fill="none" stroke="currentColor" stroke-width="2.4"/>'
			+ '<path d="M15 36h18M19 30v6m10-6v6" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round"/>'
			+ '<path class="flow" d="M14 19h20M14 24h12" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-dasharray="5 4"/>'
			+ '<circle cx="34" cy="19" r="2.1" fill="currentColor" class="pulse"/>'
			+ '</svg>';
	},

	// Animated icon for the live-egress card, transcribed from the design sheet
	// (64x64 viewBox, the sheet's class names and the sheet's animation classes).
	//
	// Three variants, because the card can only show an uplink that is really
	// carrying traffic: the sheet's WAN card and the sheet's cellular card, plus
	// the sheet's own routing icon for the case where the default route belongs to
	// neither uplink.  Painting a WAN icon over a foreign route, or over no route
	// at all, would be the same defect this page was rebuilt to remove.
	//
	// Injected through innerHTML for the reason documented on iconSvg(): E() builds
	// nodes with document.createElement(), which has no SVG namespace.
	exitIconSvg: function(kind) {
		if (kind === 'modem') {
			return '<svg viewBox="0 0 64 64" aria-label="' + _('蜂窝网络') + '">'
				+ '<path class="cell-wave" d="M23 25a13 13 0 0 1 18 0" stroke-width="2.3"/>'
				+ '<path class="cell-wave w2" d="M17 19a22 22 0 0 1 30 0" stroke-width="2"/>'
				+ '<path class="cell-wave w3" d="M11 13a31 31 0 0 1 42 0" stroke-width="1.8" opacity=".55"/>'
				+ '<path d="M32 28 25 52h14L32 28Z" fill="currentColor" opacity=".08"/>'
				+ '<path d="M32 29 25 52m7-23 7 23M28 42h8m-10 6h12M21 56h22" class="wan-port" stroke-width="2.2"/>'
				+ '<circle class="cell-core" cx="32" cy="25" r="3.2" fill="currentColor"/>'
				+ '<path class="cell-signal" d="M32 25V9" stroke="currentColor" stroke-width="1.7" opacity=".4"/>'
				+ '<circle class="cell-packet" cx="32" cy="25" r="2" fill="currentColor"/>'
				+ '<circle class="cell-packet p2" cx="32" cy="25" r="1.7" fill="currentColor"/>'
				+ '</svg>';
		}
		if (kind === 'route') {
			return '<svg viewBox="0 0 64 64" aria-label="' + _('其他路由') + '">'
				+ '<path class="route" d="M13 45C22 45 20 19 32 19s10 26 19 26" fill="none" stroke="currentColor" stroke-width="2.7" stroke-linecap="round" stroke-dasharray="5 5"/>'
				+ '<circle cx="13" cy="45" r="5" fill="currentColor"/>'
				+ '<circle cx="51" cy="45" r="5" fill="currentColor"/>'
				+ '<circle class="node" cx="32" cy="19" r="5" fill="currentColor"/>'
				+ '</svg>';
		}
		return '<svg viewBox="0 0 64 64" aria-label="' + _('有线 WAN') + '">'
			+ '<circle class="wan-ring" cx="32" cy="32" r="19" fill="none" stroke="currentColor" stroke-width="1.5" opacity=".35"/>'
			+ '<rect x="17" y="14" width="30" height="25" rx="7" fill="currentColor" opacity=".08"/>'
			+ '<rect class="wan-port" x="20" y="17" width="24" height="20" rx="5" stroke-width="2.3"/>'
			+ '<path class="wan-port" d="M26 37v10m12-10v10M23 50h18" stroke-width="2.3"/>'
			+ '<path class="wan-port" d="M25 23h14M25 28h14M25 33h7" stroke-width="1.8" opacity=".32"/>'
			+ '<circle class="wan-led" cx="39" cy="33" r="2.4" fill="currentColor"/>'
			+ '<path class="wan-port wan-flow" d="M21 55h22" stroke-width="2"/>'
			+ '</svg>';
	},

	// The lead card's heading half: the router, the wired socket, the cellular
	// bars, and the two branches joining them.  The branch holding the default
	// route is the one whose dashes flow, with its socket or its bars filled in.
	//
	// Which branch is selected comes from the card's eg-via-* class rather than
	// from a token spliced into this string, which keeps every class attribute
	// literal: the audit script reconstructs the markup by harvesting string
	// literals, and a concatenated attribute would be invisible to it.
	egressIconSvg: function() {
		return '<svg viewBox="0 0 64 64" aria-label="' + _('网络出口') + '">'
			+ '<g class="eg-port">'
			+ '<rect x="45" y="9" width="13" height="11" rx="3.5" stroke-width="2"/>'
			+ '<path d="M49 20v3.5m5-3.5v3.5M47.5 23.5h8" stroke-width="2"/>'
			+ '</g>'
			+ '<rect class="eg-bar" x="45" y="45" width="3.2" height="8" rx="1.6"/>'
			+ '<rect class="eg-bar" x="50.4" y="41" width="3.2" height="12" rx="1.6"/>'
			+ '<rect class="eg-bar" x="55.8" y="37" width="3.2" height="16" rx="1.6"/>'
			+ '<rect class="eg-dev" x="4" y="25" width="16" height="14" rx="5" stroke-width="2.2"/>'
			+ '<circle class="eg-led" cx="12" cy="32" r="2.1" fill="currentColor"/>'
			+ '<path class="eg-link" d="M20 29c10 0 10-14 23-14"/>'
			+ '<path class="eg-flow f-wan" d="M20 29c10 0 10-14 23-14"/>'
			+ '<path class="eg-link" d="M20 35c10 0 10 14 23 14"/>'
			+ '<path class="eg-flow f-modem" d="M20 35c10 0 10 14 23 14"/>'
			+ '</svg>';
	},

	// The live egress readout, as data rather than as markup.  Split out of
	// exitCard() so that the card's own state classes and the half it renders
	// cannot disagree about which state the device is in.
	//
	// Every string here is derived from the status output.  The sheet's demo text
	// ("current exit: wired WAN" next to "current exit: cellular", both "online")
	// describes a state this device cannot be in: one uplink carries the traffic,
	// and if both are up under two different families that is a split, which the
	// card reports as a fault rather than as two tidy green cards.
	exitVerdict: function(data) {
		var active4 = data.active4 || 'none';
		var active6 = data.active6 || 'none';
		var split = data.split === '1';
		var active = active4 !== 'none' ? active4 : active6;
		var daed = data.daed_exit_state || 'none';
		var self = this;

		var tone = 'tone-live', idle = false, kind, title, live, liveCls, badge, iface, role;

		// The policy order, not the current carrier: when the backup is the one
		// carrying traffic, that is exactly what the reader needs to see.
		function roleOf(k) {
			var order = self.modeOrder(data.mode);
			if (order.indexOf(k) < 0) return _('未纳入策略');
			if (order[0] === k) return _('首选出口');
			return (k === active) ? _('备用出口 · 已接管') : _('备用出口');
		}

		if (split) {
			// A family split is the invariant this app exists to protect, so it takes
			// precedence over the "current exit" reading - there are two of them.
			kind = active4 === 'modem' ? 'modem' : 'wan';
			tone = 'tone-down';
			idle = true;
			title = _('出口分流');
			live = _('告警');
			liveCls = 'is-down';
			badge = _('IPv4 ') + this.exitLabel(active4);
			iface = this.exitDevice(data, active4);
			role = _('IPv6 ') + this.exitLabel(active6);
		}
		else if (active === 'wan' || active === 'modem') {
			var state = this.connectionState(data, active);
			kind = active;
			tone = (state.cls === 'up') ? 'tone-live'
				: (state.cls === 'pending') ? 'tone-pending'
				: (state.cls === 'idle') ? 'tone-off' : 'tone-down';
			idle = tone === 'tone-down' || tone === 'tone-off';
			title = _('当前出口：%s').format(this.exitLabel(active));
			live = (state.cls === 'up') ? _('在线') : state.label;
			liveCls = (state.cls === 'up') ? 'is-up'
				: (state.cls === 'pending') ? 'is-pending'
				: (state.cls === 'idle') ? 'is-off' : 'is-down';
			badge = (active === 'wan') ? 'Ethernet' : '5G / LTE';
			iface = (active === 'wan' ? data.wan_device : data.modem_device) || _('未指定');
			role = roleOf(active);
		}
		else if (active === 'other') {
			// A foreign default route (mwan3 / VPN / daed).  It is up and it is
			// carrying traffic, but it is not ours, so it neither animates green nor
			// claims a position in this app's policy.
			kind = 'route';
			tone = 'tone-pending';
			liveCls = 'is-pending';
			title = _('当前出口：其他路由');
			live = _('外部接管');
			badge = (daed !== 'none') ? (_('daed 接管') ) : _('外部路由');
			iface = data.egress4 || this.exitDevice(data, active4);
			role = _('不在本插件策略内');
		}
		else {
			kind = 'route';
			tone = 'tone-off';
			idle = true;
			liveCls = 'is-off';
			title = _('无可用出口');
			live = _('离线');
			badge = _('无出口');
			iface = '—';
			role = _('策略：') + this.modeLabel(data.mode);
		}

		// Which of the two drawn branches carries traffic.  Both is a split;
		// neither means the traffic is not leaving through this app's uplinks at
		// all, and the glyph then shows no motion.
		var carriesWan = (active4 === 'wan' || active6 === 'wan');
		var carriesModem = (active4 === 'modem' || active6 === 'modem');
		var via = (carriesWan && carriesModem) ? 'eg-via-both'
			: carriesWan ? 'eg-via-wan'
			: carriesModem ? 'eg-via-modem' : 'eg-via-none';

		return { tone: tone, idle: idle, kind: kind, title: title, live: live,
			liveCls: liveCls, badge: badge, iface: iface, role: role, via: via };
	},

	// The lead card of the page: the configured plan on the left, the live exit on
	// the right, in one card.  Both halves are built through the same vocabulary,
	// so neither is styled as an exception to the other.
	//
	// The card's tone still comes from the live exit alone.  Toning it by the plan
	// verdict as well would turn it amber for a deliberate "only this exit" choice,
	// and colour that no longer means "something is wrong right now" is worse than
	// no colour; the plan states that in its own pill, inside its own half.
	exitCard: function(data) {
		var v = this.exitVerdict(data);
		var card = E('article', {
			'class': 'h5net-ecard hero' + (v.kind === 'modem' ? ' cell' : '') + ' '
				+ v.tone + (v.idle ? ' is-idle' : '') + ' ' + v.via
		});

		card.appendChild(this.egressSlot(data));
		card.appendChild(E('span', { 'class': 'hero-div' }));
		card.appendChild(this.exitSlot(v));

		return card;
	},

	// The plan half.  Its verdict is deliberately not the link verdict: the two
	// facts can disagree, because a healthy link under a plan whose backup has
	// gone away is a failover that no longer exists - which is the failure this
	// app was rebuilt to stop hiding.  The ladder below is exhaustive over the
	// states the backend can report.
	egressSlot: function(data) {
		var self = this;
		var order = this.modeOrder(data.mode);
		var active = (data.active4 && data.active4 !== 'none') ? data.active4
			: ((data.active6 && data.active6 !== 'none') ? data.active6 : 'none');

		var live, liveCls;
		if (data.split === '1') {
			live = _('分流告警'); liveCls = 'is-down';
		}
		else if (active === 'none') {
			live = _('无默认路由'); liveCls = 'is-off';
		}
		else if (active === 'other') {
			live = _('策略被绕过'); liveCls = 'is-pending';
		}
		else if (order.length === 1) {
			live = _('单出口运行'); liveCls = 'is-pending';
		}
		else if (order.filter(function(k) {
			return self.connectionState(data, k).cls === 'up';
		}).length < order.length) {
			live = _('无备用链路'); liveCls = 'is-pending';
		}
		else {
			live = _('主备就绪'); liveCls = 'is-up';
		}

		// The plan is printed in the order it is configured: the number is the
		// policy position, which is what the cards below let the user change.
		var plan = order.map(function(k, i) {
			return (order.length > 1 ? (i + 1) + ' ' : '') + self.exitLabel(k);
		}).join(' · ');

		var hint = _('点击连接卡片切换首选出口，“仅用此出口”会移除备用链路。');

		var slot = E('div', { 'class': 'hero-slot hero-egress' });
		var box = E('div', { 'class': 'svgbox' });
		box.innerHTML = this.egressIconSvg();

		slot.appendChild(box);
		slot.appendChild(E('div', { 'class': 'ec-content' }, [
			E('div', { 'class': 'ec-topline' }, [
				E('h2', { 'class': 'ec-title' }, _('网络出口')),
				E('div', { 'class': 'ec-live ' + liveCls }, [
					E('i', { 'class': 'ec-dot' }),
					E('span', {}, live)
				])
			]),
			E('div', { 'class': 'ec-meta' }, [
				E('span', { 'class': 'ec-badge' }, this.modeLabel(data.mode)),
				E('span', { 'class': 'ec-sep' }, '·'),
				E('span', { 'class': 'ec-role' }, plan)
			]),
			E('div', { 'class': 'ec-hint', 'title': hint }, hint)
		]));

		return slot;
	},

	// The readout half: which uplink holds the default route, its real netdev, its
	// real place in the configured policy, and the two family verdicts when they
	// disagree.
	exitSlot: function(v) {
		var slot = E('div', { 'class': 'hero-slot hero-exit' });
		var box = E('div', { 'class': 'svgbox' });
		box.innerHTML = this.exitIconSvg(v.kind);

		slot.appendChild(box);
		slot.appendChild(E('div', { 'class': 'ec-content' }, [
			E('div', { 'class': 'ec-topline' }, [
				E('div', { 'class': 'ec-title' }, v.title),
				E('div', { 'class': 'ec-live ' + v.liveCls }, [
					E('i', { 'class': 'ec-dot' }),
					E('span', {}, v.live)
				])
			]),
			E('div', { 'class': 'ec-meta' }, [
				E('span', { 'class': 'ec-badge' }, v.badge),
				E('span', { 'class': 'ec-sep' }, '·'),
				E('span', { 'class': 'ec-iface' }, v.iface),
				E('span', { 'class': 'ec-sep' }, '·'),
				E('span', { 'class': 'ec-role' }, v.role)
			])
		]));

		return slot;
	},

	exitDevice: function(data, exit) {
		if (exit === 'wan') return data.wan_device || '—';
		if (exit === 'modem') return data.modem_device || '—';
		return data.egress4 || '—';
	},

	// Maps the graded link state onto the icon tone.  `tone-live` is deliberately
	// not defined in CSS: a healthy uplink keeps the card's own colour, and only
	// the degraded states override it.
	iconTone: function(state) {
		if (state.cls === 'up') return 'tone-live';
		if (state.cls === 'pending') return 'tone-pending';
		if (state.cls === 'idle') return 'tone-off';
		return 'tone-down';
	},

	// Live status tiles.  Every animated SVG from the design sheet is bound to one
	// real subsystem and reports that subsystem's actual value: on this page the
	// sheet's icons are a readout, not a gallery.  A tile animates only while the
	// thing it depicts is actually happening, so a moving icon always means
	// "live", never "decorative".
	//
	// Sources, all from the backend status output that the page polls every five
	// seconds (no value below is invented or hard-coded):
	//   WAN / 5G      wan_*, modem_* liveness plus per-family route readiness
	//   Wi-Fi         wifi_total / wifi_up / wifi_ssid / wifi_clients
	//   forwarding    egress4 / egress6 / split
	//   failover      watcher / watch_interval / mode
	//   IPv6          active6 / egress6 / split
	//   health probe  health_check / wan_health / modem_health
	//   routing       active4 / daed_exit_state
	//
	// One source defect in the sheet was corrected: the Wi-Fi entry ships its
	// <path> elements without an enclosing <svg>.  Bare SVG shape elements in the
	// HTML namespace are never rendered, and the sheet's own rule
	// `.svg-preview svg,.svg-preview>path` shows the wrapper was intended, so the
	// missing <svg viewBox="0 0 64 64"> was added to match the other seven.
	statusTiles: function(data) {
		var wan = this.connectionState(data, 'wan');
		var modem = this.connectionState(data, 'modem');
		var e4 = data.egress4 || 'none';
		var e6 = data.egress6 || 'none';
		var split = data.split === '1';
		var watcherOn = data.watcher === 'on';
		var healthOn = data.health_check === '1';
		var active4 = data.active4 || 'none';
		var active6 = data.active6 || 'none';
		var daed = data.daed_exit_state || 'none';
		var wifiTotal = Number(data.wifi_total || 0) || 0;
		var wifiUp = Number(data.wifi_up || 0) || 0;
		var wifiClients = Number(data.wifi_clients || 0) || 0;

		function ready(v) { return v === '1' ? _('就绪') : _('不可用'); }
		function probe(v) {
			if (v === '1') return _('正常');
			if (v === '0') return _('异常');
			return _('未探测');
		}
		function liveness(state) {
			if (state.cls === 'up') return 'ok';
			if (state.cls === 'pending') return 'warn';
			if (state.cls === 'idle') return 'off';
			return 'bad';
		}

		var items = [
			{
				tone: 'green',
				name: _('有线 WAN'),
				value: (data.wan_device || _('未指定')) + ' · ' + wan.label,
				hint: _('IPv4 ') + ready(data.wan4_ready) + ' · ' + _('IPv6 ') + ready(data.wan6_ready),
				state: liveness(wan),
				live: wan.cls === 'up',
				svg: '<svg viewBox="0 0 64 64">'
					+ '<rect x="14" y="11" width="36" height="28" rx="7" fill="none" stroke="currentColor" stroke-width="2.8"/>'
					+ '<path d="M21 47h22M26 39v8m12-8v8" fill="none" stroke="currentColor" stroke-width="2.8" stroke-linecap="round"/>'
					+ '<path class="dash" d="M21 21h22M21 28h15" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-dasharray="6 5"/>'
					+ '<circle class="blink" cx="43" cy="21" r="2.7" fill="currentColor"/>'
					+ '</svg>'
			},
			{
				tone: 'blue',
				name: 'Wi‑Fi',
				value: wifiTotal ? _('%s/%s 射频在线').format(wifiUp, wifiTotal) : _('未检测到射频'),
				hint: data.wifi_ssid
					? data.wifi_ssid + ' · ' + _('%s 台客户端').format(wifiClients)
					: _('无线未启用'),
				state: !wifiTotal ? 'off' : (wifiUp === wifiTotal ? 'ok' : (wifiUp > 0 ? 'warn' : 'bad')),
				live: wifiUp > 0,
				svg: '<svg viewBox="0 0 64 64">'
					+ '<path d="M16 38a18 18 0 0 1 32 0" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" opacity=".18"/>'
					+ '<path class="wave" d="M22 38a11 11 0 0 1 20 0" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round"/>'
					+ '<path class="wave wave2" d="M28 38a5 5 0 0 1 8 0" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round"/>'
					+ '<circle class="blink" cx="32" cy="43" r="3.5" fill="currentColor"/>'
					+ '</svg>'
			},
			{
				tone: 'mint',
				name: _('5G 模组'),
				value: (data.modem_device || _('未指定')) + ' · ' + modem.label,
				hint: _('IPv4 ') + ready(data.modem4_ready) + ' · ' + _('IPv6 ') + ready(data.modem6_ready),
				state: liveness(modem),
				live: modem.cls === 'up',
				svg: '<svg viewBox="0 0 64 64">'
					+ '<rect x="13" y="13" width="38" height="38" rx="9" fill="none" stroke="currentColor" stroke-width="2.8"/>'
					+ '<text x="32" y="39" text-anchor="middle" font-size="19" font-weight="800" fill="currentColor">5G</text>'
					+ '<circle class="orbit" cx="32" cy="32" r="25" fill="none" stroke="currentColor" stroke-width="1.8" stroke-dasharray="3 7" opacity=".45"/>'
					+ '</svg>'
			},
			{
				tone: 'orange',
				name: _('流量转发'),
				value: split ? (e4 + ' / ' + e6) : (e4 + ' → ' + e6),
				hint: split ? _('IPv4 与 IPv6 出口不一致') : _('IPv4 与 IPv6 同出口'),
				state: split ? 'bad' : ((e4 !== 'none' || e6 !== 'none') ? 'ok' : 'bad'),
				live: !split && (e4 !== 'none' || e6 !== 'none'),
				svg: '<svg viewBox="0 0 64 64">'
					+ '<path d="M16 32h32M38 23l10 9-10 9" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"/>'
					+ '<circle class="travel" cx="18" cy="32" r="4" fill="currentColor"/>'
					+ '<path d="M12 19h16M36 45h16" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" opacity=".35"/>'
					+ '</svg>'
			},
			{
				tone: 'purple',
				name: _('自动切换'),
				value: watcherOn ? _('运行中 · 每 %s 秒').format(data.watch_interval || '10') : _('已停止'),
				hint: _('策略：') + this.modeLabel(data.mode),
				state: watcherOn ? 'ok' : 'warn',
				live: watcherOn,
				svg: '<svg viewBox="0 0 64 64">'
					+ '<circle cx="32" cy="32" r="22" fill="none" stroke="currentColor" stroke-width="2.2" opacity=".18"/>'
					+ '<circle class="progress" cx="32" cy="32" r="17" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-dasharray="70 37"/>'
					+ '<path d="M25 32h14M32 25v14" stroke="currentColor" stroke-width="2.6" stroke-linecap="round"/>'
					+ '</svg>'
			},
			{
				tone: 'cyan',
				name: _('IPv6 / 上行'),
				value: active6 === 'none' ? _('已禁用') : (_('出口 ') + e6),
				hint: _('策略：IPv6 跟随 IPv4 出口'),
				state: split ? 'bad' : (active6 === 'none' ? 'off' : 'ok'),
				live: active6 !== 'none',
				svg: '<svg viewBox="0 0 64 64">'
					+ '<path d="M20 44h24a10 10 0 0 0 1-20 14 14 0 0 0-26-3 9 9 0 0 0 1 23Z" fill="none" stroke="currentColor" stroke-width="2.8"/>'
					+ '<path class="upload" d="M32 39V25m0 0-6 6m6-6 6 6" fill="none" stroke="currentColor" stroke-width="2.7" stroke-linecap="round" stroke-linejoin="round"/>'
					+ '</svg>'
			},
			{
				tone: 'red',
				name: _('链路检测'),
				value: healthOn
					? (_('WAN ') + probe(data.wan_health) + ' · ' + _('模组 ') + probe(data.modem_health))
					: _('未启用'),
				hint: healthOn ? _('探测公共 anycast 地址') : _('启用后可提前发现假连接'),
				state: !healthOn ? 'off' : ((data.wan_health === '0' || data.modem_health === '0') ? 'warn' : 'ok'),
				live: healthOn,
				svg: '<svg viewBox="0 0 64 64">'
					+ '<path d="M16 23h32M16 32h32M16 41h32" stroke="currentColor" stroke-width="3" stroke-linecap="round" opacity=".25"/>'
					+ '<circle class="scan" cx="17" cy="23" r="4" fill="currentColor"/>'
					+ '<circle class="scan scan2" cx="17" cy="32" r="4" fill="currentColor"/>'
					+ '<circle class="scan scan3" cx="17" cy="41" r="4" fill="currentColor"/>'
					+ '</svg>'
			},
			{
				tone: 'gray',
				name: _('智能路由'),
				value: active4 === 'none'
					? _('无默认路由')
					: (active4 === 'other' ? _('外部路由接管') : _('本插件自主选路')),
				hint: (daed !== 'none') ? (_('daed 出口 ') + daed) : _('FIB 查询 · 最低 metric 胜出'),
				state: active4 === 'none' ? 'bad' : (active4 === 'other' ? 'warn' : 'ok'),
				live: active4 !== 'none' && active4 !== 'other',
				svg: '<svg viewBox="0 0 64 64">'
					+ '<path class="route" d="M13 45C22 45 20 19 32 19s10 26 19 26" fill="none" stroke="currentColor" stroke-width="2.7" stroke-linecap="round" stroke-dasharray="5 5"/>'
					+ '<circle cx="13" cy="45" r="5" fill="currentColor"/>'
					+ '<circle cx="51" cy="45" r="5" fill="currentColor"/>'
					+ '<circle class="node" cx="32" cy="19" r="5" fill="currentColor"/>'
					+ '</svg>'
			}
		];

		// No title row here: the eight tiles are self-describing (each has its own
		// name + value + hint), and the card above already carries the page's
		// section heading.  Keeping a second <h2> competed with the "网络出口"
		// heading for the same visual level.
		return E('section', { 'class': 'h5net-stat' }, [
			E('div', { 'class': 'h5net-stat-grid' }, items.map(function(item) {
				// The markup goes in through innerHTML, not E(): E() uses
				// document.createElement(), which yields an HTMLUnknownElement for
				// SVG tag names, so an E()-built <svg> renders nothing at all.
				var icon = E('div', {
					'class': 'h5net-stat-icon tone-' + item.tone + ' st-' + item.state
						+ (item.live ? '' : ' is-idle')
				});
				icon.innerHTML = item.svg;
				return E('div', { 'class': 'h5net-stat-item' }, [
					icon,
					E('b', {}, item.name),
					E('span', { 'class': 'h5net-stat-value' }, item.value),
					E('span', { 'class': 'h5net-stat-hint' }, item.hint)
				]);
			}))
		]);
	},

	modeLabel: function(mode) {
		if (mode === 'modem_first') return _('5G 优先');
		if (mode === 'wan_only') return _('仅有线');
		if (mode === 'modem_only') return _('仅 5G');
		return _('有线优先');
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

		// The animated icon is injected as markup; see iconSvg() for why E() cannot
		// be used to build SVG elements in LuCI.  The tone class is what turns the
		// icon from decoration into a readout: an uplink that is down or still
		// negotiating stops animating instead of always looking busy.
		var iconBox = E('div', { 'class': 'h5net-icon ' + this.iconTone(state) });
		iconBox.innerHTML = this.iconSvg(kind);

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
					iconBox,
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
		var message = this.statusMessage(data);
		// The header is one card: the plan on the left, the live exit and the family
		// split on the right.  Both halves come out of exitCard(), so what the header
		// claims cannot drift from what the device is doing.
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
			// The heading is part of the card below, not a sibling of it: the plan and
			// the live exit are the same statement about the same two links.
			this.exitCard(data),
			E('div', { 'class': message.cls }, message.text),
			// The readout sits above the controls it describes: the tiles state what
			// the device is doing right now, and the cards below are what the user
			// changes in response.
			this.statusTiles(data),
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

	// The panel is rebuilt on every poll, but the animated icons must not restart
	// with it: replacing the nodes resets every CSS animation, so a steady device
	// would show the same icons stuttering in place every five seconds.  This key
	// covers exactly the fields the panel draws - plus the local editing state, so
	// an unapplied selection still repaints immediately - which means an unchanged
	// device is left alone and a real change still appears at once.
	renderKey: function(data) {
		return [
			data.mode, data.split, data.active4, data.active6,
			data.egress4, data.egress6,
			data.watcher, data.watch_interval, data.health_check,
			data.wan_health, data.modem_health, data.daed_exit_state,
			data.wan_present, data.wan_available, data.wan_pending, data.wan_carrier,
			data.wan_up, data.wan6_up, data.wan4_ready, data.wan6_ready, data.wan_device,
			data.modem_present, data.modem_available, data.modem_pending, data.modem_carrier,
			data.modem_up, data.modem6_up, data.modem4_ready, data.modem6_ready, data.modem_device,
			data.wifi_total, data.wifi_up, data.wifi_ssid, data.wifi_clients,
			this.pendingMode,
			(this.pendingDeviceMap || {}).wan,
			(this.pendingDeviceMap || {}).modem,
			(this.availableDevices || []).join(','),
			this.applying ? '1' : '0',
			this.deviceDirty ? '1' : '0'
		].join('\u0001');
	},

	repaint: function() {
		var old = document.getElementById('h5net-status');
		if (!old || !this.liveData) return;

		var key = this.renderKey(this.liveData);
		if (key === this.renderedKey) return;

		this.renderedKey = key;
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

		// Seed the render key so the first poll does not rebuild (and therefore
		// restart) the icons that were just mounted.
		this.renderedKey = this.renderKey(this.liveData);

		poll.add(L.bind(this.refreshStatus, this), 5);
		return this.statusPanel(this.liveData);
	}
});
