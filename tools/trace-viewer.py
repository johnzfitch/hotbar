#!/usr/bin/env python3
"""
Hotbar Trace Viewer — DeltaGraph Edition

A DeltaGraph-inspired TUI/htmx trace viewer for hotbar's SQLite trace database.
Reads from ~/.local/share/hotbar/traces.db and serves a retro-charting web UI.

Usage:
    python tools/trace-viewer.py [--port 8777] [--db path/to/traces.db]
    Then open http://localhost:8777 in your browser.
"""

import http.server
import json
import os
import sqlite3
import sys
import webbrowser
from datetime import datetime
from html import escape as h
from urllib.parse import parse_qs, urlparse

DEFAULT_PORT = 8777
DEFAULT_DB = os.path.expandvars("$HOME/.local/share/hotbar/traces.db")

# ── Color palette (derived from DeltaGraph Professional branding) ────

COLORS = {
    "burgundy": "#8B1A2B",
    "burgundy_light": "#a52a3a",
    "teal": "#2d8c9e",
    "teal_dark": "#1a5e6e",
    "amber": "#cc8822",
    "orange": "#cc5522",
    "red_hot": "#cc2222",
    "green": "#44aa66",
    "blue": "#4488cc",
    "purple": "#8866bb",
}

SPAN_COLORS = [
    "#8B1A2B", "#2d8c9e", "#cc8822", "#44aa66",
    "#cc5522", "#4488cc", "#8866bb", "#cc2222",
]

# ── HTML Template ────────────────────────────────────────────────────

