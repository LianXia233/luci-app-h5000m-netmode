'use strict';
'require view';
'require fs';
'require ui';
'require poll';

// Exit policy UI for the H5000M netmode backend.
// Invariants strictly enforced:
// 1. IPv4 and IPv6 leave through the same uplink (family split alert).
// 2. Backup uplink failover preserved by default; explicit confirm for only-mode.
// 3. Graded liveness evaluation (up, pending, idle, down).

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
			/* ================= 主题与色彩体系（支持高光玻璃态与暗黑主题） ================= */
			'.h5net{',
				'--h5-blue:#3b82f6;--h5-blue-soft:rgba(59,130,246,0.12);--h5-blue-glow:rgba(59,130,246,0.3);',
				'--h5-green:#10b981;--h5-green-soft:rgba(16,185,129,0.12);--h5-green-glow:rgba(16,185,129,0.35);',
				'--h5-amber:#f59e0b;--h5-amber-soft:rgba(245,158,11,0.12);--h5-amber-glow:rgba(245,158,11,0.35);',
				'--h5-red:#ef4444;--h5-red-soft:rgba(239,68,68,0.12);--h5-red-glow:rgba(239,68,68,0.35);',
				'--h5-purple:#8b5cf6;--h5-purple-soft:rgba(139,92,246,0.12);',
				'--h5-cyan:#06b6d4;--h5-cyan-soft:rgba(6,182,212,0.12);',
				'--h5-card-bg:rgba(255,255,255,0.92);',
				'--h5-card-border:rgba(226,232,240,0.85);',
				'--h5-card-shadow:0 10px 25px -4px rgba(15,23,42,0.05),0 4px 10px -2px rgba(15,23,42,0.03);',
				'--h5-text-main:#1e293b;--h5-text-sub:#64748b;--h5-text-muted:#94a3b8;',
				'font-family:system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,"PingFang SC",sans-serif;',
			'}',
			'@media(prefers-color-scheme:dark){.h5net{',
				'--h5-card-bg:rgba(30,41,59,0.85);',
				'--h5-card-border:rgba(51,65,85,0.7);',
				'--h5-card-shadow:0 12px 30px -4px rgba(0,0,0,0.35);',
				'--h5-text-main:#f8fafc;--h5-text-sub:#cbd5e1;--h5-text-muted:#64748b;',
			'}}',
			'[data-theme="dark"] .h5net{',
				'--h5-card-bg:rgba(30,41,59,0.85);',
				'--h5-card-border:rgba(51,65,85,0.7);',
				'--h5-text-main:#f8fafc;--h5-text-sub:#cbd5e1;--h5-text-muted:#64748b;',
			'}',

			/* ================= 顶部 Hero 全景出口看板 ================= */
			'.h5net .h5net-hero{',
				'display:grid;grid-template-columns:1fr auto 1fr;align-items:stretch;gap:0;',
				'margin:0 0 16px;padding:18px 22px;border-radius:20px;',
				'background:var(--h5-card-bg);border:1px solid var(--h5-card-border);',
				'box-shadow:var(--h5-card-shadow);backdrop-filter:blur(16px);-webkit-backdrop-filter:blur(16px);',
				'position:relative;overflow:hidden;',
			'}',
			'.h5net .h5net-hero:before{',
				'content:"";position:absolute;top:0;left:0;right:0;height:3px;',
				'background:linear-gradient(90deg,var(--h5-blue),var(--h5-cyan),var(--h5-green));',
				'opacity:0.9;',
			'}',
			'.h5net .hero-slot{display:flex;align-items:center;gap:16px;min-width:0;position:relative;z-index:1}',
			'.h5net .hero-div{width:1px;margin:2px 24px;background:linear-gradient(180deg,transparent,var(--h5-card-border),transparent)}',
			'.h5net .hero-svgbox{',
				'width:56px;height:56px;flex:none;display:grid;place-items:center;border-radius:16px;',
				'background:var(--h5-blue-soft);color:var(--h5-blue);transition:all .3s;',
			'}',
			'.h5net .hero-svgbox.cell{background:var(--h5-cyan-soft);color:var(--h5-cyan)}',
			'.h5net .hero-svgbox.tone-pending{background:var(--h5-amber-soft);color:var(--h5-amber)}',
			'.h5net .hero-svgbox.tone-down{background:var(--h5-red-soft);color:var(--h5-red)}',
			'.h5net .hero-svgbox.tone-off{background:rgba(148,163,184,0.12);color:#94a3b8}',
			'.h5net .hero-svgbox svg{width:46px;height:46px;overflow:visible}',
			'.h5net .hero-info{flex:1;min-width:0}',
			'.h5net .hero-topline{display:flex;align-items:center;gap:10px;margin-bottom:4px}',
			'.h5net .hero-title{font-size:16px;font-weight:700;color:var(--h5-text-main);margin:0;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}',
			'.h5net .hero-badge{',
				'display:inline-flex;align-items:center;gap:6px;padding:3px 8px;border-radius:20px;',
				'font-size:11px;font-weight:700;letter-spacing:0.3px;',
			'}',
			'.h5net .hero-badge.is-up{background:var(--h5-green-soft);color:var(--h5-green);box-shadow:0 0 10px var(--h5-green-soft)}',
			'.h5net .hero-badge.is-pending{background:var(--h5-amber-soft);color:var(--h5-amber)}',
			'.h5net .hero-badge.is-down{background:var(--h5-red-soft);color:var(--h5-red);box-shadow:0 0 10px var(--h5-red-soft)}',
			'.h5net .hero-badge.is-off{background:rgba(148,163,184,0.12);color:#94a3b8}',
			'.h5net .hero-pulse-dot{width:6px;height:6px;border-radius:50%;background:currentColor;animation:h5-beacon 1.8s infinite}',
			'.h5net .hero-meta{display:flex;align-items:center;flex-wrap:wrap;gap:6px;font-size:12px;color:var(--h5-text-sub);line-height:1.4}',
			'.h5net .hero-meta .pill{padding:2px 7px;border-radius:6px;background:rgba(148,163,184,0.12);font-weight:600}',
			'.h5net .hero-meta .iface{font-family:ui-monospace,SFMono-Regular,Consolas,monospace;font-weight:600;color:var(--h5-text-main)}',
			'.h5net .hero-hint{margin-top:6px;color:var(--h5-text-muted);font-size:11.5px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}',

			/* ================= 通知 / 告警条 ================= */
			'.h5net-note{margin:0 0 16px;padding:12px 16px;border-radius:12px;font-size:13px;line-height:1.5;display:flex;align-items:center;gap:10px;border:1px solid transparent}',
			'.h5net-note{background:var(--h5-blue-soft);color:var(--h5-blue);border-color:rgba(59,130,246,0.2)}',
			'.h5net-note.alert{background:var(--h5-red-soft);color:var(--h5-red);border-color:rgba(239,68,68,0.25);font-weight:600}',
			'.h5net-note.warn{background:var(--h5-amber-soft);color:var(--h5-amber);border-color:rgba(245,158,11,0.25)}',

			/* ================= 8大核心指标卡片矩阵 ================= */
			'.h5net-stat{margin:0 0 18px}',
			'.h5net-stat-grid{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:12px}',
			'.h5net-stat-item{',
				'background:var(--h5-card-bg);border:1px solid var(--h5-card-border);border-radius:16px;',
				'padding:14px;box-shadow:var(--h5-card-shadow);transition:all .22s cubic-bezier(0.4,0,0.2,1);',
				'position:relative;overflow:hidden;backdrop-filter:blur(10px);',
			'}',
			'.h5net-stat-item:hover{transform:translateY(-2px);border-color:var(--h5-blue)}',
			'.h5net-stat-icon{',
				'height:62px;display:grid;place-items:center;border-radius:12px;margin-bottom:10px;',
				'background:rgba(241,245,249,0.7);transition:all .3s;',
			'}',
			'@media(prefers-color-scheme:dark){.h5net-stat-icon{background:rgba(15,23,42,0.4)}}',
			'.h5net-stat-icon svg{width:46px;height:46px;overflow:visible}',
			'.h5net-stat-icon.tone-green{color:var(--h5-green);background:var(--h5-green-soft)}',
			'.h5net-stat-icon.tone-blue{color:var(--h5-blue);background:var(--h5-blue-soft)}',
			'.h5net-stat-icon.tone-mint{color:#059669;background:rgba(5,150,105,0.12)}',
			'.h5net-stat-icon.tone-orange{color:var(--h5-amber);background:var(--h5-amber-soft)}',
			'.h5net-stat-icon.tone-purple{color:var(--h5-purple);background:var(--h5-purple-soft)}',
			'.h5net-stat-icon.tone-cyan{color:var(--h5-cyan);background:var(--h5-cyan-soft)}',
			'.h5net-stat-icon.tone-red{color:var(--h5-red);background:var(--h5-red-soft)}',
			'.h5net-stat-icon.tone-gray{color:#64748b;background:rgba(100,116,139,0.12)}',
			'.h5net-stat-item b{display:block;font-size:13.5px;font-weight:700;color:var(--h5-text-main);margin-bottom:4px}',
			'.h5net-stat-value{display:block;font-size:12px;font-weight:600;color:var(--h5-text-sub);white-space:nowrap;overflow:hidden;text-overflow:ellipsis}',
			'.h5net-stat-hint{display:block;margin-top:3px;color:var(--h5-text-muted);font-size:11px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}',

			/* ================= 链路选择双卡片 ================= */
			'.h5net-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:14px}',
			'.h5net-card{',
				'position:relative;display:flex;flex-direction:column;padding:18px;',
				'background:var(--h5-card-bg);border:1.5px solid var(--h5-card-border);border-radius:18px;',
				'box-shadow:var(--h5-card-shadow);cursor:pointer;user-select:none;',
				'transition:all .22s cubic-bezier(0.4,0,0.2,1);outline:none;',
			'}',
			'.h5net-card:hover{transform:translateY(-2px);box-shadow:0 14px 28px -5px rgba(0,0,0,0.08)}',
			'.h5net-card.selected{border-color:var(--h5-blue);box-shadow:0 0 0 2px var(--h5-blue-glow)}',
			'.h5net-card.active{border-color:var(--h5-green);box-shadow:0 0 0 2px var(--h5-green-glow)}',
			'.h5net-card.unselected{opacity:0.75}',
			'.h5net-cardtop{display:flex;align-items:center;justify-content:space-between;gap:12px}',
			'.h5net-name{display:flex;align-items:center;gap:14px}',
			'.h5net-icon{',
				'width:46px;height:46px;display:grid;place-items:center;border-radius:14px;',
				'background:var(--h5-blue-soft);color:var(--h5-blue);transition:all .3s;',
			'}',
			'.h5net-card.modem .h5net-icon{background:var(--h5-cyan-soft);color:var(--h5-cyan)}',
			'.h5net-icon.tone-pending{background:var(--h5-amber-soft);color:var(--h5-amber)}',
			'.h5net-icon.tone-down{background:var(--h5-red-soft);color:var(--h5-red)}',
			'.h5net-icon.tone-off{background:rgba(148,163,184,0.12);color:#94a3b8}',
			'.h5net-name h3{margin:0 0 3px;font-size:16px;font-weight:700;color:var(--h5-text-main)}',
			'.h5net-role{color:var(--h5-text-muted);font-size:12px;font-weight:500}',
			'.h5net-card.selected .h5net-role{color:var(--h5-blue);font-weight:700}',
			'.h5net-card.active .h5net-role{color:var(--h5-green);font-weight:700}',
			'.h5net-state{display:inline-flex;align-items:center;gap:6px;font-size:12px;font-weight:700;color:var(--h5-red);white-space:nowrap}',
			'.h5net-state:before{content:"";width:8px;height:8px;border-radius:50%;background:currentColor}',
			'.h5net-state.up{color:var(--h5-green);text-shadow:0 0 8px var(--h5-green-soft)}',
			'.h5net-state.up:before{box-shadow:0 0 8px currentColor;animation:h5-beacon 1.8s infinite}',
			'.h5net-state.pending{color:var(--h5-amber)}',
			'.h5net-state.idle{color:var(--h5-text-muted)}',
			'.h5net-protos{display:flex;flex-wrap:wrap;gap:8px;margin-top:16px}',
			'.h5net-proto{',
				'padding:5px 10px;border-radius:8px;background:rgba(148,163,184,0.1);',
				'font-size:11.5px;font-weight:600;color:var(--h5-text-sub);',
			'}',
			'.h5net-proto.current{background:var(--h5-green-soft);color:var(--h5-green)}',
			'.h5net-proto.dead{background:var(--h5-red-soft);color:var(--h5-red)}',
			'.h5net-device-row{',
				'display:flex;align-items:center;gap:10px;margin-top:14px;padding-top:12px;',
				'border-top:1px dashed var(--h5-card-border);',
			'}',
			'.h5net-device-row label{font-size:12px;font-weight:600;color:var(--h5-text-sub);white-space:nowrap}',
			'.h5net-device-row select{',
				'flex:1;padding:7px 10px;border:1px solid var(--h5-card-border);border-radius:8px;',
				'background:var(--h5-card-bg);font-size:12.5px;font-weight:600;color:var(--h5-text-main);',
				'cursor:pointer;outline:none;transition:border-color .15s;',
			'}',
			'.h5net-device-row select:focus{border-color:var(--h5-blue);box-shadow:0 0 0 2px var(--h5-blue-glow)}',
			'.h5net-cardfoot{',
				'margin-top:12px;padding-top:10px;border-top:1px dashed var(--h5-card-border);',
				'display:flex;justify-content:flex-end;',
			'}',
			'.h5net-link{',
				'border:0;background:rgba(59,130,246,0.08);padding:5px 12px;color:var(--h5-blue);',
				'font-size:12px;font-weight:600;cursor:pointer;border-radius:6px;transition:all .18s;',
			'}',
			'.h5net-link:hover{background:var(--h5-blue);color:#fff}',
			'.h5net-link[disabled]{background:transparent;color:var(--h5-text-muted);cursor:default}',

			/* ================= 底部状态栏与控制按钮 ================= */
			'.h5net-foot{',
				'display:flex;justify-content:space-between;align-items:center;gap:16px;',
				'margin-top:16px;padding:16px 20px;border-radius:16px;',
				'background:var(--h5-card-bg);border:1px solid var(--h5-card-border);box-shadow:var(--h5-card-shadow);',
			'}',
			'.h5net-meta{display:flex;flex-wrap:wrap;gap:14px;color:var(--h5-text-sub);font-size:12px}',
			'.h5net-meta b{font-weight:700;color:var(--h5-text-main)}',
			'.h5net-meta .off{color:var(--h5-amber)}',
			'.h5net-buttons{display:flex;gap:10px;flex-shrink:0}',
			'.h5net-buttons .cbi-button{min-width:110px;padding:8px 16px;border-radius:8px;font-weight:600}',

			/* ================= 极具科技感的动态 SVG 关键帧动效 ================= */
			'@keyframes h5-beacon{0%,100%{opacity:1;transform:scale(1)}50%{opacity:0.4;transform:scale(1.2)}}',
			'@keyframes h5-dash{to{stroke-dashoffset:-24}}',
			'@keyframes h5-dash-rev{to{stroke-dashoffset:24}}',
			'@keyframes h5-pulse{0%,100%{opacity:0.35}50%{opacity:1}}',
			'@keyframes h5-ring{to{transform:rotate(360deg)}}',
			'@keyframes h5-ring-rev{to{transform:rotate(-360deg)}}',
			'@keyframes h5-expand{0%{r:4;opacity:0.8}100%{r:18;opacity:0}}',
			'@keyframes h5-wave{0%,100%{opacity:0.25;transform:scale(0.92)}50%{opacity:1;transform:scale(1.05)}}',
			'@keyframes h5-bounce-eq{0%,100%{height:4px}50%{height:14px}}',
			'@keyframes h5-flow-dot{0%{transform:translateX(0);opacity:0}20%{opacity:1}80%{opacity:1}100%{transform:translateX(24px);opacity:0}}',

			'.h5net .svg-dash{animation:h5-dash 1.2s linear infinite}',
			'.h5net .svg-dash-fast{animation:h5-dash 0.8s linear infinite}',
			'.h5net .svg-pulse{animation:h5-pulse 1.6s ease-in-out infinite}',
			'.h5net .svg-spin{transform-origin:center;animation:h5-ring 4s linear infinite}',
			'.h5net .svg-spin-rev{transform-origin:center;animation:h5-ring-rev 3s linear infinite}',
			'.h5net .svg-wave{transform-origin:center;animation:h5-wave 1.6s ease-in-out infinite}',
			'.h5net .svg-wave2{animation-delay:.35s}',
			'.h5net .svg-wave3{animation-delay:.7s}',
			'.h5net .svg-flow-dot{animation:h5-flow-dot 1.5s ease-in-out infinite}',

			/* 状态停止动效：故障或离线时让动效平稳归于静止 */
			'.h5net .is-idle svg *,.h5net .tone-down svg *,.h5net .tone-off svg *{animation:none!important}',

			/* ================= 切换进度 (后台任务) ================= */
			'.h5net-switch{',
				'margin-top:12px;padding:14px 18px;border-radius:14px;',
				'background:var(--h5-card-bg);border:1px solid var(--h5-card-border);',
				'box-shadow:var(--h5-card-shadow);',
				'}',
			'.h5net-switch.busy{border-color:var(--h5-blue)}',
			'.h5net-switch.failed{border-color:var(--h5-red)}',
			'.h5net-switch-head{display:flex;align-items:center;gap:9px;font-size:13.5px;font-weight:700;color:var(--h5-text-main)}',
			'.h5net-switch-head .spin{width:13px;height:13px;border-radius:50%;border:2px solid var(--h5-blue);border-top-color:transparent;animation:h5-ring .8s linear infinite}',
			'.h5net-switch-head .dot-ok{width:9px;height:9px;border-radius:50%;background:var(--h5-green)}',
			'.h5net-switch-head .dot-fail{width:9px;height:9px;border-radius:50%;background:var(--h5-red)}',
			'.h5net-switch-meta{margin-top:6px;color:var(--h5-text-sub);font-size:12px;line-height:1.7;word-break:break-all}',
			'.h5net-switch-meta code{font-family:ui-monospace,Menlo,Consolas,monospace;font-size:11.5px}',
			'.h5net-switch-steps{display:flex;flex-wrap:wrap;gap:6px;margin-top:10px}',
			'.h5net-switch-step{',
				'padding:3px 9px;border-radius:999px;font-size:11.5px;font-weight:600;',
				'background:rgba(148,163,184,0.14);color:var(--h5-text-muted);',
				'}',
			'.h5net-switch-step.done{background:var(--h5-green-soft);color:#047857}',
			'.h5net-switch-step.active{background:var(--h5-blue);color:#fff;animation:h5-pulse 1.4s ease-in-out infinite}',
			'.h5net-switch-step.failed{background:var(--h5-red-soft);color:var(--h5-red)}',
			'.h5net-switch-actions{margin-top:10px;display:flex;justify-content:flex-end}',

			/* ================= 响应式排版 ================= */
			'@media(max-width:960px){',
				'.h5net-stat-grid{grid-template-columns:repeat(2,minmax(0,1fr))}',
				'.h5net .h5net-hero{grid-template-columns:1fr;gap:16px}',
				'.h5net .hero-div{width:100%;height:1px;margin:8px 0;background:linear-gradient(90deg,transparent,var(--h5-card-border),transparent)}',
			'}',
			'@media(max-width:640px){',
				'.h5net-grid{grid-template-columns:1fr}',
				'.h5net-foot{flex-direction:column;align-items:stretch}',
				'.h5net-buttons{flex-direction:column}',
				'.h5net-buttons .cbi-button{width:100%}',
				'.h5net .hero-hint{display:none}',
			'}'
		].join(''));
	},

	// 卡片主图标：高精细动态 SVG
	iconSvg: function(kind) {
		if (kind === 'modem') {
			// 5G 模组：基站天线辐射波纹 + 阶梯跳动信号等离子柱
			return '<svg viewBox="0 0 48 48" aria-hidden="true">'
				+ '<path d="M12 28a14 14 0 0 1 24 0" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" class="svg-wave svg-wave3" opacity=".3"/>'
				+ '<path d="M16 28a9 9 0 0 1 16 0" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" class="svg-wave svg-wave2" opacity=".6"/>'
				+ '<path d="M20 28a5 5 0 0 1 8 0" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" class="svg-wave"/>'
				+ '<circle cx="24" cy="28" r="2.8" fill="currentColor"/>'
				+ '<path d="M24 28v14M18 42h12" stroke="currentColor" stroke-width="2.6" stroke-linecap="round"/>'
				+ '<circle cx="36" cy="12" r="2.2" fill="currentColor" class="svg-pulse"/>'
				+ '<path d="M36 17v7M32 20v4M40 14v10" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"/>'
				+ '</svg>';
		}

		// 有线 WAN：RJ45 千兆网口 + 动态传输光脉冲 + 双指示灯
		return '<svg viewBox="0 0 48 48" aria-hidden="true">'
			+ '<rect x="8" y="10" width="32" height="24" rx="6" fill="none" stroke="currentColor" stroke-width="2.4"/>'
			+ '<path d="M17 10v4M31 10v4M16 34v6M32 34v6M12 40h24" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"/>'
			+ '<path class="svg-dash" d="M14 22h20" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-dasharray="4 4"/>'
			+ '<circle cx="15" cy="16" r="2" fill="currentColor" class="svg-pulse"/>'
			+ '<circle cx="33" cy="16" r="2" fill="currentColor" opacity=".5"/>'
			+ '</svg>';
	},

	// 实时活动出口卡片图标
	exitIconSvg: function(kind) {
		if (kind === 'modem') {
			// 5G 蜂窝塔动态发射矩阵
			return '<svg viewBox="0 0 64 64" aria-label="' + _('蜂窝网络') + '">'
				+ '<circle cx="32" cy="22" r="3.5" fill="currentColor"/>'
				+ '<path class="svg-wave svg-wave3" d="M12 22a20 20 0 0 1 40 0" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" opacity=".4"/>'
				+ '<path class="svg-wave svg-wave2" d="M18 22a14 14 0 0 1 28 0" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" opacity=".7"/>'
				+ '<path class="svg-wave" d="M24 22a8 8 0 0 1 16 0" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round"/>'
				+ '<path d="M32 24v30M22 54h20M25 36l7-12 7 12M22 46h20" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"/>'
				+ '<circle cx="32" cy="11" r="2.2" fill="currentColor" class="svg-pulse"/>'
				+ '</svg>';
		}
		if (kind === 'route') {
			// 多维智能路由拓扑网络
			return '<svg viewBox="0 0 64 64" aria-label="' + _('其他路由') + '">'
				+ '<path class="svg-dash" d="M14 46C24 46 20 18 32 18s8 28 18 28" fill="none" stroke="currentColor" stroke-width="2.8" stroke-linecap="round" stroke-dasharray="6 5"/>'
				+ '<circle cx="14" cy="46" r="5" fill="currentColor"/>'
				+ '<circle cx="50" cy="46" r="5" fill="currentColor"/>'
				+ '<circle cx="32" cy="18" r="6" fill="currentColor" class="svg-pulse"/>'
				+ '<path d="M26 18h12M14 40v6M50 40v6" stroke="currentColor" stroke-width="2" stroke-linecap="round" opacity=".4"/>'
				+ '</svg>';
	}
		// 有线千兆光猫/WAN端口高阶微光
		return '<svg viewBox="0 0 64 64" aria-label="' + _('有线 WAN') + '">'
			+ '<rect x="14" y="16" width="36" height="28" rx="8" fill="none" stroke="currentColor" stroke-width="2.4"/>'
			+ '<rect x="22" y="22" width="20" height="16" rx="4" fill="currentColor" opacity=".12"/>'
			+ '<path d="M26 38v10M38 38v10M20 48h24" stroke="currentColor" stroke-width="2.4" stroke-linecap="round"/>'
			+ '<path class="svg-dash-fast" d="M20 30h24" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-dasharray="5 4"/>'
			+ '<circle cx="21" cy="21" r="2.5" fill="currentColor" class="svg-pulse"/>'
			+ '<circle cx="43" cy="21" r="2.5" fill="currentColor"/>'
			+ '</svg>';
	},

	// 顶部网关网络出口拓扑全景 SVG
	egressIconSvg: function() {
		return '<svg viewBox="0 0 64 64" aria-label="' + _('网络出口') + '">'
			+ '<rect x="6" y="24" width="16" height="16" rx="5" fill="none" stroke="currentColor" stroke-width="2.4"/>'
			+ '<circle cx="14" cy="32" r="3" fill="currentColor" class="svg-pulse"/>'
			+ '<path class="svg-dash" d="M22 28c12 0 10-14 24-14" fill="none" stroke="currentColor" stroke-width="2.2" stroke-dasharray="4 4"/>'
			+ '<path class="svg-dash" d="M22 36c12 0 10 14 24 14" fill="none" stroke="currentColor" stroke-width="2.2" stroke-dasharray="4 4"/>'
			+ '<rect x="46" y="8" width="12" height="12" rx="3.5" fill="none" stroke="currentColor" stroke-width="2"/>'
			+ '<circle cx="52" cy="14" r="2" fill="currentColor"/>'
			+ '<path d="M47 48h10M50 44h4M52 40v8" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"/>'
			+ '</svg>';
	},

	exitVerdict: function(data) {
		var active4 = data.active4 || 'none';
		var active6 = data.active6 || 'none';
		var split = data.split === '1';
		var active = active4 !== 'none' ? active4 : active6;
		var daed = data.daed_exit_state || 'none';
		var self = this;

		var tone = 'tone-green', idle = false, kind, title, live, liveCls, badge, iface, role;

		function roleOf(k) {
			var order = self.modeOrder(data.mode);
			if (order.indexOf(k) < 0) return _('未纳入策略');
			if (order[0] === k) return _('首选出口');
			return (k === active) ? _('备用出口 · 已接管') : _('备用出口');
		}

		if (split) {
			kind = active4 === 'modem' ? 'modem' : 'wan';
			tone = 'tone-down';
			idle = false;
			title = _('出口分流告警');
			live = _('分流故障');
			liveCls = 'is-down';
			badge = _('IPv4 ') + this.exitLabel(active4);
			iface = this.exitDevice(data, active4);
			role = _('IPv6 ') + this.exitLabel(active6);
		}
		else if (active === 'wan' || active === 'modem') {
			var state = this.connectionState(data, active);
			kind = active;
			tone = (state.cls === 'up') ? (active === 'modem' ? 'cell' : 'tone-green')
				: (state.cls === 'pending') ? 'tone-pending'
				: (state.cls === 'idle') ? 'tone-off' : 'tone-down';
			idle = tone === 'tone-down' || tone === 'tone-off';
			title = _('当前出口：%s').format(this.exitLabel(active));
			live = (state.cls === 'up') ? _('在线中') : state.label;
			liveCls = (state.cls === 'up') ? 'is-up'
				: (state.cls === 'pending') ? 'is-pending'
				: (state.cls === 'idle') ? 'is-off' : 'is-down';
			badge = (active === 'wan') ? _('有线宽带') : _('5G / LTE');
			iface = (active === 'wan' ? data.wan_device : data.modem_device) || _('未指定');
			role = roleOf(active);
		}
		else if (active === 'other') {
			kind = 'route';
			tone = 'tone-pending';
			liveCls = 'is-pending';
			title = _('当前出口：第三方路由');
			live = _('外部接管');
			badge = (daed !== 'none') ? (_('daed 接管') ) : _('外部路由');
			iface = data.egress4 || this.exitDevice(data, active4);
			role = _('未在本策略管辖内');
		}
		else {
			kind = 'route';
			tone = 'tone-off';
			idle = true;
			liveCls = 'is-off';
			title = _('无可用网络出口');
			live = _('离线');
			badge = _('无出口');
			iface = '—';
			role = _('策略：') + this.modeLabel(data.mode);
		}

		return { tone: tone, idle: idle, kind: kind, title: title, live: live,
			liveCls: liveCls, badge: badge, iface: iface, role: role };
	},

	exitCard: function(data) {
		var v = this.exitVerdict(data);
		var card = E('article', {
			'class': 'h5net-hero' + (v.idle ? ' is-idle' : '')
		});

		card.appendChild(this.egressSlot(data));
		card.appendChild(E('span', { 'class': 'hero-div' }));
		card.appendChild(this.exitSlot(v));

		return card;
	},

	egressSlot: function(data) {
		var self = this;
		var order = this.modeOrder(data.mode);
		var active = (data.active4 && data.active4 !== 'none') ? data.active4
			: ((data.active6 && data.active6 !== 'none') ? data.active6 : 'none');

		var live, liveCls;
		if (data.split === '1') {
			live = _('分流异常'); liveCls = 'is-down';
		}
		else if (active === 'none') {
			live = _('无默认网关'); liveCls = 'is-off';
		}
		else if (active === 'other') {
			live = _('策略旁路'); liveCls = 'is-pending';
		}
		else if (order.length === 1) {
			live = _('单出口运行'); liveCls = 'is-pending';
		}
		else if (order.filter(function(k) {
			return self.connectionState(data, k).cls === 'up';
		}).length < order.length) {
			live = _('备用链路未就绪'); liveCls = 'is-pending';
		}
		else {
			live = _('双路由就绪'); liveCls = 'is-up';
		}

		var plan = order.map(function(k, i) {
			return (order.length > 1 ? (i + 1) + '. ' : '') + self.exitLabel(k);
		}).join(' ➔ ');

		var hint = _('点击下方卡片可快速切换首选出口；“仅用此出口”将移出备选路由。');

		var slot = E('div', { 'class': 'hero-slot hero-egress' });
		var box = E('div', { 'class': 'hero-svgbox' });
		box.innerHTML = this.egressIconSvg();

		slot.appendChild(box);
		slot.appendChild(E('div', { 'class': 'hero-info' }, [
			E('div', { 'class': 'hero-topline' }, [
				E('h2', { 'class': 'hero-title' }, _('多出口调度策略')),
				E('div', { 'class': 'hero-badge ' + liveCls }, [
					E('i', { 'class': 'hero-pulse-dot' }),
					E('span', {}, live)
				])
			]),
			E('div', { 'class': 'hero-meta' }, [
				E('span', { 'class': 'pill' }, this.modeLabel(data.mode)),
				E('span', {}, '•'),
				E('span', {}, plan)
			]),
			E('div', { 'class': 'hero-hint', 'title': hint }, hint)
		]));

		return slot;
	},

	exitSlot: function(v) {
		var slot = E('div', { 'class': 'hero-slot hero-exit' });
		var box = E('div', { 'class': 'hero-svgbox ' + v.tone });
		box.innerHTML = this.exitIconSvg(v.kind);

		slot.appendChild(box);
		slot.appendChild(E('div', { 'class': 'hero-info' }, [
			E('div', { 'class': 'hero-topline' }, [
				E('div', { 'class': 'hero-title' }, v.title),
				E('div', { 'class': 'hero-badge ' + v.liveCls }, [
					E('i', { 'class': 'hero-pulse-dot' }),
					E('span', {}, v.live)
				])
			]),
			E('div', { 'class': 'hero-meta' }, [
				E('span', { 'class': 'pill' }, v.badge),
				E('span', {}, '•'),
				E('span', { 'class': 'iface' }, v.iface),
				E('span', {}, '•'),
				E('span', {}, v.role)
			])
		]));

		return slot;
	},

	exitDevice: function(data, exit) {
		if (exit === 'wan') return data.wan_device || '—';
		if (exit === 'modem') return data.modem_device || '—';
		return data.egress4 || '—';
	},

	iconTone: function(state) {
		if (state.cls === 'up') return 'tone-live';
		if (state.cls === 'pending') return 'tone-pending';
		if (state.cls === 'idle') return 'tone-off';
		return 'tone-down';
	},

	// 8个精美状态仪表微组件
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
				name: _('有线 WAN 链路'),
				value: (data.wan_device || _('未指定')) + ' · ' + wan.label,
				hint: 'v4: ' + ready(data.wan4_ready) + ' | v6: ' + ready(data.wan6_ready),
				live: wan.cls === 'up',
				svg: '<svg viewBox="0 0 64 64">'
					+ '<rect x="14" y="12" width="36" height="26" rx="6" fill="none" stroke="currentColor" stroke-width="2.6"/>'
					+ '<path d="M22 46h20M26 38v8m12-8v8" stroke="currentColor" stroke-width="2.6" stroke-linecap="round"/>'
					+ '<path class="svg-dash" d="M20 22h24M20 28h15" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-dasharray="5 4"/>'
					+ '<circle class="svg-pulse" cx="42" cy="28" r="2.5" fill="currentColor"/>'
					+ '</svg>'
			},
			{
				tone: 'blue',
				name: 'Wi‑Fi 无线覆盖',
				value: wifiTotal ? _('%s/%s 频段在线').format(wifiUp, wifiTotal) : _('未检测到射频'),
				hint: data.wifi_ssid ? data.wifi_ssid + ' · ' + _('%s 终端').format(wifiClients) : _('无线未开启'),
				live: wifiUp > 0,
				svg: '<svg viewBox="0 0 64 64">'
					+ '<path class="svg-wave svg-wave3" d="M14 36a22 22 0 0 1 36 0" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" opacity=".25"/>'
					+ '<path class="svg-wave svg-wave2" d="M20 40a14 14 0 0 1 24 0" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" opacity=".6"/>'
					+ '<path class="svg-wave" d="M26 44a6 6 0 0 1 12 0" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round"/>'
					+ '<circle cx="32" cy="48" r="3" fill="currentColor" class="svg-pulse"/>'
					+ '</svg>'
			},
			{
				tone: 'mint',
				name: _('5G 移动模组'),
				value: (data.modem_device || _('未指定')) + ' · ' + modem.label,
				hint: 'v4: ' + ready(data.modem4_ready) + ' | v6: ' + ready(data.modem6_ready),
				live: modem.cls === 'up',
				svg: '<svg viewBox="0 0 64 64">'
					+ '<rect x="14" y="14" width="36" height="36" rx="10" fill="none" stroke="currentColor" stroke-width="2.6"/>'
					+ '<text x="32" y="38" text-anchor="middle" font-size="16" font-weight="900" fill="currentColor" letter-spacing="1">5G</text>'
					+ '<circle class="svg-spin" cx="32" cy="32" r="23" fill="none" stroke="currentColor" stroke-width="2" stroke-dasharray="6 8" opacity=".5"/>'
					+ '</svg>'
			},
			{
				tone: 'orange',
				name: _('全双栈流量转发'),
				value: split ? (e4 + ' ≠ ' + e6) : (e4 + ' ⇄ ' + e6),
				hint: split ? _('⚠️ IPv4/v6 出口不一致') : _('双栈出口已严格对齐'),
				live: !split && (e4 !== 'none' || e6 !== 'none'),
				svg: '<svg viewBox="0 0 64 64">'
					+ '<path d="M14 26h36M42 18l8 8-8 8" fill="none" stroke="currentColor" stroke-width="2.8" stroke-linecap="round" stroke-linejoin="round"/>'
					+ '<path d="M50 38H14M22 30l-8 8 8 8" fill="none" stroke="currentColor" stroke-width="2.8" stroke-linecap="round" stroke-linejoin="round" opacity=".6"/>'
					+ '<circle cx="32" cy="26" r="3.2" fill="currentColor" class="svg-flow-dot"/>'
					+ '</svg>'
			},
			{
				tone: 'purple',
				name: _('自动链路看门狗'),
				value: watcherOn ? _('守护中 · %ss 巡检').format(data.watch_interval || '10') : _('已挂起'),
				hint: _('主选：') + this.modeLabel(data.mode),
				live: watcherOn,
				svg: '<svg viewBox="0 0 64 64">'
					+ '<circle cx="32" cy="32" r="20" fill="none" stroke="currentColor" stroke-width="2.2" opacity=".25"/>'
					+ '<circle class="svg-spin" cx="32" cy="32" r="16" fill="none" stroke="currentColor" stroke-width="2.8" stroke-linecap="round" stroke-dasharray="40 30"/>'
					+ '<circle class="svg-spin-rev" cx="32" cy="32" r="11" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-dasharray="20 20"/>'
					+ '<circle cx="32" cy="32" r="3" fill="currentColor" class="svg-pulse"/>'
					+ '</svg>'
			},
			{
				tone: 'cyan',
				name: _('IPv6 承载状态'),
				value: active6 === 'none' ? _('未激活') : (_('出接口: ') + e6),
				hint: _('策略: 强绑定 IPv4 物理出口'),
				live: active6 !== 'none',
				svg: '<svg viewBox="0 0 64 64">'
					+ '<path d="M20 44h24a10 10 0 0 0 1-20 14 14 0 0 0-26-3 9 9 0 0 0 1 23Z" fill="none" stroke="currentColor" stroke-width="2.6"/>'
					+ '<path class="svg-dash" d="M32 38V22" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-dasharray="3 3"/>'
					+ '<path d="M26 28l6-6 6 6" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"/>'
					+ '</svg>'
			},
			{
				tone: 'red',
				name: _('链路主动健康探测'),
				value: healthOn ? ('WAN ' + probe(data.wan_health) + ' · 模组 ' + probe(data.modem_health)) : _('未启用'),
				hint: healthOn ? _('Anycast 端到端活体心跳') : _('开启后即时探测防假死'),
				live: healthOn,
				svg: '<svg viewBox="0 0 64 64">'
					+ '<circle cx="32" cy="32" r="22" fill="none" stroke="currentColor" stroke-width="2" opacity=".2"/>'
					+ '<circle class="svg-spin" cx="32" cy="32" r="14" fill="none" stroke="currentColor" stroke-width="2.6" stroke-dasharray="25 35"/>'
					+ '<path d="M18 32h7l3-6 6 12 4-8 3 2h5" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"/>'
					+ '</svg>'
			},
			{
				tone: 'gray',
				name: _('智能核心路由'),
				value: active4 === 'none' ? _('无默认路由') : (active4 === 'other' ? _('外部接管') : _('自主纳管')),
				hint: (daed !== 'none') ? (_('daed 出口: ') + daed) : _('FIB 动态路由度量选路'),
				live: active4 !== 'none' && active4 !== 'other',
				svg: '<svg viewBox="0 0 64 64">'
					+ '<path class="svg-dash" d="M16 46C24 46 22 20 32 20s8 26 16 26" fill="none" stroke="currentColor" stroke-width="2.6" stroke-dasharray="5 4"/>'
					+ '<circle cx="16" cy="46" r="4.5" fill="currentColor"/>'
					+ '<circle cx="48" cy="46" r="4.5" fill="currentColor"/>'
					+ '<circle cx="32" cy="20" r="5" fill="currentColor" class="svg-pulse"/>'
					+ '</svg>'
			}
		];

		return E('section', { 'class': 'h5net-stat' }, [
			E('div', { 'class': 'h5net-stat-grid' }, items.map(function(item) {
				var icon = E('div', {
					'class': 'h5net-stat-icon tone-' + item.tone + (item.live ? '' : ' is-idle')
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
		if (mode === 'modem_first') return _('5G 模组优先');
		if (mode === 'wan_only') return _('仅用有线 WAN');
		if (mode === 'modem_only') return _('仅用 5G 模组');
		return _('有线 WAN 优先');
	},

	splitSentence: function(data) {
		return _('⚠️ 出口分流异常：当前 IPv4 走 %s，IPv6 走 %s。流量分离会导致部分应用鉴权或连接中断，建议立即点击“对齐出口”。')
			.format(this.exitLabel(data.active4), this.exitLabel(data.active6));
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
		if (position < 0) return _('未启用');
		if (order.length === 1) return _('独占出口 (无故障容灾)');
		return position === 0 ? '★ ' + _('首选主路由') : '✦ ' + _('热备用路由');
	},

	connectionState: function(data, kind) {
		var present = data[kind + '_present'];
		var available = data[kind + '_available'];
		var pending = data[kind + '_pending'];
		var carrier = data[kind + '_carrier'];
		var up4 = data[kind + '_up'] === '1';
		var up6 = data[kind + '6_up'] === '1';

		if (present !== '1') return { label: _('硬件未配置'), cls: 'idle' };
		if (up4 || up6) return { label: _('已接通运行'), cls: 'up' };
		if (available === '1' || pending === '1') return { label: _('网络握手中'), cls: 'pending' };
		if (carrier === '0' && kind === 'wan') return { label: _('物理网线断开'), cls: '' };
		return { label: _('链路离线'), cls: '' };
	},

	selectRoute: function(kind, ev) {
		if (ev) ev.preventDefault();
		if (this.applying || this.switchRunning()) return;

		var next = kind === 'modem' ? 'modem_first' : 'wan_first';
		if (this.pendingMode === next && !this.selecting) return;

		this.selecting = true;
		this.pendingMode = next;
		this.repaint();
	},

	selectOnly: function(kind, ev) {
		if (ev) { ev.preventDefault(); ev.stopPropagation(); }
		if (this.applying || this.switchRunning()) return;

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
		if (!dev || this.applying || this.switchRunning()) return;

		var otherRole = role === 'wan' ? 'modem' : 'wan';
		if (this.pendingDeviceMap[otherRole] === dev)
			this.pendingDeviceMap[otherRole] = this.pendingDeviceMap[role];

		this.pendingDeviceMap[role] = dev;
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
			E('label', {}, _('绑定物理接口')),
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
		var up4 = data[kind + '_up'] === '1';
		var up6 = data[kind + '6_up'] === '1';
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
		var cls = 'h5net-card ' + (modem ? 'modem' : 'wan') + (selected ? ' selected' : ' unselected') + ((active4 || active6) ? ' active' : '')
			+ (this.switchBusy(this.liveData) ? ' switching' : '');

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
						E('h3', {}, modem ? _('5G 移动蜂窝模组') : _('有线以太网 WAN')),
						E('div', { 'class': 'h5net-role' }, this.roleLabel(this.pendingMode, kind))
					])
				]),
				E('div', { 'class': 'h5net-state ' + state.cls }, state.label)
			]),
			E('div', { 'class': 'h5net-protos' }, [
				E('span', {
					'class': 'h5net-proto' + (active4 ? ' current' : (ready4 === '1' ? '' : ' dead'))
				}, active4 ? _('● IPv4 正在通信') : (ready4 === '1' ? _('○ IPv4 就绪') : _('✕ IPv4 不可用'))),
				E('span', {
					'class': 'h5net-proto' + (active6 ? ' current' : (ready6 === '1' ? '' : ' dead'))
				}, active6 ? _('● IPv6 正在通信') : (ready6 === '1' ? _('○ IPv6 就绪') : _('✕ IPv6 不可用')))
			]),
			this.deviceDropdown(kind),
			E('div', { 'class': 'h5net-cardfoot' }, [
				E('button', {
					'class': 'h5net-link',
					'type': 'button',
					'title': _('将该链路设为唯一出口，另一条链路将被完全隔离，无自动故障切换保护'),
					'disabled': (this.applying || isOnly) ? 'disabled' : null,
					'click': L.bind(this.selectOnly, this, kind)
				}, isOnly ? _('当前为独占出口') : _('仅用此出口 (移除备用)'))
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

		if (data.split === '1') {
			return { text: this.splitSentence(data), cls: 'h5net-note alert' };
		}
		if (active === 'none') {
			return { text: _('⚠️ 未发现任何可用的默认出口网关，请检查物理网线或 SIM 卡射频注网状态。'), cls: 'h5net-note alert' };
		}
		if (mode === 'wan_only' || mode === 'modem_only') {
			return { text: _('ℹ️ 当前处于单链路独占模式（%s）。容灾备选路由已停用，链路中断将直接离线。').format(this.exitLabel(preferred)), cls: 'h5net-note warn' };
		}
		if (active === fallback && active6 === 'none') {
			return { text: _('⚡ 首选 %s 异常，IPv4 已平滑降级至备用 %s；IPv6 临时下线以阻止出口分流。').format(this.exitLabel(preferred), this.exitLabel(fallback)), cls: 'h5net-note warn' };
		}
		if (active === fallback) {
			return { text: _('⚡ 首选 %s 离线，IPv4/IPv6 双栈已完整接管至备用出口 %s。').format(this.exitLabel(preferred), this.exitLabel(fallback)), cls: 'h5net-note warn' };
		}
		if (active === preferred && active6 === 'none') {
			return { text: _('IPv4 正经由首选 %s 传输。该出口暂无 IPv6 连通性，备用出口 IPv6 被策略性封锁以确保同路。').format(this.exitLabel(preferred)), cls: 'h5net-note' };
		}
		if (active === preferred) {
			return { text: _('✔ 双栈网络畅通：IPv4 与 IPv6 均稳定经由首选出口 %s 发送，热备链路就绪。').format(this.exitLabel(preferred)), cls: 'h5net-note' };
		}
		return { text: _('当前数据流量由外部系统路由（例如 mwan3、VPN 或 daed）接管调度。'), cls: 'h5net-note warn' };
	},

	metaBar: function(data) {
		return E('div', { 'class': 'h5net-meta' }, [
			E('span', {}, [
				_('守护看门狗：'),
				E('b', { 'class': data.watcher === 'on' ? '' : 'off' }, data.watcher === 'on' ? _('运行中') : _('已挂起'))
			]),
			E('span', {}, [
				_('实时健康探测：'),
				E('b', { 'class': data.health_check === '1' ? '' : 'off' }, data.health_check === '1' ? _('已就绪') : _('未开启'))
			]),
			E('span', {}, [
				_('双栈对称原则：'),
				E('b', {}, _('强制 IPv6 伴随 IPv4'))
			])
		]);
	},

	// ------------------------------------------------------------------
	// 切换任务 (后台作业)
	//
	// 后端把切换做成一个后台任务：`set` 立刻返回，工作进程在后台推进状态机，中途
	// 任何一步都不阻塞 LuCI 的 HTTP/ubus 调用。因此这里从不等待网络操作本身，只
	// 轮询只读的 `status`，并把状态机当前阶段显示出来。
	// ------------------------------------------------------------------
	switchState: function(data) {
		return (data || {}).switch_state || '';
	},

	switchBusy: function(data) {
		return ((data || {}).switch_busy || '0') === '1';
	},

	switchRunning: function() {
		return this.switchBusy(this.liveData);
	},

	// `set` 在已有任务运行时会以 rc=3 返回（请求已排队），两者都不是错误。
	execStatus: function(args) {
		return fs.exec('/usr/sbin/h5000m-netmode', args).then(function(res) {
			return { code: (res && res.code) || 0, stdout: (res && res.stdout) || '' };
		}, function(err) {
			return {
				code: (err && err.code) || 1,
				stdout: (err && (err.stdout || '')) || ''
			};
		});
	},

	phaseList: function(data) {
		var out = [];
		String((data || {}).switch_phases || '').trim().split(/\s+/).forEach(function(part) {
			if (!part) return;
			var pos = part.indexOf(':');
			if (pos > 0)
				out.push({ name: part.substring(0, pos), time: part.substring(pos + 1) });
		});
		return out;
	},

	phaseLabel: function(name) {
		var map = {
			PREPARING_TARGET: _('准备目标出口'),
			WAIT_IPV4: _('等待 IPv4 就绪'),
			WAIT_IPV6: _('等待 IPv6 就绪'),
			VERIFY_IPV4: _('验证 IPv4 连通性'),
			VERIFY_IPV6: _('验证 IPv6 连通性'),
			SWITCHING: _('切换默认路由'),
			VERIFY_TARGET: _('复核新出口'),
			COMMITTED: _('切换完成'),
			ROLLBACK: _('回滚'),
			FAILED: _('切换失败'),
			IDLE: _('空闲')
		};
		return map[name] || name;
	},

	// 步骤条与后端状态机同序：执行到哪一步，就点亮到哪一步。
	switchSteps: function(data) {
		var order = [ 'PREPARING_TARGET', 'WAIT_IPV4', 'WAIT_IPV6', 'VERIFY_IPV4',
			'VERIFY_IPV6', 'SWITCHING', 'VERIFY_TARGET' ];
		var state = this.switchState(data);
		var seen = {};
		this.phaseList(data).forEach(function(phase) { seen[phase.name] = phase.time; });
		return order.map(function(name) {
			var cls = 'h5net-switch-step';
			if (seen[name])
				cls += ' done';
			if (name === state)
				cls += state === 'FAILED' ? ' failed' : ' active';
			return { name: name, cls: cls, time: seen[name] || '' };
		});
	},

	switchPanel: function(data) {
		var busy = this.switchBusy(data);
		var state = this.switchState(data);
		if (!busy && state !== 'FAILED')
			return null;
		if (this.switchDismissed === state && !busy)
			return null;

		var target = (data.switch_target && data.switch_target !== 'none')
			? this.exitLabel(data.switch_target) : '';
		var meta = [];
		if (target)
			meta.push(E('span', {}, [ _('目标出口：'), E('b', {}, target) ]));
		if (data.switch_message)
			meta.push(E('span', {}, [ _('状态：'), E('b', {}, data.switch_message) ]));
		if (data.switch_elapsed)
			meta.push(E('span', {}, [ _('耗时：'), E('b', {}, data.switch_elapsed) ]));
		if (!busy) {
			meta.push(E('span', {}, [ _('失败原因：'), E('b', {}, this.switchReason(data)) ]));
			meta.push(E('span', {}, [ _('当前出口：'), E('b', {}, this.exitLabel(data.active4) + ' / ' + this.exitLabel(data.active6)) ]));
		}

		var steps = this.switchSteps(data).map(function(step) {
			return E('span', { 'class': step.cls, 'title': step.time ? step.name + ' ' + step.time : step.name },
				this.phaseLabel(step.name) + (step.time ? ' ' + step.time : ''));
		}, this);

		var children = [
			E('div', { 'class': 'h5net-switch-head' }, [
				E('span', { 'class': busy ? 'spin' : (state === 'FAILED' ? 'dot-fail' : 'dot-ok') }),
				busy ? _('正在切换出口，页面保持可用…')
					: (state === 'FAILED' ? _('上次切换未完成，已恢复到原出口') : _('出口切换完成'))
			]),
			E('div', { 'class': 'h5net-switch-meta' }, meta),
			E('div', { 'class': 'h5net-switch-steps' }, steps)
		];
		if (!busy) {
			children.push(E('div', { 'class': 'h5net-switch-actions' }, [
				E('button', {
					'class': 'cbi-button cbi-button-neutral',
					'click': L.bind(function(ev) {
						ev.preventDefault();
						this.switchDismissed = state;
						this.repaint();
					}, this)
				}, _('知道了'))
			]));
		}
		return E('div', { 'class': 'h5net-switch ' + (busy ? 'busy' : 'failed') }, children);
	},

	// 后端的状态机原因码 -> 中文说明（未知原因原样显示）
	switchReason: function(data) {
		var map = {
			ipv4_not_ready: _('目标出口的 IPv4 未在限时内就绪'),
			ipv6_not_ready: _('目标出口的 IPv6 未在限时内就绪'),
			ipv4_not_configured: _('目标出口没有 IPv4 接口'),
			ipv6_not_configured: _('目标出口没有 IPv6 接口（双栈出口要求，拒绝切换）'),
			ipv4_unreachable: _('目标出口的 IPv4 探测不通'),
			ipv6_unreachable: _('目标出口的 IPv6 探测不通'),
			ipv4_gateway_unreachable: _('目标出口的 IPv4 网关不可达'),
			ipv4_commit_failed: _('IPv4 默认路由切换失败'),
			ipv6_commit_failed: _('IPv6 默认路由切换失败'),
			priority_verify_failed: _('切换后出口优先级校验失败'),
			priority_verify_failed_after_settle: _('静置后出口优先级校验失败'),
			target_unreachable_after_settle: _('静置后目标出口不可达'),
			policy_write_failed: _('策略写入失败'),
			prepare_failed: _('目标出口准备失败')
		};
		var reason = data.switch_reason || '';
		if (!reason)
			return _('未记录');
		return map[reason] || reason;
	},

	// 轮询直到后台任务结束。总时长有上限，到期后不阻塞页面，只在通知里说明仍在
	// 后台执行 —— 切换的正确性由后端的状态机与回滚保证，与页面是否停留无关。
	waitForSwitch: function(maxSeconds) {
		var deadline = Date.now() + (maxSeconds || 90) * 1000;

		return new Promise(L.bind(function(resolve) {
			var tick = L.bind(function() {
				this.statusCommand().then(L.bind(function(res) {
					this.liveData = this.parseStatus(res);
					this.switchFastPoll = this.switchBusy(this.liveData);
					this.repaint();

					var state = this.switchState(this.liveData);
					var done = !this.switchBusy(this.liveData) &&
						(state === 'COMMITTED' || state === 'FAILED' || state === 'IDLE' || state === '');
					if (done) {
						this.applying = false;
						this.syncPollInterval();
						this.repaint();
						this.notifySwitchOutcome(this.liveData);
						resolve(this.liveData);
						return;
					}
					if (Date.now() >= deadline) {
						// 后台仍在执行：把控制权还给页面，任务由后端继续完成。
						this.applying = false;
						this.syncPollInterval();
						ui.addNotification(null, E('p',
							_('切换任务仍在后台执行，页面将继续自动刷新结果。')), 'info');
						resolve(this.liveData);
						return;
					}
					window.setTimeout(tick, 1000);
				}, this));
			}, this);
			window.setTimeout(tick, 300);
		}, this));
	},

	notifySwitchOutcome: function(data) {
		var state = this.switchState(data);
		if (state === 'FAILED') {
			ui.addNotification(null, E('p',
				_('切换失败：') + this.switchReason(data) + _('，已保持/恢复到可用出口。')), 'danger');
			return;
		}
		if (state === 'COMMITTED') {
			ui.addNotification(null, E('p',
				_('切换完成：IPv4 与 IPv6 均已经由 %s 出口。').format(this.exitLabel(data.active4 || data.active6))));
		}
	},

	// 切换进行中把轮询间隔收紧到 1 秒（阶段变化要立刻可见），结束后回到 5 秒。
	syncPollInterval: function() {
		var want = this.switchRunning() ? 1 : 5;
		if (want === this.pollInterval)
			return;
		try {
			if (this.pollHandle != null && typeof poll.remove === 'function')
				poll.remove(this.pollHandle);
			this.pollHandle = poll.add(L.bind(this.refreshStatus, this), want);
			this.pollInterval = want;
		}
		catch (e) {
			// 老版本 LuCI 不支持 poll.remove：保持原有间隔即可，状态依旧会刷新。
			this.pollInterval = this.pollInterval || 5;
		}
	},

	applySelection: function() {
		if (this.applying || this.switchRunning()) return;

		var curWanDev = (this.deviceMap || {}).wan || '';
		var curModemDev = (this.deviceMap || {}).modem || '';
		var newWanDev = (this.pendingDeviceMap || {}).wan || curWanDev;
		var newModemDev = (this.pendingDeviceMap || {}).modem || curModemDev;
		var modeChanged = this.pendingMode !== ((this.liveData || {}).mode || 'wan_first');
		var deviceChanged = curWanDev !== newWanDev || curModemDev !== newModemDev;

		if (!modeChanged && !deviceChanged) return;

		// 接口绑定是两次 UCI 写入，会立即返回；出口策略 `set` 只负责创建后台
		// 任务后立刻返回，真正的切换在后台进行。所以这里先写接口映射，再发起切换
		// 并轮询进度 —— 全程不阻塞页面，也不允许重复点击启动第二个切换。
		var steps = [];
		if (curWanDev !== newWanDev && newWanDev)
			steps.push({ label: _('有线 WAN 接口'), args: [ 'set-device-map', 'wan', newWanDev ] });
		if (curModemDev !== newModemDev && newModemDev)
			steps.push({ label: _('5G 模组接口'), args: [ 'set-device-map', 'modem', newModemDev ] });

		this.applying = true;
		this.switchDismissed = null;
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
			this.selecting = false;
			this.deviceDirty = false;
			if (!modeChanged) {
				ui.addNotification(null, E('p', _('接口映射已保存。')));
				return this.refreshStatus().then(L.bind(function() {
					this.applying = false;
					this.repaint();
				}, this));
			}

			return this.execStatus([ 'set', this.pendingMode ]).then(L.bind(function(res) {
				if (res.code !== 0 && res.code !== 3)
					throw new Error(_('出口策略下发失败') + ' (rc=' + res.code + ')');
				if (res.code === 3)
					ui.addNotification(null, E('p', _('已有切换任务在执行，本次请求已排队，稍后自动执行。')), 'info');
				else
					ui.addNotification(null, E('p', _('切换任务已启动，正在后台切换出口…')));
				return this.waitForSwitch(90);
			}, this));
		}, this)).catch(L.bind(function(err) {
			this.applying = false;
			this.repaint();
			ui.addNotification(null, E('p', _('配置应用失败：') + ' ' + (err.message || _('未知错误'))), 'danger');
		}, this));
	},

	reconcileExits: function() {
		if (this.applying || this.switchRunning()) return;

		this.applying = true;
		this.repaint();

		return fs.exec('/usr/sbin/h5000m-netmode', [ 'reconcile' ]).then(function() {
			ui.addNotification(null, E('p', _('已发送重对齐信令，IPv6 正在对齐 IPv4 出口。')));
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

		var split = data.split === '1';
		var busy = this.switchBusy(data);
		var message = this.statusMessage(data);

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
				'disabled': (this.applying || busy) ? 'disabled' : null,
				'click': L.bind(this.reconcileExits, this)
			}, _('⚡ 一键对齐双栈出口')));
		}
		buttons.push(E('button', {
			'class': 'cbi-button cbi-button-apply',
			'disabled': (!dirty || this.applying || busy) ? 'disabled' : null,
			'click': L.bind(this.applySelection, this)
		}, busy ? _('切换执行中…') : (this.applying ? _('策略部署中…') : _('保存并应用策略'))));

		// 切换进度卡片只在任务进行中或上次失败时出现。null 子节点在 LuCI 的 E() 里
		// 会被忽略，但显式过滤掉更清楚，也让离线的渲染检查工具不必特判。
		var sections = [
			this.styleNode(),
			this.exitCard(data),
			E('div', { 'class': message.cls }, message.text),
			this.switchPanel(data)
		].filter(function(node) { return node != null; });

		return E('div', { 'class': 'h5net', id: 'h5net-status' }, sections.concat([
			this.statusTiles(data),
			E('div', { 'class': 'h5net-grid' }, [
				this.routeCard('wan', data),
				this.routeCard('modem', data)
			]),
			E('div', { 'class': 'h5net-foot' }, [
				this.metaBar(data),
				E('div', { 'class': 'h5net-buttons' }, buttons)
			])
		]));
	},

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
			data.switch_state, data.switch_busy, data.switch_target, data.switch_target_mode,
			data.switch_reason, data.switch_message, data.switch_elapsed, data.switch_phases,
			this.switchDismissed,
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
			var previous = (this.liveData || {}).switch_state;
			this.liveData = this.parseStatus(results[0]);
			if (previous !== this.liveData.switch_state)
				this.switchDismissed = null;
			this.syncPollInterval();
			if (!this.applying) {
				if (!this.selecting)
					this.pendingMode = this.liveData.mode || 'wan_first';
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

		this.switchDismissed = null;
		this.renderedKey = this.renderKey(this.liveData);

		this.pollInterval = 5;
		this.pollHandle = poll.add(L.bind(this.refreshStatus, this), this.pollInterval);
		return this.statusPanel(this.liveData);
	}
});
