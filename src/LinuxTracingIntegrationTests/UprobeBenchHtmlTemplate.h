// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Generated from uprobe_attach_detach_bench.html — do not clang-format.
#ifndef LINUX_TRACING_INTEGRATION_TESTS_UPROBE_BENCH_HTML_TEMPLATE_H_
#define LINUX_TRACING_INTEGRATION_TESTS_UPROBE_BENCH_HTML_TEMPLATE_H_

#include <string_view>

inline constexpr std::string_view kUprobeBenchHtmlTemplate = R"UPROBEHTML(<!DOCTYPE html>
<!-- If you edit this file, regenerate UprobeBenchHtmlTemplate.h from it. -->
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Orbit uprobe stop: why it was slow, what we changed</title>
<style>
  :root {
    --bg: #f4f1ea;
    --card: #fffdf8;
    --ink: #1c2329;
    --muted: #5a6570;
    --line: #d7d0c4;
    --old: #8f2d3a;
    --old-soft: #f6e4e6;
    --new: #1f6b4a;
    --new-soft: #e3f2ea;
    --lock: #9a3412;
    --chip: #e8eef6;
    --mono: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    --sans: "Iowan Old Style", "Palatino Linotype", Palatino, "Book Antiqua", Georgia, serif;
    --ui: ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif;
  }
  * { box-sizing: border-box; }
  body {
    margin: 0;
    color: var(--ink);
    background: var(--bg);
    font: 17px/1.5 var(--sans);
  }
  main {
    max-width: 880px;
    margin: 0 auto;
    padding: 32px 20px 64px;
  }
  h1, h2, h3 { font-family: var(--ui); line-height: 1.25; }
  h1 { font-size: 1.85rem; margin: 0 0 8px; }
  h2 { font-size: 1.25rem; margin: 36px 0 12px; }
  h3 { font-size: 1.05rem; margin: 20px 0 8px; }
  p, li { max-width: 72ch; }
  .lede { color: var(--muted); font-size: 1.05rem; margin: 0 0 24px; }
  .card {
    background: var(--card);
    border: 1px solid var(--line);
    border-radius: 10px;
    padding: 16px 18px 12px;
    margin: 16px 0 24px;
  }
  .formula {
    font-family: var(--mono);
    font-size: 0.92rem;
    background: #efeae0;
    border-radius: 6px;
    padding: 8px 12px;
    display: inline-block;
  }
  code, .cmd {
    font-family: var(--mono);
    font-size: 0.88em;
    background: #efeae0;
    padding: 1px 5px;
    border-radius: 4px;
  }
  pre.cmd {
    display: block;
    white-space: pre-wrap;
    padding: 12px 14px;
    margin: 8px 0 0;
    background: #2b3036;
    color: #f4f1ea;
    border-radius: 8px;
  }
  pre.cmd code { background: none; color: inherit; padding: 0; }
  table {
    width: 100%;
    border-collapse: collapse;
    font-family: var(--ui);
    font-size: 0.92rem;
  }
  th, td {
    text-align: right;
    padding: 7px 8px;
    border-bottom: 1px solid var(--line);
    white-space: nowrap;
  }
  th:first-child, td:first-child { text-align: left; }
  th { font-weight: 650; color: var(--muted); font-size: 0.78rem; letter-spacing: 0.02em; }
  .na { color: var(--muted); font-style: italic; }
  .verdict {
    font-family: var(--ui);
    font-weight: 700;
    display: inline-block;
    padding: 4px 10px;
    border-radius: 999px;
    margin-right: 8px;
  }
  .verdict.pass { background: var(--new-soft); color: var(--new); }
  .verdict.fail { background: var(--old-soft); color: var(--old); }
  .verdict.na { background: #eee; color: var(--muted); }
  .note { color: var(--muted); font-size: 0.92rem; }
  .svg-wrap { overflow-x: auto; margin: 8px 0 4px; }
  svg { display: block; max-width: 100%; height: auto; }
  footer { margin-top: 40px; color: var(--muted); font-size: 0.9rem; }
  ol.tight > li { margin: 6px 0; }
</style>
</head>
<body>
<main>
<h1>Why Orbit’s kernel-uprobe stop was slow</h1>
<p class="lede">Self-contained report: the kernel lock, why fewer process-wide fds are not the production fix, the shared-tracefs change, and the 10/20/50 attach/detach numbers from this run.</p>

<h2>1. The issue</h2>
<p>Orbit’s historical kernel-uprobe path opens <strong>one uprobe and one uretprobe PMU fd per CPU per function</strong> (<code>pid=-1</code>, <code>cpu=0..N-1</code>):</p>
<p class="formula">fds = 2 × F × NCPU</p>
<p>Each of those fds is its own <code>create_local_trace_uprobe</code> — not another consumer of one probe. Closing a capture therefore destroys <em>F × NCPU × 2</em> independent probes.</p>
<p><code>perf_uprobe_destroy</code> holds the <strong>global <code>event_mutex</code></strong> around the <em>entire</em> close:</p>
<ol class="tight">
  <li><code>uprobe_apply(false)</code></li>
  <li><code>uprobe_unregister_nosync</code> — VMA walk under <code>dup_mmap_sem</code> + <code>register_rwsem</code></li>
  <li><code>uprobe_unregister_sync()</code> — RCU tasks-trace <em>and</em> uretprobes SRCU</li>
  <li><code>tracepoint_synchronize_unregister</code></li>
</ol>
<p>Parallel <code>close()</code> cannot overlap those grace periods; they queue on <code>event_mutex</code>. Wall time stays</p>
<p class="formula">O(F × NCPU × 2 × (VMA walk + 2–3 RCU GPs))</p>
<p>Stop time growing with core count is expected from the per-CPU fd model. <strong>ParallelFor does not help</strong>: it issues more closes at once, but they still serialize on the same mutex.</p>

<div class="card">
<h3>Old path — every sample fd is a local probe</h3>
<p class="note">Two functions × three CPUs shown. Each chip is a PMU fd. Every close takes <code>event_mutex</code>.</p>
<div class="svg-wrap">
<svg viewBox="0 0 840 340" width="840" height="340" role="img" aria-labelledby="old-title old-desc">
  <title id="old-title">Historical per-CPU uprobe PMU fds all serialize on event_mutex</title>
  <desc id="old-desc">For each function and CPU, an uprobe fd and a uretprobe fd each perform create_local_trace_uprobe. All closes go through one global event_mutex.</desc>
  <rect x="0" y="0" width="840" height="340" fill="#fffdf8"/>
  <text x="16" y="28" font-family="ui-sans-serif,system-ui,sans-serif" font-size="14" font-weight="700" fill="#1c2329">F functions × N CPUs × 2 PMU fds → event_mutex</text>
  <g font-family="ui-sans-serif,system-ui,sans-serif" font-size="12">
    <text x="40" y="64" fill="#5a6570">function 0</text>
    <text x="300" y="64" fill="#5a6570">function 1</text>
    <text x="560" y="64" fill="#5a6570">… function F-1</text>
  </g>
  <!-- function 0 fds -->
  <g font-family="ui-sans-serif,system-ui,sans-serif" font-size="11">
    <rect x="24" y="76" rx="5" width="86" height="28" fill="#f6e4e6" stroke="#8f2d3a"/>
    <text x="36" y="94" fill="#8f2d3a">u cpu0</text>
    <rect x="118" y="76" rx="5" width="86" height="28" fill="#f6e4e6" stroke="#8f2d3a"/>
    <text x="128" y="94" fill="#8f2d3a">ur cpu0</text>
    <rect x="24" y="112" rx="5" width="86" height="28" fill="#f6e4e6" stroke="#8f2d3a"/>
    <text x="36" y="130" fill="#8f2d3a">u cpu1</text>
    <rect x="118" y="112" rx="5" width="86" height="28" fill="#f6e4e6" stroke="#8f2d3a"/>
    <text x="128" y="130" fill="#8f2d3a">ur cpu1</text>
    <rect x="24" y="148" rx="5" width="86" height="28" fill="#f6e4e6" stroke="#8f2d3a"/>
    <text x="36" y="166" fill="#8f2d3a">u cpu2</text>
    <rect x="118" y="148" rx="5" width="86" height="28" fill="#f6e4e6" stroke="#8f2d3a"/>
    <text x="128" y="166" fill="#8f2d3a">ur cpu2</text>
  </g>
  <!-- function 1 fds -->
  <g font-family="ui-sans-serif,system-ui,sans-serif" font-size="11">
    <rect x="284" y="76" rx="5" width="86" height="28" fill="#f6e4e6" stroke="#8f2d3a"/>
    <text x="296" y="94" fill="#8f2d3a">u cpu0</text>
    <rect x="378" y="76" rx="5" width="86" height="28" fill="#f6e4e6" stroke="#8f2d3a"/>
    <text x="388" y="94" fill="#8f2d3a">ur cpu0</text>
    <rect x="284" y="112" rx="5" width="86" height="28" fill="#f6e4e6" stroke="#8f2d3a"/>
    <text x="296" y="130" fill="#8f2d3a">u cpu1</text>
    <rect x="378" y="112" rx="5" width="86" height="28" fill="#f6e4e6" stroke="#8f2d3a"/>
    <text x="388" y="130" fill="#8f2d3a">ur cpu1</text>
    <rect x="284" y="148" rx="5" width="86" height="28" fill="#f6e4e6" stroke="#8f2d3a"/>
    <text x="296" y="166" fill="#8f2d3a">u cpu2</text>
    <rect x="378" y="148" rx="5" width="86" height="28" fill="#f6e4e6" stroke="#8f2d3a"/>
    <text x="388" y="166" fill="#8f2d3a">ur cpu2</text>
  </g>
  <g font-family="ui-sans-serif,system-ui,sans-serif" font-size="11" fill="#5a6570">
    <rect x="544" y="76" rx="5" width="86" height="28" fill="#eee8df" stroke="#b5a896"/>
    <text x="560" y="94">…</text>
    <rect x="638" y="76" rx="5" width="86" height="28" fill="#eee8df" stroke="#b5a896"/>
    <text x="654" y="94">…</text>
    <rect x="544" y="112" rx="5" width="86" height="28" fill="#eee8df" stroke="#b5a896"/>
    <text x="560" y="130">…</text>
    <rect x="638" y="112" rx="5" width="86" height="28" fill="#eee8df" stroke="#b5a896"/>
    <text x="654" y="130">…</text>
    <rect x="544" y="148" rx="5" width="86" height="28" fill="#eee8df" stroke="#b5a896"/>
    <text x="560" y="166">…</text>
    <rect x="638" y="148" rx="5" width="86" height="28" fill="#eee8df" stroke="#b5a896"/>
    <text x="654" y="166">…</text>
  </g>
  <!-- lines to mutex -->
  <g stroke="#8f2d3a" stroke-width="1.2" opacity="0.55">
    <line x1="67" y1="176" x2="420" y2="232"/>
    <line x1="161" y1="176" x2="420" y2="232"/>
    <line x1="327" y1="176" x2="420" y2="232"/>
    <line x1="421" y1="176" x2="420" y2="232"/>
    <line x1="587" y1="176" x2="420" y2="232"/>
    <line x1="681" y1="176" x2="420" y2="232"/>
  </g>
  <rect x="268" y="228" rx="8" width="304" height="52" fill="#9a3412"/>
  <text x="420" y="250" text-anchor="middle" font-family="ui-sans-serif,system-ui,sans-serif" font-size="14" font-weight="700" fill="#fff">global event_mutex</text>
  <text x="420" y="270" text-anchor="middle" font-family="ui-sans-serif,system-ui,sans-serif" font-size="11" fill="#f6e4e6">one close at a time — ParallelFor still queues here</text>
  <text x="16" y="316" font-family="ui-sans-serif,system-ui,sans-serif" font-size="13" fill="#5a6570">Each fd = create_local_trace_uprobe. Close holds the mutex for a VMA walk + 2–3 RCU grace periods.</text>
</svg>
</div>
</div>

<h2>2. Why <code>pid=target, cpu=-1</code> is not the production fix</h2>
<p>Opening one uprobe + one uretprobe for the process (<code>2×F</code> fds) was measured. It is <strong>not</strong> how production samples:</p>
<ul>
  <li><code>perf_event_open</code> <code>pid</code> is a <strong>tid</strong>. Samples follow that task only.</li>
  <li><code>inherit</code> covers children created <em>after</em> open, not existing sibling threads.</li>
  <li><code>inherit</code> + <code>cpu=-1</code> <strong>cannot mmap</strong> a sample ring buffer (<code>perf_event_open(2)</code>).</li>
</ul>
<p>Production keeps <code>pid=-1, cpu=N</code> so every thread on every core is sampled. The bench still times the process-wide experiment; it is a side table, not the fix.</p>

<h2>3. The solution</h2>
<p>Register each probe <strong>once</strong> via tracefs (<code>uprobe_events</code>, group <code>orbit</code>) and open the per-CPU fds as <strong>TRACEPOINT</strong> consumers of that named probe.</p>
<ul>
  <li>Sample fds (coverage): still <code>2 × F × NCPU</code></li>
  <li>Named probes / expensive unregisters: <code>2 × F</code> (last close of each name)</li>
  <li><code>Shutdown()</code> disables, unmaps, <strong>closes every fd</strong>, then deletes the tracefs names. No leaked probes, no fire-and-forget.</li>
</ul>

<div class="card">
<h3>New path — many sample fds share two named probes per function</h3>
<p class="note">TRACEPOINT fds still deliver per-CPU samples. Only the named probes pay the unregister cost.</p>
<div class="svg-wrap">
<svg viewBox="0 0 840 320" width="840" height="320" role="img" aria-labelledby="new-title new-desc">
  <title id="new-title">Shared tracefs registration with per-CPU TRACEPOINT consumers</title>
  <desc id="new-desc">Each function has one named uprobe and one named uretprobe. Many per-CPU TRACEPOINT fds share those two registrations. Expensive unregister is 2 times F.</desc>
  <rect x="0" y="0" width="840" height="320" fill="#fffdf8"/>
  <text x="16" y="28" font-family="ui-sans-serif,system-ui,sans-serif" font-size="14" font-weight="700" fill="#1c2329">2 named probes per function; per-CPU fds are TRACEPOINT consumers</text>
  <g font-family="ui-sans-serif,system-ui,sans-serif" font-size="12" fill="#5a6570">
    <text x="48" y="60">function 0</text>
    <text x="328" y="60">function 1</text>
    <text x="608" y="60">… function F-1</text>
  </g>
  <g font-family="ui-sans-serif,system-ui,sans-serif" font-size="12" font-weight="650">
    <rect x="20" y="72" rx="6" width="118" height="36" fill="#e3f2ea" stroke="#1f6b4a"/>
    <text x="34" y="95" fill="#1f6b4a">orbit:u_0</text>
    <rect x="148" y="72" rx="6" width="118" height="36" fill="#e3f2ea" stroke="#1f6b4a"/>
    <text x="158" y="95" fill="#1f6b4a">orbit:ur_0</text>
    <rect x="300" y="72" rx="6" width="118" height="36" fill="#e3f2ea" stroke="#1f6b4a"/>
    <text x="314" y="95" fill="#1f6b4a">orbit:u_1</text>
    <rect x="428" y="72" rx="6" width="118" height="36" fill="#e3f2ea" stroke="#1f6b4a"/>
    <text x="438" y="95" fill="#1f6b4a">orbit:ur_1</text>
    <rect x="580" y="72" rx="6" width="118" height="36" fill="#e8efe4" stroke="#7aa08c"/>
    <text x="610" y="95" fill="#5a6570">orbit:u_…</text>
    <rect x="708" y="72" rx="6" width="118" height="36" fill="#e8efe4" stroke="#7aa08c"/>
    <text x="732" y="95" fill="#5a6570">orbit:ur_…</text>
  </g>
  <g font-family="ui-sans-serif,system-ui,sans-serif" font-size="10" fill="#3d4a55">
    <text x="20" y="132">TRACEPOINT fds (cpu0..N-1)</text>
    <rect x="20" y="140" rx="4" width="52" height="22" fill="#e8eef6" stroke="#6b7c90"/><text x="30" y="155">c0</text>
    <rect x="76" y="140" rx="4" width="52" height="22" fill="#e8eef6" stroke="#6b7c90"/><text x="86" y="155">c1</text>
    <rect x="132" y="140" rx="4" width="52" height="22" fill="#e8eef6" stroke="#6b7c90"/><text x="142" y="155">c2</text>
    <rect x="188" y="140" rx="4" width="52" height="22" fill="#e8eef6" stroke="#6b7c90"/><text x="202" y="155">…</text>
    <rect x="300" y="140" rx="4" width="52" height="22" fill="#e8eef6" stroke="#6b7c90"/><text x="310" y="155">c0</text>
    <rect x="356" y="140" rx="4" width="52" height="22" fill="#e8eef6" stroke="#6b7c90"/><text x="366" y="155">c1</text>
    <rect x="412" y="140" rx="4" width="52" height="22" fill="#e8eef6" stroke="#6b7c90"/><text x="422" y="155">c2</text>
    <rect x="468" y="140" rx="4" width="52" height="22" fill="#e8eef6" stroke="#6b7c90"/><text x="482" y="155">…</text>
  </g>
  <g stroke="#1f6b4a" stroke-width="1.2" opacity="0.45">
    <line x1="79" y1="108" x2="46" y2="140"/>
    <line x1="79" y1="108" x2="102" y2="140"/>
    <line x1="79" y1="108" x2="158" y2="140"/>
    <line x1="207" y1="108" x2="158" y2="140"/>
    <line x1="207" y1="108" x2="214" y2="140"/>
    <line x1="359" y1="108" x2="326" y2="140"/>
    <line x1="359" y1="108" x2="382" y2="140"/>
    <line x1="487" y1="108" x2="438" y2="140"/>
    <line x1="487" y1="108" x2="494" y2="140"/>
  </g>
  <rect x="20" y="188" rx="8" width="800" height="56" fill="#e3f2ea" stroke="#1f6b4a"/>
  <text x="420" y="214" text-anchor="middle" font-family="ui-sans-serif,system-ui,sans-serif" font-size="14" font-weight="700" fill="#1f6b4a">expensive unregister = 2 × F named probes — not 2 × F × NCPU</text>
  <text x="420" y="234" text-anchor="middle" font-family="ui-sans-serif,system-ui,sans-serif" font-size="12" fill="#1c2329">Shutdown still closes every sample fd, then UndefineTracefsUprobe for each name</text>
  <text x="16" y="272" font-family="ui-sans-serif,system-ui,sans-serif" font-size="13" fill="#5a6570">Sample layout (PERF_SAMPLE_REGS_USER / STACK_USER) matches the old PMU helpers.</text>
  <text x="16" y="294" font-family="ui-sans-serif,system-ui,sans-serif" font-size="13" fill="#5a6570">Coverage stays pid=-1,cpu=N. The shared registration is what makes stop cheap.</text>
</svg>
</div>
</div>

<div class="card">
<h3>close() under <code>event_mutex</code> (historical local PMU)</h3>
<p class="note">This is why core count multiplies stop time. Shared-tracefs pays this sequence once per named probe, not once per sample fd.</p>
<div class="svg-wrap">
<svg viewBox="0 0 840 220" width="840" height="220" role="img" aria-labelledby="seq-title seq-desc">
  <title id="seq-title">perf_uprobe_destroy holds event_mutex for the whole close</title>
  <desc id="seq-desc">close of a local PMU fd locks event_mutex, then uprobe_apply false, uprobe_unregister_nosync with a VMA walk, uprobe_unregister_sync with RCU grace periods, and tracepoint_synchronize_unregister, then unlocks.</desc>
  <rect x="0" y="0" width="840" height="220" fill="#fffdf8"/>
  <text x="16" y="26" font-family="ui-sans-serif,system-ui,sans-serif" font-size="14" font-weight="700" fill="#1c2329">close(fd) → lock event_mutex → four steps → unlock</text>
  <rect x="16" y="44" rx="6" width="100" height="44" fill="#2b3036"/>
  <text x="66" y="71" text-anchor="middle" font-family="ui-sans-serif,system-ui,sans-serif" font-size="13" fill="#fff">close(fd)</text>
  <rect x="132" y="44" rx="6" width="120" height="44" fill="#9a3412"/>
  <text x="192" y="71" text-anchor="middle" font-family="ui-sans-serif,system-ui,sans-serif" font-size="12" fill="#fff">event_mutex</text>
  <g font-family="ui-sans-serif,system-ui,sans-serif" font-size="11">
    <rect x="268" y="36" rx="6" width="132" height="60" fill="#f6e4e6" stroke="#8f2d3a"/>
    <text x="334" y="58" text-anchor="middle" fill="#8f2d3a" font-weight="700">1. apply(false)</text>
    <text x="334" y="76" text-anchor="middle" fill="#5a6570">disable the probe</text>
    <rect x="412" y="36" rx="6" width="148" height="60" fill="#f6e4e6" stroke="#8f2d3a"/>
    <text x="486" y="58" text-anchor="middle" fill="#8f2d3a" font-weight="700">2. unregister_nosync</text>
    <text x="486" y="76" text-anchor="middle" fill="#5a6570">VMA walk</text>
    <rect x="572" y="36" rx="6" width="132" height="60" fill="#f6e4e6" stroke="#8f2d3a"/>
    <text x="638" y="58" text-anchor="middle" fill="#8f2d3a" font-weight="700">3. unregister_sync</text>
    <text x="638" y="76" text-anchor="middle" fill="#5a6570">RCU + SRCU GPs</text>
    <rect x="716" y="36" rx="6" width="108" height="60" fill="#f6e4e6" stroke="#8f2d3a"/>
    <text x="770" y="58" text-anchor="middle" fill="#8f2d3a" font-weight="700">4. sync unreg</text>
    <text x="770" y="76" text-anchor="middle" fill="#5a6570">tracepoint</text>
  </g>
  <path d="M116 66 H132" stroke="#2b3036" stroke-width="2"/>
  <path d="M252 66 H268" stroke="#9a3412" stroke-width="2"/>
  <path d="M400 66 H412" stroke="#8f2d3a" stroke-width="1.5"/>
  <path d="M560 66 H572" stroke="#8f2d3a" stroke-width="1.5"/>
  <path d="M704 66 H716" stroke="#8f2d3a" stroke-width="1.5"/>
  <rect x="16" y="120" rx="8" width="808" height="80" fill="#efeae0"/>
  <text x="32" y="146" font-family="ui-sans-serif,system-ui,sans-serif" font-size="13" fill="#1c2329">A second close() waits on event_mutex until this whole sequence finishes. Grace periods do not overlap.</text>
  <text x="32" y="168" font-family="ui-sans-serif,system-ui,sans-serif" font-size="13" fill="#1c2329">Historical cost: repeat once per PMU fd (2×F×NCPU). Production cost: once per named probe (2×F).</text>
  <text x="32" y="190" font-family="ui-sans-serif,system-ui,sans-serif" font-size="13" fill="#5a6570">That is why stop time was proportional to cores, and why ParallelFor cannot erase it.</text>
</svg>
</div>
</div>

<h2>4. Measured attach/detach (10 / 20 / 50)</h2>
<p class="note">Times are this run only. Missing columns are <span class="na">n/a</span> — numbers are never invented. Speedup = percpu_stop / shared_stop. percpu = historical local PMU; shared = production tracefs path.</p>

<!-- ORBIT_BENCH_FILL_START -->
<div class="card">
<p><strong>kernel:</strong> not measured on this static copy<br>
<strong>ncpus:</strong> n/a<br>
<strong>pid model:</strong> production samples use pid=-1,cpu=N (every thread, every core). pid=target,cpu=-1 is a fewer-fd experiment only (single tid + inherit).<br>
<strong>uprobe PMU:</strong> n/a<br>
<strong>tracefs:</strong> n/a</p>
<table>
  <thead>
    <tr>
      <th>nfunctions</th>
      <th>ncpus</th>
      <th>percpu fds</th>
      <th>named probes</th>
      <th>percpu start</th>
      <th>percpu stop</th>
      <th>shared start</th>
      <th>shared stop</th>
      <th>speedup</th>
    </tr>
  </thead>
  <tbody>
    <tr>
      <td>10</td>
      <td class="na">n/a</td>
      <td>2×10×NCPU</td>
      <td>20</td>
      <td class="na">n/a</td>
      <td class="na">n/a</td>
      <td class="na">n/a</td>
      <td class="na">n/a</td>
      <td class="na">n/a</td>
    </tr>
    <tr>
      <td>20</td>
      <td class="na">n/a</td>
      <td>2×20×NCPU</td>
      <td>40</td>
      <td class="na">n/a</td>
      <td class="na">n/a</td>
      <td class="na">n/a</td>
      <td class="na">n/a</td>
      <td class="na">n/a</td>
    </tr>
    <tr>
      <td>50</td>
      <td class="na">n/a</td>
      <td>2×50×NCPU</td>
      <td>100</td>
      <td class="na">n/a</td>
      <td class="na">n/a</td>
      <td class="na">n/a</td>
      <td class="na">n/a</td>
      <td class="na">n/a</td>
    </tr>
  </tbody>
</table>
<p class="note">percpu_fds = 2×F×NCPU; named_probes = 2×F.</p>
<h3>pid=target,cpu=-1 (experiment, 2×F fds; not production)</h3>
<table>
  <thead>
    <tr><th>nfunctions</th><th>start</th><th>stop</th></tr>
  </thead>
  <tbody>
    <tr><td>10</td><td class="na">n/a</td><td class="na">n/a</td></tr>
    <tr><td>20</td><td class="na">n/a</td><td class="na">n/a</td></tr>
    <tr><td>50</td><td class="na">n/a</td><td class="na">n/a</td></tr>
  </tbody>
</table>
<p><span class="verdict na">n/a</span> Bench not run on this static copy. Open the HTML written next to cwd after a real run to see timings.</p>
<p><strong>command:</strong> not run</p>
</div>
<!-- ORBIT_BENCH_FILL_END -->

<p class="note">legend: percpu = historical local PMU (<code>event_mutex</code> × fds); shared = production (one tracefs unregister per function). Verdict uses the existing policy: FAIL only if shared start/stop is more than 250&nbsp;ms worse than percpu PMU.</p>

<h2>5. How to re-run</h2>
<p>Off by default, even as root. The 10/20/50 bench is not part of a normal <code>LinuxTracingIntegrationTests</code> invocation.</p>
<pre class="cmd"><code>./src/LinuxTracingIntegrationTests/run_uprobe_attach_detach_bench.sh
# or: sudo ORBIT_UPROBE_BENCH=1 ./bin/LinuxTracingIntegrationTests --gtest_filter='*UprobeAttachDetachBench*'</code></pre>
<p>The script finds the binary (or takes a path), exports <code>ORBIT_UPROBE_BENCH=1</code> (<code>true</code>/<code>yes</code> also work), re-execs under sudo, and runs only this test — timings plus the tracer e2e (start/stop/start + function-call check). It writes <code>uprobe_attach_detach_bench.txt</code> and <code>uprobe_attach_detach_bench.html</code> in the current directory.</p>

<footer>
  No external CSS, fonts, or images. This file is readable offline. Static copy in the tree: <code>docs/uprobe-stop.html</code> and <code>src/LinuxTracingIntegrationTests/uprobe_attach_detach_bench.html</code>.
</footer>
</main>
</body>
</html>
)UPROBEHTML";

#endif  // LINUX_TRACING_INTEGRATION_TESTS_UPROBE_BENCH_HTML_TEMPLATE_H_