HTML_PAGE = r"""<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Hotbar Trace Viewer — DeltaGraph Edition</title>
<script src="https://unpkg.com/htmx.org@2.0.4"></script>
<style>
*,*::before,*::after{box-sizing:border-box;margin:0;padding:0}
:root {
  --bg: #0c0a0e; --bg-panel: #141218; --bg-cell: #1a1720;
  --bg-hover: #221f28; --bg-active: #2a2530;
  --border: #2a2630; --border-light: #3a3540;
  --text: #d0ccd6; --text-dim: #6a6670; --text-bright: #eee8f0;
  --burgundy: #8B1A2B; --burgundy-l: #a52a3a; --burgundy-d: #6a1020;
  --teal: #2d8c9e; --teal-d: #1a5e6e; --teal-l: #3da8bc;
  --amber: #cc8822; --orange: #cc5522; --red: #cc2222;
  --green: #44aa66; --blue: #4488cc; --purple: #8866bb;
  --font: "Berkeley Mono", "SF Mono", "Fira Code", "JetBrains Mono", "Cascadia Code", monospace;
  --radius: 2px;
}
html,body { background:var(--bg); color:var(--text); font:13px/1.5 var(--font);
  height:100%; overflow:hidden; }

/* ── Mac System 7 Window Chrome ── */
.window { display:flex; flex-direction:column; height:100vh; border:1px solid var(--border-light); }
.title-bar {
  display:flex; align-items:center; gap:10px;
  padding:6px 12px; background:var(--burgundy);
  border-bottom:3px solid var(--teal);
  user-select:none; flex-shrink:0;
}
.title-bar .diamond { color:var(--teal-l); font-size:16px; }
.title-bar h1 { font-size:13px; font-weight:600; color:#fff; letter-spacing:1.5px; text-transform:uppercase; }
.title-bar .subtitle { color:rgba(255,255,255,.55); font-size:11px; letter-spacing:3px; margin-left:auto; }
.title-bar .win-btns { display:flex; gap:4px; margin-left:12px; }
.title-bar .win-btn {
  width:12px; height:12px; border-radius:50%;
  border:1px solid rgba(255,255,255,.2);
}
.win-btn.close { background:#cc4444; }
.win-btn.min { background:#ccaa22; }
.win-btn.max { background:#44aa44; }

/* ── Toolbar ── */
.toolbar {
  display:flex; gap:2px; padding:4px 8px;
  background:var(--bg-panel); border-bottom:1px solid var(--border);
  flex-shrink:0;
}
.toolbar button {
  background:var(--bg-cell); color:var(--text-dim); border:1px solid var(--border);
  padding:4px 14px; font:11px var(--font); cursor:pointer;
  letter-spacing:0.5px; transition:all 0.15s;
}
.toolbar button:hover { background:var(--bg-hover); color:var(--text); border-color:var(--border-light); }
.toolbar button.active {
  background:var(--burgundy-d); color:var(--text-bright);
  border-color:var(--burgundy); border-bottom:2px solid var(--teal);
}
.toolbar .spacer { flex:1; }
.toolbar .db-info { color:var(--text-dim); font-size:10px; align-self:center; letter-spacing:1px; }

/* ── Layout ── */
.main { display:flex; flex:1; overflow:hidden; }
.sidebar {
  width:220px; min-width:220px; background:var(--bg-panel);
  border-right:1px solid var(--border); display:flex; flex-direction:column;
  overflow-y:auto;
}
.content { flex:1; overflow-y:auto; padding:0; }

/* ── Sidebar Sections ── */
.sidebar-section { padding:8px; border-bottom:1px solid var(--border); }
.sidebar-section h3 {
  font-size:10px; letter-spacing:2px; color:var(--teal);
  margin-bottom:6px; text-transform:uppercase;
}
.session-item {
  display:block; padding:6px 8px; margin:2px 0;
  background:var(--bg-cell); border:1px solid transparent;
  cursor:pointer; text-decoration:none; color:inherit;
  transition:all 0.12s;
}
.session-item:hover { border-color:var(--border-light); background:var(--bg-hover); }
.session-item.active { border-color:var(--burgundy); background:var(--bg-active);
  border-left:3px solid var(--teal); }
.session-component { font-size:11px; font-weight:600; }
.session-component.daemon { color:var(--green); }
.session-component.panel { color:var(--amber); }
.session-meta { font-size:10px; color:var(--text-dim); }

.stat-row { display:flex; justify-content:space-between; padding:2px 0; font-size:11px; }
.stat-label { color:var(--text-dim); }
.stat-value { color:var(--text-bright); font-weight:600; }

/* ── Status Bar ── */
.status-bar {
  display:flex; align-items:center; padding:3px 12px; gap:16px;
  background:var(--bg-panel); border-top:1px solid var(--border);
  font-size:10px; color:var(--text-dim); letter-spacing:1.5px;
  flex-shrink:0;
}
.status-bar .brand { color:var(--burgundy-l); letter-spacing:3px; }

/* ── Content Views ── */
.view-header {
  padding:12px 16px 8px; border-bottom:1px solid var(--border);
  display:flex; align-items:baseline; gap:12px;
}
.view-header h2 { font-size:13px; color:var(--teal-l); letter-spacing:1.5px; text-transform:uppercase; }
.view-header .count { font-size:11px; color:var(--text-dim); }

/* ── Timeline View ── */
.timeline { padding:8px 16px; }
.timeline-row {
  display:flex; align-items:center; gap:8px; padding:3px 0;
  border-bottom:1px solid rgba(42,38,48,0.5);
  font-size:11px; transition:background 0.1s;
}
.timeline-row:hover { background:var(--bg-hover); }
.timeline-name { width:180px; min-width:180px; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
.timeline-bar-container { flex:1; height:18px; position:relative; }
.timeline-bar {
  height:100%; min-width:2px;
  border-radius:1px; position:relative; overflow:hidden;
  transition:width 0.3s ease;
}
/* DeltaGraph translucent pedestal effect */
.timeline-bar::after {
  content:''; position:absolute; inset:0;
  background:linear-gradient(180deg, rgba(255,255,255,0.15) 0%, transparent 40%, rgba(0,0,0,0.2) 100%);
}
.timeline-dur { width:70px; text-align:right; color:var(--text-dim); font-size:10px; font-variant-numeric:tabular-nums; }
.timeline-indent { display:inline-block; }
.timeline-tree { color:var(--border-light); margin-right:4px; }

/* ── Data Grid (DeltaGraph Notebook style) ── */
.data-grid { width:100%; border-collapse:collapse; font-size:11px; }
.data-grid thead { position:sticky; top:0; z-index:2; }
.data-grid th {
  background:var(--bg-panel); color:var(--teal);
  padding:6px 10px; text-align:left; font-weight:600;
  border-bottom:2px solid var(--border-light);
  font-size:10px; letter-spacing:1px; text-transform:uppercase;
}
.data-grid td {
  padding:4px 10px; border-bottom:1px solid var(--border);
  font-variant-numeric:tabular-nums;
}
.data-grid tr:hover td { background:var(--bg-hover); }
.data-grid .col-num { color:var(--text-dim); text-align:right; width:40px; }
/* DeltaGraph notebook row numbers */
.data-grid .row-num {
  background:var(--bg-panel); color:var(--text-dim); text-align:center;
  border-right:2px solid var(--border-light); width:36px; font-size:10px;
}

/* Level badges */
.level { padding:1px 5px; font-size:9px; letter-spacing:0.5px; font-weight:600; border-radius:var(--radius); }
.level-TRACE { color:#666; }
.level-DEBUG { color:var(--blue); }
.level-INFO { color:var(--green); }
.level-WARN { color:var(--amber); background:rgba(204,136,34,0.1); }
.level-ERROR { color:var(--red); background:rgba(204,34,34,0.1); }

/* ── Distribution Chart (DeltaGraph bar chart) ── */
.chart-section { padding:16px; }
.chart-title {
  font-size:11px; color:var(--text-bright); letter-spacing:1px;
  margin-bottom:12px; padding-bottom:4px;
  border-bottom:1px solid var(--border);
}
.chart-row { display:flex; align-items:center; gap:8px; margin:3px 0; font-size:11px; }
.chart-label { width:80px; text-align:right; color:var(--text-dim); font-size:10px; }
.chart-bar-wrap { flex:1; height:20px; background:var(--bg-cell); border:1px solid var(--border); position:relative; }
.chart-bar {
  height:100%; position:relative; overflow:hidden; transition:width 0.4s ease;
}
.chart-bar::after {
  content:''; position:absolute; inset:0;
  background:linear-gradient(180deg, rgba(255,255,255,0.18) 0%, transparent 35%, rgba(0,0,0,0.15) 100%);
}
.chart-count { width:60px; font-size:10px; color:var(--text-dim); font-variant-numeric:tabular-nums; }

/* ── Grid lines overlay (DeltaGraph signature) ── */
.chart-bar-wrap::before {
  content:''; position:absolute; inset:0; z-index:1; pointer-events:none;
  background:repeating-linear-gradient(90deg, transparent, transparent 19.9%, rgba(255,255,255,0.03) 20%);
}

/* Axis */
.chart-axis { display:flex; gap:0; font-size:9px; color:var(--text-dim); margin-top:2px; padding-left:88px; }
.chart-axis span { flex:1; text-align:center; border-left:1px solid var(--border); padding-left:2px; }

/* ── Top Spans Table ── */
.top-spans { padding:16px; }

/* ── Heatmap (Lotus 1-2-3 grid) ── */
.heatmap-wrap { padding:16px; overflow-x:auto; }
.heatmap { border-collapse:collapse; font-size:10px; }
.heatmap th {
  background:var(--bg-panel); color:var(--teal); padding:3px 6px;
  font-weight:600; letter-spacing:0.5px; position:sticky; top:0; z-index:2;
  border:1px solid var(--border);
}
.heatmap .lotus-col { text-align:center; min-width:48px; color:var(--teal-l); font-weight:700; }
.heatmap .lotus-row {
  background:var(--bg-panel); color:var(--text-dim); text-align:center;
  border:1px solid var(--border); padding:2px 6px; font-weight:600;
  min-width:30px; position:sticky; left:0; z-index:1;
}
.heatmap td {
  border:1px solid var(--border); padding:3px 6px; text-align:center;
  font-variant-numeric:tabular-nums; transition:background 0.15s;
  cursor:default; min-width:48px;
}
.heatmap td:hover { outline:1px solid var(--teal); z-index:3; position:relative; }
/* Lotus 1-2-3 cell reference overlay */
.heatmap td[title]:hover::after {
  content:attr(title); position:absolute; bottom:100%; left:50%;
  transform:translateX(-50%); background:var(--bg-panel); color:var(--text);
  border:1px solid var(--border-light); padding:2px 6px; font-size:9px;
  white-space:nowrap; z-index:10; pointer-events:none;
}
.lotus-formula-bar {
  padding:4px 12px; background:var(--bg-cell); border-bottom:1px solid var(--border);
  font-size:10px; color:var(--text-dim); display:flex; gap:8px;
}
.lotus-cell-ref { color:var(--teal-l); font-weight:700; min-width:40px; }
.heat-legend { display:flex; gap:2px; align-items:center; padding:8px 16px; font-size:10px; }
.heat-legend-swatch { width:16px; height:12px; border:1px solid var(--border); }
.heat-legend span { color:var(--text-dim); margin:0 6px 0 2px; }

/* ── Sparkline Trend (Lotus line chart) ── */
.sparkline-section { padding:16px; }
.sparkline-chart {
  display:flex; align-items:flex-end; gap:1px; height:160px;
  border-left:2px solid var(--border-light); border-bottom:2px solid var(--border-light);
  padding:0 0 0 0; position:relative; margin:12px 0;
}
.sparkline-bar {
  flex:1; min-width:1px; max-width:6px; transition:height 0.2s;
  position:relative; cursor:crosshair;
}
.sparkline-bar:hover { opacity:0.8; }
.sparkline-bar:hover::after {
  content:attr(data-val); position:absolute; bottom:100%; left:50%;
  transform:translateX(-50%); background:var(--bg-panel); color:var(--text);
  border:1px solid var(--border-light); padding:1px 4px; font-size:9px;
  white-space:nowrap; z-index:5;
}
/* Y-axis labels */
.sparkline-y-axis { position:absolute; left:-42px; top:0; bottom:0;
  display:flex; flex-direction:column; justify-content:space-between;
  font-size:9px; color:var(--text-dim); font-variant-numeric:tabular-nums; }
/* Harvard Graphics grid lines */
.sparkline-grid { position:absolute; inset:0; pointer-events:none;
  background:repeating-linear-gradient(0deg, transparent, transparent 31px, rgba(255,255,255,0.03) 32px);
}
/* Threshold line */
.sparkline-threshold {
  position:absolute; left:0; right:0; border-top:1px dashed var(--red);
  font-size:9px; color:var(--red); text-align:right; padding-right:4px;
}
.sparkline-title {
  font-size:12px; color:var(--text-bright); letter-spacing:1px;
  padding:4px 0; border-bottom:2px solid var(--burgundy);
  margin-bottom:4px; font-weight:700;
}
.sparkline-subtitle { font-size:10px; color:var(--text-dim); font-weight:400; margin-left:12px; }
/* Harvard Graphics ruled title style */
.hg-ruled { border-top:3px solid var(--burgundy); border-bottom:1px solid var(--burgundy);
  padding:6px 0; margin-bottom:16px; }

/* ── Pie Chart (Harvard Graphics) ── */
.pie-section { padding:16px; display:flex; gap:32px; flex-wrap:wrap; }
.pie-chart-wrap { display:flex; flex-direction:column; align-items:center; }
.pie-chart {
  width:220px; height:220px; border-radius:50%;
  position:relative; margin:12px;
  box-shadow:4px 4px 0 rgba(0,0,0,0.3);
}
/* Harvard Graphics 3D pie effect */
.pie-chart::after {
  content:''; position:absolute; bottom:-8px; left:4px; right:-4px; top:8px;
  border-radius:50%; z-index:-1;
  background:linear-gradient(180deg, transparent 40%, rgba(0,0,0,0.4) 100%);
}
.pie-legend { font-size:11px; }
.pie-legend-item { display:flex; align-items:center; gap:8px; padding:3px 0; }
.pie-legend-swatch { width:14px; height:14px; border:1px solid rgba(255,255,255,0.1);
  box-shadow:1px 1px 0 rgba(0,0,0,0.3); }
.pie-legend-label { color:var(--text); }
.pie-legend-pct { color:var(--text-dim); font-variant-numeric:tabular-nums; margin-left:auto; min-width:50px; text-align:right; }
/* Harvard Graphics chart frame */
.hg-frame {
  border:2px solid var(--border-light); padding:16px;
  background:var(--bg-panel); position:relative;
}
.hg-frame-title {
  position:absolute; top:-10px; left:16px; background:var(--bg-panel);
  padding:0 8px; font-size:11px; color:var(--teal-l); letter-spacing:1px;
  font-weight:600;
}

/* ── Waterfall (Harvard Graphics cascade) ── */
.waterfall-wrap { padding:16px; overflow-x:auto; }
.waterfall-row { display:flex; align-items:center; height:24px; margin:1px 0; font-size:11px; }
.waterfall-label { width:140px; min-width:140px; text-align:right; padding-right:8px;
  overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
.waterfall-track { flex:1; height:20px; position:relative; }
.waterfall-bar {
  position:absolute; height:100%; min-width:2px; border-radius:1px;
  overflow:hidden;
}
.waterfall-bar::after {
  content:''; position:absolute; inset:0;
  background:linear-gradient(180deg, rgba(255,255,255,0.2) 0%, transparent 40%, rgba(0,0,0,0.15) 100%);
}
.waterfall-connector { position:absolute; top:50%; width:1px; height:50%;
  border-left:1px dotted var(--border-light); }
.waterfall-dur { position:absolute; top:1px; right:4px; font-size:9px;
  color:rgba(255,255,255,0.7); text-shadow:0 0 2px rgba(0,0,0,0.8); }
.waterfall-axis {
  display:flex; border-top:1px solid var(--border-light); margin-top:4px;
  padding-top:2px; padding-left:148px; font-size:9px; color:var(--text-dim);
}
.waterfall-axis span { flex:1; }

/* ── Empty state ── */
.empty-state {
  display:flex; flex-direction:column; align-items:center; justify-content:center;
  height:100%; color:var(--text-dim); gap:12px; padding:40px;
}
.empty-state .diamond { font-size:48px; color:var(--burgundy-l); }
.empty-state p { text-align:center; max-width:400px; line-height:1.8; }
.empty-state code { color:var(--teal); background:var(--bg-cell); padding:2px 8px; }

/* ── Filter controls ── */
.filter-bar { display:flex; gap:4px; padding:8px 16px; border-bottom:1px solid var(--border); flex-wrap:wrap; }
.filter-btn {
  background:var(--bg-cell); color:var(--text-dim); border:1px solid var(--border);
  padding:2px 10px; font:10px var(--font); cursor:pointer;
  letter-spacing:0.5px; transition:all 0.12s;
}
.filter-btn:hover { color:var(--text); border-color:var(--border-light); }
.filter-btn.active { color:var(--text-bright); border-color:var(--teal-d); background:rgba(45,140,158,0.1); }

/* ── Scrollbar styling ── */
::-webkit-scrollbar { width:8px; height:8px; }
::-webkit-scrollbar-track { background:var(--bg); }
::-webkit-scrollbar-thumb { background:var(--border-light); border-radius:4px; }
::-webkit-scrollbar-thumb:hover { background:var(--text-dim); }

/* ── Animations ── */
@keyframes fadeIn { from{opacity:0;transform:translateY(4px)} to{opacity:1;transform:none} }
.htmx-added { animation:fadeIn 0.2s ease; }

/* ── Two-panel split inside content ── */
.split-v { display:flex; flex-direction:column; height:100%; }
.split-v > .top-panel { flex:1; overflow-y:auto; min-height:200px; }
.split-v > .bottom-panel { border-top:2px solid var(--border-light);
  max-height:45%; overflow-y:auto; flex-shrink:0; }

.fields { color:var(--text-dim); font-size:10px; }

/* Loading */
.htmx-indicator { display:none; }
.htmx-request .htmx-indicator { display:inline; }
.htmx-request.htmx-indicator { display:inline; }
</style>
</head>
<body>
<div class="window">
  <!-- Title Bar (Mac System 7 style) -->
  <div class="title-bar">
    <span class="diamond">&#9670;</span>
    <h1>DeltaGraph Trace Viewer</h1>
    <span class="subtitle">H O T B A R&ensp;&ensp;v 2 . 0</span>
    <div class="win-btns">
      <div class="win-btn close"></div>
      <div class="win-btn min"></div>
      <div class="win-btn max"></div>
    </div>
  </div>

  <!-- Toolbar -->
  <div class="toolbar">
    <button class="active" onclick="switchView('timeline',this)"
      hx-get="/htmx/timeline" hx-target="#content" hx-include="#session-id"
      >Timeline</button>
    <button onclick="switchView('events',this)"
      hx-get="/htmx/events" hx-target="#content" hx-include="#session-id"
      >Events</button>
    <button onclick="switchView('performance',this)"
      hx-get="/htmx/performance" hx-target="#content" hx-include="#session-id"
      >Performance</button>
    <button onclick="switchView('top-spans',this)"
      hx-get="/htmx/top-spans" hx-target="#content" hx-include="#session-id"
      >Top Spans</button>
    <button onclick="switchView('heatmap',this)"
      hx-get="/htmx/heatmap" hx-target="#content" hx-include="#session-id"
      >Heatmap</button>
    <button onclick="switchView('sparkline',this)"
      hx-get="/htmx/sparkline" hx-target="#content" hx-include="#session-id"
      >Trend</button>
    <button onclick="switchView('pie',this)"
      hx-get="/htmx/pie" hx-target="#content" hx-include="#session-id"
      >Pie</button>
    <button onclick="switchView('waterfall',this)"
      hx-get="/htmx/waterfall" hx-target="#content" hx-include="#session-id"
      >Waterfall</button>
    <div class="spacer"></div>
    <span class="db-info">__DB_INFO__</span>
  </div>

  <!-- Main Layout -->
  <div class="main">
    <!-- Sidebar -->
    <div class="sidebar" hx-get="/htmx/sessions" hx-trigger="load" hx-target="this">
      <div class="sidebar-section"><h3>Loading...</h3></div>
    </div>

    <!-- Content Area -->
    <div class="content" id="content">
      <div class="empty-state">
        <div class="diamond">&#9670;</div>
        <p>Select a session from the sidebar to view trace data.</p>
        <p>Trace database: <code>__DB_PATH__</code></p>
      </div>
    </div>
  </div>

  <!-- Status Bar -->
  <div class="status-bar">
    <span class="brand">D E L T A P O I N T ,&ensp;I N C .</span>
    <span>__DB_STATUS__</span>
    <div class="spacer"></div>
    <span id="status-text">Ready</span>
  </div>
</div>

<!-- Hidden input for session tracking -->
<input type="hidden" id="session-id" name="session_id" value="">

<script>
function switchView(view, btn) {
  document.querySelectorAll('.toolbar button').forEach(b => b.classList.remove('active'));
  btn.classList.add('active');
}

function selectSession(id, el) {
  document.getElementById('session-id').value = id;
  document.querySelectorAll('.session-item').forEach(s => s.classList.remove('active'));
  el.classList.add('active');
  // Trigger the active toolbar button
  const activeBtn = document.querySelector('.toolbar button.active');
  if (activeBtn) activeBtn.click();
  document.getElementById('status-text').textContent = 'Session #' + id;
}

function setEventLevel(level, btn) {
  document.querySelectorAll('.filter-btn').forEach(b => b.classList.remove('active'));
  btn.classList.add('active');
}

// Auto-select first session after sidebar loads
document.body.addEventListener('htmx:afterSwap', function(e) {
  if (e.detail.target.classList.contains('sidebar')) {
    const first = e.detail.target.querySelector('.session-item');
    if (first) first.click();
  }
});
</script>
</body>
</html>"""


# ── htmx Partial Renderers ───────────────────────────────────────────

def render_sessions(db):
    """Sidebar: session list + stats."""
    cur = db.execute(
        "SELECT id, pid, component, started_at FROM sessions ORDER BY started_at DESC"
    )
    sessions = cur.fetchall()

    total_spans = db.execute("SELECT COUNT(*) FROM spans").fetchone()[0]
    total_events = db.execute("SELECT COUNT(*) FROM events").fetchone()[0]

    parts = ['<div class="sidebar-section"><h3>Sessions</h3>']

    for sid, pid, component, started_at in sessions:
        ts = datetime.fromtimestamp(started_at).strftime("%Y-%m-%d %H:%M:%S")
        comp_class = "daemon" if component == "daemon" else "panel"
        span_count = db.execute(
            "SELECT COUNT(*) FROM spans WHERE session_id=?", (sid,)
        ).fetchone()[0]
        event_count = db.execute(
            "SELECT COUNT(*) FROM events WHERE session_id=?", (sid,)
        ).fetchone()[0]

        parts.append(f'''<div class="session-item" onclick="selectSession({sid},this)">
  <div class="session-component {comp_class}">&#9679; {h(component)}</div>
  <div class="session-meta">PID {pid} &middot; {h(ts)}</div>
  <div class="session-meta">{span_count:,} spans &middot; {event_count:,} events</div>
</div>''')

    parts.append("</div>")

    # Stats section
    parts.append('<div class="sidebar-section"><h3>Totals</h3>')
    parts.append(f'<div class="stat-row"><span class="stat-label">Sessions</span><span class="stat-value">{len(sessions):,}</span></div>')
    parts.append(f'<div class="stat-row"><span class="stat-label">Spans</span><span class="stat-value">{total_spans:,}</span></div>')
    parts.append(f'<div class="stat-row"><span class="stat-label">Events</span><span class="stat-value">{total_events:,}</span></div>')

    # DB size
    try:
        db_size = os.path.getsize(db_path_global)
        if db_size > 1_048_576:
            size_str = f"{db_size / 1_048_576:.1f} MB"
        else:
            size_str = f"{db_size / 1024:.0f} KB"
        parts.append(f'<div class="stat-row"><span class="stat-label">DB Size</span><span class="stat-value">{size_str}</span></div>')
    except OSError:
        pass

    parts.append("</div>")
    return "\n".join(parts)


def render_timeline(db, session_id):
    """Span timeline with nested hierarchy and duration bars."""
    if not session_id:
        return '<div class="empty-state"><p>Select a session first.</p></div>'

    cur = db.execute(
        """SELECT id, parent_id, name, target, level, start_us, end_us, fields
           FROM spans WHERE session_id=?
           ORDER BY start_us ASC LIMIT 2000""",
        (session_id,),
    )
    spans = cur.fetchall()

    if not spans:
        return '<div class="empty-state"><p>No spans recorded for this session.</p></div>'

    # Find max duration for bar scaling
    max_dur = max((end - start) for _, _, _, _, _, start, end, _ in spans) or 1

    # Build parent→children map for tree rendering
    children = {}
    span_map = {}
    roots = []
    for sid, pid, name, target, level, start, end, fields in spans:
        span_map[sid] = (sid, pid, name, target, level, start, end, fields)
        if pid:
            children.setdefault(pid, []).append(sid)
        else:
            roots.append(sid)

    parts = [
        '<div class="view-header">',
        f'<h2>Span Timeline</h2><span class="count">{len(spans):,} spans</span>',
        '</div><div class="timeline">',
    ]

    color_map = {}
    color_idx = [0]

    def get_color(name):
        if name not in color_map:
            color_map[name] = SPAN_COLORS[color_idx[0] % len(SPAN_COLORS)]
            color_idx[0] += 1
        return color_map[name]

    def render_span(sid, depth=0):
        s = span_map.get(sid)
        if not s:
            return
        _, _, name, target, level, start, end, fields = s
        dur_us = end - start
        dur_ms = dur_us / 1000.0
        pct = min((dur_us / max_dur) * 100, 100)
        color = get_color(name)

        indent = depth * 16
        tree_char = "&#9500; " if depth > 0 else ""

        if dur_ms >= 1000:
            dur_str = f"{dur_ms/1000:.2f}s"
        elif dur_ms >= 1:
            dur_str = f"{dur_ms:.1f}ms"
        else:
            dur_str = f"{dur_us}&#181;s"

        fields_str = ""
        if fields:
            fields_str = f' <span class="fields">{h(fields)}</span>'

        parts.append(
            f'<div class="timeline-row">'
            f'<div class="timeline-name" title="{h(target)}::{h(name)}">'
            f'<span class="timeline-indent" style="width:{indent}px"></span>'
            f'<span class="timeline-tree">{tree_char}</span>{h(name)}{fields_str}</div>'
            f'<div class="timeline-bar-container">'
            f'<div class="timeline-bar" style="width:{max(pct, 0.5):.1f}%;background:{color}"></div>'
            f'</div>'
            f'<div class="timeline-dur">{dur_str}</div>'
            f'</div>'
        )

        for child_id in children.get(sid, []):
            render_span(child_id, depth + 1)

    for root_id in roots:
        render_span(root_id)

    # Also render orphaned spans (parent not in this result set)
    rendered = set()

    def collect_rendered(sid):
        rendered.add(sid)
        for child_id in children.get(sid, []):
            collect_rendered(child_id)

    for root_id in roots:
        collect_rendered(root_id)

    for sid, pid, name, target, level, start, end, fields in spans:
        if sid not in rendered:
            render_span(sid, 0)
            rendered.add(sid)

    parts.append("</div>")
    return "\n".join(parts)


def render_events(db, session_id, level_filter="ALL"):
    """Event log as DeltaGraph notebook-style data grid."""
    if not session_id:
        return '<div class="empty-state"><p>Select a session first.</p></div>'

    where = "WHERE session_id=?"
    params = [session_id]
    if level_filter and level_filter != "ALL":
        where += " AND level=?"
        params.append(level_filter)

    cur = db.execute(
        f"""SELECT id, span_id, level, target, message, timestamp_us, fields
            FROM events {where}
            ORDER BY timestamp_us ASC LIMIT 2000""",
        params,
    )
    events = cur.fetchall()

    levels = ["ALL", "DEBUG", "INFO", "WARN", "ERROR"]

    parts = [
        '<div class="view-header">',
        f'<h2>Event Log</h2><span class="count">{len(events):,} events</span>',
        "</div>",
        '<div class="filter-bar">',
    ]

    for lv in levels:
        active = "active" if lv == level_filter else ""
        parts.append(
            f'<button class="filter-btn {active}" '
            f'hx-get="/htmx/events?session_id={session_id}&level={lv}" '
            f'hx-target="#content" '
            f'onclick="setEventLevel(\'{lv}\',this)">{lv}</button>'
        )

    parts.append("</div>")

    if not events:
        parts.append('<div class="empty-state"><p>No events at this level.</p></div>')
        return "\n".join(parts)

    parts.append(
        '<div style="overflow-y:auto;max-height:calc(100vh - 200px)">'
        '<table class="data-grid"><thead><tr>'
        '<th class="row-num">#</th>'
        '<th>Time</th><th>Level</th><th>Target</th><th>Message</th><th>Fields</th>'
        "</tr></thead><tbody>"
    )

    for i, (eid, span_id, level, target, message, ts_us, fields) in enumerate(events, 1):
        # Format timestamp as relative seconds
        ts_s = ts_us / 1_000_000.0
        ts_str = f"{ts_s:.3f}s"

        parts.append(
            f'<tr>'
            f'<td class="row-num">{i}</td>'
            f'<td style="font-variant-numeric:tabular-nums">{ts_str}</td>'
            f'<td><span class="level level-{h(level)}">{h(level)}</span></td>'
            f'<td style="color:var(--text-dim)">{h(shorten_target(target))}</td>'
            f'<td>{h(message)}</td>'
            f'<td class="fields">{h(fields or "")}</td>'
            f"</tr>"
        )

    parts.append("</tbody></table></div>")
    return "\n".join(parts)


def render_performance(db, session_id):
    """Performance distribution chart (DeltaGraph bar chart style)."""
    if not session_id:
        return '<div class="empty-state"><p>Select a session first.</p></div>'

    cur = db.execute(
        "SELECT name, (end_us - start_us) as dur FROM spans WHERE session_id=? ORDER BY dur",
        (session_id,),
    )
    spans = cur.fetchall()

    if not spans:
        return '<div class="empty-state"><p>No spans recorded.</p></div>'

    durations = [dur for _, dur in spans]

    # Compute percentiles
    def percentile(data, p):
        k = (len(data) - 1) * p / 100.0
        f = int(k)
        c = f + 1 if f + 1 < len(data) else f
        return data[f] + (k - f) * (data[c] - data[f])

    durations.sort()
    p50 = percentile(durations, 50)
    p90 = percentile(durations, 90)
    p95 = percentile(durations, 95)
    p99 = percentile(durations, 99)
    mean_dur = sum(durations) / len(durations)

    # Distribution buckets
    buckets = [
        ("< 10\u00b5s", 0, 10),
        ("10-100\u00b5s", 10, 100),
        ("0.1-1ms", 100, 1000),
        ("1-5ms", 1000, 5000),
        ("5-10ms", 5000, 10000),
        ("10-50ms", 10000, 50000),
        ("50ms+", 50000, float("inf")),
    ]

    bucket_counts = []
    for label, lo, hi in buckets:
        count = sum(1 for d in durations if lo <= d < hi)
        bucket_counts.append((label, count))

    max_count = max(c for _, c in bucket_counts) or 1

    bar_colors = [
        COLORS["teal"], COLORS["blue"], COLORS["green"],
        COLORS["amber"], COLORS["orange"], COLORS["burgundy_light"], COLORS["red_hot"],
    ]

    parts = [
        '<div class="view-header">',
        f'<h2>Performance</h2><span class="count">{len(spans):,} spans analyzed</span>',
        "</div>",
        '<div class="split-v"><div class="top-panel">',
        # Percentile stats
        '<div class="chart-section">',
        '<div class="chart-title">Latency Percentiles</div>',
    ]

    stats = [
        ("Mean", mean_dur), ("P50", p50), ("P90", p90), ("P95", p95), ("P99", p99),
        ("Min", durations[0]), ("Max", durations[-1]),
    ]
    for label, val in stats:
        parts.append(
            f'<div class="stat-row"><span class="stat-label">{label}</span>'
            f'<span class="stat-value">{format_duration(val)}</span></div>'
        )

    parts.append("</div>")

    # Distribution chart
    parts.append('<div class="chart-section">')
    parts.append('<div class="chart-title">Duration Distribution</div>')

    for i, (label, count) in enumerate(bucket_counts):
        pct = (count / max_count * 100) if max_count > 0 else 0
        color = bar_colors[i % len(bar_colors)]
        parts.append(
            f'<div class="chart-row">'
            f'<div class="chart-label">{label}</div>'
            f'<div class="chart-bar-wrap">'
            f'<div class="chart-bar" style="width:{max(pct, 0.5):.1f}%;background:{color}"></div>'
            f'</div>'
            f'<div class="chart-count">{count:,}</div>'
            f'</div>'
        )

    # Axis labels
    parts.append('<div class="chart-axis">')
    step = max_count // 5 or 1
    for i in range(6):
        parts.append(f"<span>{i * step:,}</span>")
    parts.append("</div>")

    parts.append("</div></div>")

    # Bottom panel: per-name breakdown
    parts.append('<div class="bottom-panel">')
    parts.append('<div class="chart-section">')
    parts.append('<div class="chart-title">By Span Name (Mean Duration)</div>')

    name_stats = {}
    for name, dur in spans:
        if name not in name_stats:
            name_stats[name] = []
        name_stats[name].append(dur)

    name_avgs = [
        (name, sum(ds) / len(ds), len(ds))
        for name, ds in name_stats.items()
    ]
    name_avgs.sort(key=lambda x: -x[1])
    max_avg = name_avgs[0][1] if name_avgs else 1

    for i, (name, avg, count) in enumerate(name_avgs[:20]):
        pct = (avg / max_avg * 100) if max_avg > 0 else 0
        color = SPAN_COLORS[i % len(SPAN_COLORS)]
        parts.append(
            f'<div class="chart-row">'
            f'<div class="chart-label" title="{h(name)}">{h(name[:12])}</div>'
            f'<div class="chart-bar-wrap">'
            f'<div class="chart-bar" style="width:{max(pct, 0.5):.1f}%;background:{color}"></div>'
            f'</div>'
            f'<div class="chart-count">{format_duration(avg)} ({count:,}x)</div>'
            f'</div>'
        )

    parts.append("</div></div></div>")
    return "\n".join(parts)


def render_top_spans(db, session_id):
    """Top slowest spans as a DeltaGraph notebook data grid."""
    if not session_id:
        return '<div class="empty-state"><p>Select a session first.</p></div>'

    cur = db.execute(
        """SELECT id, name, target, level, start_us, end_us, fields,
                  (end_us - start_us) as dur
           FROM spans WHERE session_id=?
           ORDER BY dur DESC LIMIT 100""",
        (session_id,),
    )
    spans = cur.fetchall()

    if not spans:
        return '<div class="empty-state"><p>No spans recorded.</p></div>'

    parts = [
        '<div class="view-header">',
        f'<h2>Top Spans</h2><span class="count">Slowest 100</span>',
        "</div>",
        '<div style="overflow-y:auto;max-height:calc(100vh - 160px)">',
        '<table class="data-grid"><thead><tr>',
        '<th class="row-num">#</th>',
        "<th>Duration</th><th>Name</th><th>Target</th><th>Level</th><th>Fields</th>",
        "</tr></thead><tbody>",
    ]

    for i, (sid, name, target, level, start, end, fields, dur) in enumerate(spans, 1):
        parts.append(
            f"<tr>"
            f'<td class="row-num">{i}</td>'
            f"<td><strong>{format_duration(dur)}</strong></td>"
            f"<td>{h(name)}</td>"
            f'<td style="color:var(--text-dim)">{h(shorten_target(target))}</td>'
            f'<td><span class="level level-{h(level)}">{h(level)}</span></td>'
            f'<td class="fields">{h(fields or "")}</td>'
            f"</tr>"
        )

    parts.append("</tbody></table></div>")
    return "\n".join(parts)


def render_heatmap(db, session_id):
    """Lotus 1-2-3 style heatmap grid: span names x time buckets, cell color = avg duration."""
    if not session_id:
        return '<div class="empty-state"><p>Select a session first.</p></div>'

    cur = db.execute(
        "SELECT name, start_us, (end_us - start_us) as dur FROM spans WHERE session_id=? ORDER BY start_us",
        (session_id,),
    )
    spans = cur.fetchall()
    if not spans:
        return '<div class="empty-state"><p>No spans recorded.</p></div>'

    # Get distinct span names (top 20 by frequency)
    name_counts = {}
    for name, _, dur in spans:
        if name not in name_counts:
            name_counts[name] = [0, 0]
        name_counts[name][0] += 1
        name_counts[name][1] += dur
    top_names = sorted(name_counts.keys(), key=lambda n: -name_counts[n][0])[:20]

    # Time buckets: divide the session into N columns
    min_t = spans[0][1]
    max_t = spans[-1][1]
    time_range = max(max_t - min_t, 1)
    n_cols = min(26, max(8, len(spans) // 50))  # A-Z columns, Lotus style

    bucket_size = time_range / n_cols

    # Build grid[name][col] = list of durations
    grid = {name: [[] for _ in range(n_cols)] for name in top_names}
    for name, start, dur in spans:
        if name in grid:
            col = min(int((start - min_t) / bucket_size), n_cols - 1)
            grid[name][col].append(dur)

    # Find global max avg for color scaling
    all_avgs = []
    for name in top_names:
        for col_data in grid[name]:
            if col_data:
                all_avgs.append(sum(col_data) / len(col_data))
    max_avg = max(all_avgs) if all_avgs else 1

    # Column headers: A, B, C, ... (Lotus style)
    col_letters = [chr(65 + i) if i < 26 else str(i) for i in range(n_cols)]

    parts = [
        '<div class="view-header">',
        f'<h2>Heatmap</h2><span class="count">Lotus 1-2-3 Data Grid &middot; {len(top_names)} series &times; {n_cols} columns</span>',
        '</div>',
        '<div class="lotus-formula-bar">',
        '<span class="lotus-cell-ref">A1</span>',
        f'<span>Session #{session_id} &middot; {format_duration(time_range)} window &middot; {format_duration(int(bucket_size))}/col</span>',
        '</div>',
        '<div class="heat-legend">',
        '<span>Duration:</span>',
    ]

    # Legend swatches
    for label, color in [("cold", "#1a2a3a"), ("cool", "#1a5e6e"), ("warm", "#cc8822"), ("hot", "#cc2222")]:
        parts.append(f'<div class="heat-legend-swatch" style="background:{color}"></div><span>{label}</span>')
    parts.append('</div>')

    parts.append('<div class="heatmap-wrap"><table class="heatmap"><thead><tr>')
    parts.append('<th class="lotus-row"></th>')
    for col_letter in col_letters:
        parts.append(f'<th class="lotus-col">{col_letter}</th>')
    parts.append('</tr></thead><tbody>')

    for row_idx, name in enumerate(top_names, 1):
        parts.append(f'<tr><td class="lotus-row">{row_idx}</td>')
        for col_idx in range(n_cols):
            col_data = grid[name][col_idx]
            col_letter = col_letters[col_idx]
            cell_ref = f"{col_letter}{row_idx}: {h(name)}"

            if col_data:
                avg = sum(col_data) / len(col_data)
                intensity = min(avg / max_avg, 1.0)

                # Color gradient: cold blue → teal → amber → red
                if intensity < 0.25:
                    r, g, b = 26, 42 + int(intensity * 4 * 50), 58 + int(intensity * 4 * 50)
                elif intensity < 0.5:
                    t = (intensity - 0.25) * 4
                    r, g, b = int(26 + t * 170), int(92 + t * 44), int(108 - t * 74)
                elif intensity < 0.75:
                    t = (intensity - 0.5) * 4
                    r, g, b = int(196 + t * 8), int(136 - t * 102), int(34 - t * 34)
                else:
                    t = (intensity - 0.75) * 4
                    r, g, b = int(204), int(34 - t * 34), int(0)

                val = format_duration(avg)
                count = len(col_data)
                parts.append(
                    f'<td style="background:rgb({r},{g},{b});color:{"#fff" if intensity > 0.3 else "#aaa"}"'
                    f' title="{cell_ref} = {val} (n={count})">{val}</td>'
                )
            else:
                parts.append(f'<td title="{cell_ref} = (empty)" style="color:var(--border)">&middot;</td>')

        parts.append('</tr>')

    parts.append('</tbody></table></div>')
    return "\n".join(parts)


def render_sparkline(db, session_id):
    """Lotus line chart / Harvard Graphics trend — frame timing sparkline."""
    if not session_id:
        return '<div class="empty-state"><p>Select a session first.</p></div>'

    # Get top-level spans (no parent) to show frame-level timing
    cur = db.execute(
        """SELECT name, start_us, (end_us - start_us) as dur
           FROM spans WHERE session_id=? AND parent_id IS NULL
           ORDER BY start_us ASC LIMIT 500""",
        (session_id,),
    )
    spans = cur.fetchall()
    if not spans:
        return '<div class="empty-state"><p>No root spans recorded.</p></div>'

    # Group by span name
    by_name = {}
    for name, start, dur in spans:
        by_name.setdefault(name, []).append((start, dur))

    parts = [
        '<div class="view-header hg-ruled">',
        f'<h2>Frame Trend</h2>',
        f'<span class="count">Lotus 1-2-3 Line Chart &middot; {len(spans):,} data points</span>',
        '</div>',
        '<div class="sparkline-section">',
    ]

    threshold_us = 16000  # 16ms frame budget

    for series_idx, (name, data) in enumerate(by_name.items()):
        durs = [d for _, d in data]
        max_dur = max(durs) if durs else 1
        min_dur = min(durs) if durs else 0
        avg_dur = sum(durs) / len(durs) if durs else 0
        over_budget = sum(1 for d in durs if d > threshold_us)

        color = SPAN_COLORS[series_idx % len(SPAN_COLORS)]

        parts.append(f'<div class="sparkline-title">{h(name)}'
                     f'<span class="sparkline-subtitle">avg {format_duration(avg_dur)} &middot; '
                     f'min {format_duration(min_dur)} &middot; max {format_duration(max_dur)}'
                     f'{f" &middot; {over_budget} over budget" if over_budget else ""}'
                     f'</span></div>')

        parts.append('<div class="sparkline-chart" style="margin-left:48px">')
        parts.append('<div class="sparkline-grid"></div>')

        # Y-axis labels
        parts.append('<div class="sparkline-y-axis">')
        for i in range(5, -1, -1):
            val = max_dur * i / 5
            parts.append(f'<span>{format_duration(val)}</span>')
        parts.append('</div>')

        # 16ms threshold line
        if max_dur > 0:
            threshold_pct = min(threshold_us / max_dur * 100, 100)
            parts.append(
                f'<div class="sparkline-threshold" style="bottom:{threshold_pct:.0f}%">16ms</div>'
            )

        # Bars
        for dur in durs:
            pct = (dur / max_dur * 100) if max_dur > 0 else 0
            bar_color = "#cc2222" if dur > threshold_us else color
            ms_val = f"{dur/1000:.1f}ms"
            parts.append(
                f'<div class="sparkline-bar" style="height:{max(pct, 0.5):.1f}%;background:{bar_color}"'
                f' data-val="{ms_val}"></div>'
            )

        parts.append('</div>')  # sparkline-chart

    parts.append('</div>')
    return "\n".join(parts)


def render_pie(db, session_id):
    """Harvard Graphics pie chart — time breakdown by span name."""
    if not session_id:
        return '<div class="empty-state"><p>Select a session first.</p></div>'

    cur = db.execute(
        "SELECT name, SUM(end_us - start_us) as total_dur, COUNT(*) as cnt FROM spans WHERE session_id=? GROUP BY name ORDER BY total_dur DESC",
        (session_id,),
    )
    rows = cur.fetchall()
    if not rows:
        return '<div class="empty-state"><p>No spans recorded.</p></div>'

    grand_total = sum(dur for _, dur, _ in rows)
    if grand_total == 0:
        return '<div class="empty-state"><p>All spans have zero duration.</p></div>'

    # Top 8 + "Other"
    top = rows[:8]
    other_dur = sum(dur for _, dur, _ in rows[8:])
    other_cnt = sum(cnt for _, _, cnt in rows[8:])

    slices = [(name, dur, cnt) for name, dur, cnt in top]
    if other_dur > 0:
        slices.append(("Other", other_dur, other_cnt))

    # Build conic-gradient
    gradient_parts = []
    cum = 0.0
    for i, (name, dur, cnt) in enumerate(slices):
        pct = dur / grand_total * 100
        color = SPAN_COLORS[i % len(SPAN_COLORS)]
        gradient_parts.append(f"{color} {cum:.1f}% {cum + pct:.1f}%")
        cum += pct

    conic = ", ".join(gradient_parts)

    parts = [
        '<div class="view-header hg-ruled">',
        f'<h2>Time Distribution</h2>',
        f'<span class="count">Harvard Graphics Pie Chart &middot; {format_duration(grand_total)} total</span>',
        '</div>',
        '<div class="pie-section">',
        '<div class="hg-frame">',
        '<div class="hg-frame-title">Total Span Time</div>',
        '<div class="pie-chart-wrap">',
        f'<div class="pie-chart" style="background:conic-gradient({conic})"></div>',
        '</div></div>',
        '<div class="hg-frame" style="min-width:280px">',
        '<div class="hg-frame-title">Legend</div>',
        '<div class="pie-legend">',
    ]

    for i, (name, dur, cnt) in enumerate(slices):
        pct = dur / grand_total * 100
        color = SPAN_COLORS[i % len(SPAN_COLORS)]
        parts.append(
            f'<div class="pie-legend-item">'
            f'<div class="pie-legend-swatch" style="background:{color}"></div>'
            f'<span class="pie-legend-label">{h(name)}</span>'
            f'<span class="pie-legend-pct">{pct:.1f}% ({cnt:,}x)</span>'
            f'</div>'
        )

    parts.append('</div></div></div>')

    # Second row: stacked summary as Harvard Graphics bullet chart
    parts.append('<div style="padding:0 16px 16px"><div class="hg-frame">')
    parts.append('<div class="hg-frame-title">Proportional Breakdown</div>')

    # Stacked horizontal bar
    parts.append('<div style="display:flex;height:28px;border:1px solid var(--border);margin:8px 0">')
    for i, (name, dur, cnt) in enumerate(slices):
        pct = dur / grand_total * 100
        color = SPAN_COLORS[i % len(SPAN_COLORS)]
        parts.append(
            f'<div style="width:{pct:.1f}%;background:{color};position:relative;overflow:hidden" title="{h(name)}: {pct:.1f}%">'
            f'<div style="position:absolute;inset:0;background:linear-gradient(180deg,rgba(255,255,255,0.15) 0%,transparent 40%,rgba(0,0,0,0.15) 100%)"></div>'
            f'</div>'
        )
    parts.append('</div>')

    # Axis labels below
    parts.append('<div style="display:flex;font-size:9px;color:var(--text-dim)">')
    for pct_mark in [0, 25, 50, 75, 100]:
        parts.append(f'<span style="flex:1;text-align:{"left" if pct_mark == 0 else "center" if pct_mark < 100 else "right"}">{pct_mark}%</span>')
    parts.append('</div>')

    parts.append('</div></div>')
    return "\n".join(parts)


def render_waterfall(db, session_id):
    """Harvard Graphics waterfall/cascade chart — span start/end as a Gantt-like view."""
    if not session_id:
        return '<div class="empty-state"><p>Select a session first.</p></div>'

    # Get a window of spans showing concurrency and nesting
    cur = db.execute(
        """SELECT id, parent_id, name, start_us, end_us, (end_us - start_us) as dur
           FROM spans WHERE session_id=?
           ORDER BY start_us ASC, dur DESC LIMIT 200""",
        (session_id,),
    )
    spans = cur.fetchall()
    if not spans:
        return '<div class="empty-state"><p>No spans recorded.</p></div>'

    # Find the time window
    min_start = min(s[3] for s in spans)
    max_end = max(s[4] for s in spans)
    time_range = max(max_end - min_start, 1)

    parts = [
        '<div class="view-header hg-ruled">',
        f'<h2>Waterfall</h2>',
        f'<span class="count">Harvard Graphics Cascade &middot; {len(spans):,} spans &middot; {format_duration(time_range)} window</span>',
        '</div>',
        '<div class="waterfall-wrap">',
    ]

    # Compute nesting depth for each span
    parent_map = {s[0]: s[1] for s in spans}
    id_set = {s[0] for s in spans}

    def get_depth(span_id):
        depth = 0
        pid = parent_map.get(span_id)
        while pid and pid in id_set and depth < 10:
            depth += 1
            pid = parent_map.get(pid)
        return depth

    for i, (sid, pid, name, start, end, dur) in enumerate(spans):
        left_pct = (start - min_start) / time_range * 100
        width_pct = max(dur / time_range * 100, 0.3)
        depth = get_depth(sid)
        color = SPAN_COLORS[depth % len(SPAN_COLORS)]

        indent = depth * 12
        label = ("  " * depth) + ("|- " if depth > 0 else "") + name

        dur_str = format_duration(dur)

        parts.append(
            f'<div class="waterfall-row">'
            f'<div class="waterfall-label" style="padding-left:{indent}px" title="{h(name)}">'
            f'{"&#9500; " if depth > 0 else ""}{h(name)}</div>'
            f'<div class="waterfall-track">'
            f'<div class="waterfall-bar" style="left:{left_pct:.2f}%;width:{width_pct:.2f}%;background:{color}">'
            f'<span class="waterfall-dur">{dur_str}</span></div>'
            f'</div></div>'
        )

    # Time axis
    parts.append('<div class="waterfall-axis">')
    for i in range(6):
        t = min_start + time_range * i / 5
        parts.append(f'<span>{format_duration(int(t - min_start))}</span>')
    parts.append('</div>')

    parts.append('</div>')
    return "\n".join(parts)


# ── Helpers ──────────────────────────────────────────────────────────

def format_duration(us):
    """Format microseconds into human-readable duration."""
    if us >= 1_000_000:
        return f"{us / 1_000_000:.2f}s"
    elif us >= 1000:
        return f"{us / 1000:.1f}ms"
    else:
        return f"{us:.0f}\u00b5s"


def shorten_target(target):
    """Shorten a Rust target path for display."""
    parts = target.split("::")
    if len(parts) > 2:
        return "::".join(parts[-2:])
    return target


def get_db_info(db_path):
    """Get DB status info for the toolbar."""
    try:
        size = os.path.getsize(db_path)
        if size > 1_048_576:
            return f"traces.db: {size / 1_048_576:.1f} MB"
        return f"traces.db: {size / 1024:.0f} KB"
    except OSError:
        return "traces.db: not found"


def seed_demo_data(db_path):
    """Create demo trace data if the DB is empty or missing."""
    conn = sqlite3.connect(db_path)
    conn.execute("PRAGMA journal_mode=WAL")

    # Check if schema exists
    tables = conn.execute(
        "SELECT name FROM sqlite_master WHERE type='table' AND name='sessions'"
    ).fetchone()

    if not tables:
        conn.executescript("""
            CREATE TABLE IF NOT EXISTS sessions (
                id INTEGER PRIMARY KEY, pid INTEGER NOT NULL,
                component TEXT NOT NULL, started_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS spans (
                id INTEGER PRIMARY KEY,
                session_id INTEGER NOT NULL REFERENCES sessions(id),
                parent_id INTEGER, name TEXT NOT NULL, target TEXT NOT NULL,
                level TEXT NOT NULL, start_us INTEGER NOT NULL,
                end_us INTEGER NOT NULL, fields TEXT
            );
            CREATE TABLE IF NOT EXISTS events (
                id INTEGER PRIMARY KEY,
                session_id INTEGER NOT NULL REFERENCES sessions(id),
                span_id INTEGER, level TEXT NOT NULL, target TEXT NOT NULL,
                message TEXT NOT NULL, timestamp_us INTEGER NOT NULL, fields TEXT
            );
        """)

    count = conn.execute("SELECT COUNT(*) FROM sessions").fetchone()[0]
    if count > 0:
        conn.close()
        return

    import time, random

    now = int(time.time())

    # Demo session: panel
    conn.execute(
        "INSERT INTO sessions (pid, component, started_at) VALUES (?, ?, ?)",
        (12847, "panel", now - 300),
    )
    sid = conn.execute("SELECT last_insert_rowid()").fetchone()[0]

    # Demo session: daemon
    conn.execute(
        "INSERT INTO sessions (pid, component, started_at) VALUES (?, ?, ?)",
        (9012, "daemon", now - 600),
    )
    dsid = conn.execute("SELECT last_insert_rowid()").fetchone()[0]

    random.seed(42)

    # Generate realistic panel spans (render frames)
    t = 0
    for frame in range(200):
        frame_dur = random.randint(8000, 18000)  # 8-18ms frames
        frame_start = t
        frame_end = t + frame_dur

        conn.execute(
            "INSERT INTO spans (session_id, name, target, level, start_us, end_us) VALUES (?,?,?,?,?,?)",
            (sid, "render_frame", "hotbar_panel::sctk_shell", "TRACE", frame_start, frame_end),
        )
        fid = conn.execute("SELECT last_insert_rowid()").fetchone()[0]

        # Sub-spans
        sub_t = frame_start
        sub_spans = [
            ("reveal_update", 100, 400),
            ("egui_run", 3000, 7000),
            ("egui_tessellate", 800, 2000),
            ("gpu_before_egui", 2000, 5000),
            ("egui_render", 1000, 2500),
            ("present", 200, 600),
        ]

        for name, lo, hi in sub_spans:
            dur = random.randint(lo, hi)
            conn.execute(
                "INSERT INTO spans (session_id, parent_id, name, target, level, start_us, end_us) VALUES (?,?,?,?,?,?,?)",
                (sid, fid, name, "hotbar_panel::sctk_shell", "TRACE", sub_t, sub_t + dur),
            )
            sub_id = conn.execute("SELECT last_insert_rowid()").fetchone()[0]

            # GPU sub-sub-spans
            if name == "gpu_before_egui":
                gpu_t = sub_t
                for gname, glo, ghi in [("chrome_pass", 200, 600), ("heat_glow_pass", 400, 1200), ("flames_pass", 300, 1500)]:
                    gdur = random.randint(glo, ghi)
                    fields = f"particles={random.randint(200,512)}" if "flames" in gname else None
                    conn.execute(
                        "INSERT INTO spans (session_id, parent_id, name, target, level, start_us, end_us, fields) VALUES (?,?,?,?,?,?,?,?)",
                        (sid, sub_id, gname, "hotbar_panel::gpu", "TRACE", gpu_t, gpu_t + gdur, fields),
                    )
                    gpu_t += gdur

            sub_t += dur

        t += frame_dur + random.randint(500, 2000)

    # Frame budget warnings
    for i in range(8):
        ts = random.randint(0, t)
        ms = round(random.uniform(16.1, 24.0), 1)
        conn.execute(
            "INSERT INTO events (session_id, level, target, message, timestamp_us, fields) VALUES (?,?,?,?,?,?)",
            (sid, "WARN", "hotbar_panel::sctk_shell", "frame budget exceeded (>16ms)", ts, f'frame_ms={ms}'),
        )

    # Daemon spans and events
    dt = 0
    for _ in range(50):
        dur = random.randint(500, 5000)
        conn.execute(
            "INSERT INTO spans (session_id, name, target, level, start_us, end_us, fields) VALUES (?,?,?,?,?,?,?)",
            (dsid, "claude_ingest", "hotbar_daemon::ingest::claude", "DEBUG", dt, dt + dur, None),
        )
        dt += dur + random.randint(100000, 500000)

        conn.execute(
            "INSERT INTO events (session_id, level, target, message, timestamp_us, fields) VALUES (?,?,?,?,?,?)",
            (dsid, "DEBUG", "hotbar_daemon::db", "db insert events", dt, f"batch_size={random.randint(1,20)}"),
        )

    for _ in range(20):
        dur = random.randint(200, 2000)
        dt2 = random.randint(0, dt)
        conn.execute(
            "INSERT INTO spans (session_id, name, target, level, start_us, end_us) VALUES (?,?,?,?,?,?)",
            (dsid, "fts5_search", "hotbar_daemon::search", "DEBUG", dt2, dt2 + dur),
        )
        conn.execute(
            "INSERT INTO events (session_id, level, target, message, timestamp_us, fields) VALUES (?,?,?,?,?,?)",
            (dsid, "DEBUG", "hotbar_daemon::search", "search dispatched", dt2, f'query="main", limit=50'),
        )

    # Info events
    conn.execute(
        "INSERT INTO events (session_id, level, target, message, timestamp_us) VALUES (?,?,?,?,?)",
        (dsid, "INFO", "hotbar_daemon::db", "database opened", 0),
    )
    conn.execute(
        "INSERT INTO events (session_id, level, target, message, timestamp_us, fields) VALUES (?,?,?,?,?,?)",
        (dsid, "INFO", "hotbar_daemon::state", "state hydrated from database", 1000, "files=47, pins=3"),
    )
    conn.execute(
        "INSERT INTO events (session_id, level, target, message, timestamp_us) VALUES (?,?,?,?,?)",
        (dsid, "INFO", "hotbar_daemon::ipc", "IPC server listening", 2000),
    )
    conn.execute(
        "INSERT INTO events (session_id, level, target, message, timestamp_us, fields) VALUES (?,?,?,?,?,?)",
        (dsid, "INFO", "hotbar_daemon::search", "search index rebuilt", 3000, "indexed=47"),
    )

    conn.commit()
    conn.close()
    print(f"  Seeded demo data into {db_path}")


# ── HTTP Server ──────────────────────────────────────────────────────

db_path_global = DEFAULT_DB


class TraceHandler(http.server.BaseHTTPRequestHandler):
    """HTTP handler serving the trace viewer UI and htmx partials."""

    def log_message(self, format, *args):
        # Compact logging
        sys.stderr.write(f"  {args[0]}\n")

    def do_GET(self):
        parsed = urlparse(self.path)
        path = parsed.path
        params = parse_qs(parsed.query)

        if path == "/":
            self.serve_index()
        elif path == "/htmx/sessions":
            self.serve_htmx(render_sessions)
        elif path == "/htmx/timeline":
            sid = params.get("session_id", [None])[0]
            self.serve_htmx(lambda db: render_timeline(db, sid))
        elif path == "/htmx/events":
            sid = params.get("session_id", [None])[0]
            level = params.get("level", ["ALL"])[0]
            self.serve_htmx(lambda db: render_events(db, sid, level))
        elif path == "/htmx/performance":
            sid = params.get("session_id", [None])[0]
            self.serve_htmx(lambda db: render_performance(db, sid))
        elif path == "/htmx/top-spans":
            sid = params.get("session_id", [None])[0]
            self.serve_htmx(lambda db: render_top_spans(db, sid))
        elif path == "/htmx/heatmap":
            sid = params.get("session_id", [None])[0]
            self.serve_htmx(lambda db: render_heatmap(db, sid))
        elif path == "/htmx/sparkline":
            sid = params.get("session_id", [None])[0]
            self.serve_htmx(lambda db: render_sparkline(db, sid))
        elif path == "/htmx/pie":
            sid = params.get("session_id", [None])[0]
            self.serve_htmx(lambda db: render_pie(db, sid))
        elif path == "/htmx/waterfall":
            sid = params.get("session_id", [None])[0]
            self.serve_htmx(lambda db: render_waterfall(db, sid))
        else:
            self.send_error(404)

    def serve_index(self):
        db_info = get_db_info(db_path_global)
        db_status = f"traces.db: {db_path_global}"

        html = HTML_PAGE.replace("__DB_INFO__", db_info)
        html = html.replace("__DB_PATH__", db_path_global)
        html = html.replace("__DB_STATUS__", db_status)

        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.end_headers()
        self.wfile.write(html.encode())

    def serve_htmx(self, renderer):
        try:
            db = sqlite3.connect(db_path_global)
            db.execute("PRAGMA journal_mode=WAL")
            html = renderer(db)
            db.close()
        except Exception as e:
            html = f'<div class="empty-state"><p>Error: {h(str(e))}</p></div>'

        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.end_headers()
        self.wfile.write(html.encode())


# ── Main ─────────────────────────────────────────────────────────────

def main():
    global db_path_global

    port = DEFAULT_PORT
    db_path_global = DEFAULT_DB

    args = sys.argv[1:]
    i = 0
    while i < len(args):
        if args[i] == "--port" and i + 1 < len(args):
            port = int(args[i + 1])
            i += 2
        elif args[i] == "--db" and i + 1 < len(args):
            db_path_global = os.path.expanduser(args[i + 1])
            i += 2
        elif args[i] in ("-h", "--help"):
            print(__doc__)
            sys.exit(0)
        else:
            print(f"Unknown argument: {args[i]}")
            sys.exit(1)

    # Ensure DB directory exists
    db_dir = os.path.dirname(db_path_global)
    if db_dir and not os.path.exists(db_dir):
        os.makedirs(db_dir, exist_ok=True)

    # Seed demo data if DB is empty
    seed_demo_data(db_path_global)

    db_info = get_db_info(db_path_global)

    print()
    print("  \033[38;5;124m\u25c6\033[0m DeltaGraph Trace Viewer")
    print(f"  \033[38;5;30mH O T B A R\033[0m  v2.0")
    print()
    print(f"  Server:  http://localhost:{port}")
    print(f"  DB:      {db_path_global}")
    print(f"  Size:    {db_info}")
    print()

    server = http.server.HTTPServer(("127.0.0.1", port), TraceHandler)

    # Open browser
    webbrowser.open(f"http://localhost:{port}")

    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\n  Shutting down.")
        server.shutdown()


if __name__ == "__main__":
    main()
