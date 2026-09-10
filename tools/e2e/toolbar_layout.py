#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Check toolbar layout, live process ordering, and shared right-pane tools.
Run against a service embedding the current viewer; API fixtures avoid attaching.
Requires Playwright and Chromium.
"""
import argparse
import json
from playwright.sync_api import sync_playwright

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--url', default='http://127.0.0.1:44771/')
parser.add_argument('--chromium', default='/usr/bin/google-chrome')
args = parser.parse_args()

with sync_playwright() as playwright:
    browser = playwright.chromium.launch(executable_path=args.chromium, headless=True,
        args=['--no-sandbox', '--enable-unsafe-swiftshader'])
    page = browser.new_page(viewport={'width': 1200, 'height': 800})
    processes = [{'pid': 42, 'name': 'alpha', 'cpu': 20}, {'pid': 43, 'name': 'beta', 'cpu': 80}]
    polls = []
    def process_list(route):
        polls.append(1)
        route.fulfill(json=processes)
    page.route('**/api/processes', process_list)
    page.route('**/api/status', lambda r: r.fulfill(json={'hooks': True, 'capturing': False, 'demo': False, 'wire': 'packed'}))
    symbols = {'pid': 42, 'status': 'loading', 'function_count': 120, 'module_count': 2, 'elapsed_ms': 20}
    page.route('**/api/symbols/**', lambda r: r.fulfill(json=symbols))
    page.route('**/api/functions/search?*', lambda r: r.fulfill(json={'pid': 42, 'status': 'ready', 'functions': []}))
    trace = {'traceEvents': [{'ph': 'X', 'name': f'work{i}', 'pid': 42, 'tid': tid,
        'ts': i * 1000, 'dur': 700} for tid in [10, 11] for i in range(10)]}
    page.route(args.url.rstrip('/') + '/toolbar-fixture.json', lambda r: r.fulfill(json=trace))
    errors = []
    page.on('pageerror', lambda e: errors.append(str(e)))
    page.goto(args.url + '?trace=/toolbar-fixture.json')
    page.wait_for_function('window.__orbit_ui')
    def rows():
        return json.loads(page.evaluate('window.__orbit_ui'))
    def state():
        return json.loads(page.evaluate('window.__orbit_sel'))
    def rect(label):
        page.wait_for_function("label => JSON.parse(window.__orbit_ui).some(r => r[0] === label)", arg=label)
        return next(r for r in rows() if r[0] == label)
    def click(label):
        _, x, y, w, h = rect(label)
        page.mouse.click(x + w / 2, y + h / 2)
        page.wait_for_timeout(300)
    page.wait_for_timeout(1100)
    for width in [1200, 1024, 900]:
        page.set_viewport_size({'width': width, 'height': 800})
        page.wait_for_timeout(300)
        controls = [rect(name) for name in ['Process', 'Symbols', 'More', 'Open', 'Settings']]
        assert all(r[1] >= 0 and r[1] + r[3] <= width for r in controls), controls
        assert max(r[2] for r in controls) - min(r[2] for r in controls) < 10, controls
        assert not any(r[0] in ['Refresh', 'Report', 'Self', 'Move', 'Paper'] for r in rows()), rows()
    page.set_viewport_size({'width': 1200, 'height': 800})
    processes.extend({'pid': pid, 'name': f'worker-{pid}', 'cpu': 1} for pid in range(100, 125))
    click('Process')
    page.wait_for_timeout(1400)
    visible = [r for r in rows() if r[0].startswith('visible-process:')]
    assert 20 <= len(visible) <= 21, visible
    assert all(r[2] >= 0 and r[2] + r[4] <= 800 for r in visible), visible
    page.screenshot(path='/tmp/orbit-process-menu.png')
    assert rect('active-process:43')
    page.keyboard.press('ArrowDown')
    page.wait_for_timeout(150)
    assert rect('active-process:42')
    page.keyboard.press('ArrowUp')
    page.wait_for_timeout(150)
    assert rect('active-process:43')
    for _ in range(24):
        page.keyboard.press('ArrowDown')
    page.wait_for_timeout(400)
    assert rect('visible-process:122')
    page.keyboard.press('Enter')
    page.wait_for_timeout(300)
    assert state()['selected_pid'] == 122, state()
    page.wait_for_timeout(200)
    page.keyboard.press('Control+Shift+p')
    page.wait_for_timeout(200)
    click('process-filter')
    page.keyboard.type('worker-124')
    page.wait_for_timeout(200)
    page.keyboard.press('Enter')
    page.wait_for_timeout(300)
    assert state()['selected_pid'] == 124, state()
    page.wait_for_timeout(200)
    page.keyboard.press('Control+Shift+p')
    page.wait_for_timeout(200)
    click('process-filter')
    page.keyboard.press('Control+a')
    page.keyboard.type('no-such-process')
    page.keyboard.press('Enter')
    page.wait_for_timeout(200)
    assert state()['selected_pid'] == 124, state()
    page.keyboard.press('Control+a')
    page.keyboard.press('Backspace')
    page.wait_for_timeout(200)
    page.keyboard.press('Enter')
    page.wait_for_timeout(300)
    assert state()['selected_pid'] == 43, state()
    page.keyboard.press('Escape')
    del processes[2:]
    page.wait_for_timeout(1400)
    click('Process')
    assert [r[0] for r in rows() if r[0].startswith('process:')] == ['process:43', 'process:42']
    before = len(polls)
    processes[0]['cpu'] = 95
    page.wait_for_timeout(1400)
    assert len(polls) > before
    assert [r[0] for r in rows() if r[0].startswith('process:')] == ['process:42', 'process:43']
    click('process:42')
    page.wait_for_timeout(700)
    assert state()['selected_pid'] == 42
    assert any(r[0].startswith('symbols-status:Loading 120 symbols') for r in rows()), rows()
    symbols.update(status='ready', function_count=240, elapsed_ms=321)
    page.wait_for_timeout(700)
    assert any(r[0] == 'symbols-status:Loaded 240 symbols in 321 ms' for r in rows()), rows()
    click('More'); click('Inspector')
    assert state()['tab'] == 'Inspector' and state()['report_open'], state()
    page.keyboard.press('r'); page.wait_for_timeout(300)
    assert not state()['report_open'], state()
    page.keyboard.press('i'); page.wait_for_timeout(300)
    assert state()['tab'] == 'Inspector' and state()['report_open'], state()
    click('Selection')
    assert state()['tab'] == 'Selection', state()
    click('More')
    follow = next(r[0] for r in rows() if r[0].startswith('Follow:'))
    click(follow)
    click('More')
    assert next(r[0] for r in rows() if r[0].startswith('Follow:')) != follow
    page.keyboard.press('Escape')
    # A real marquee opens the Selection tab in the same resizable right pane.
    page.keyboard.press('r')
    page.wait_for_timeout(500)
    _, x, y, w, h = rect('row:thread:42:10')
    page.keyboard.down('Control')
    page.mouse.move(x + w + 30, y + h * .55)
    page.mouse.down()
    page.mouse.move(x + w + 220, y + h * .95, steps=20)
    page.mouse.up()
    page.keyboard.up('Control')
    page.wait_for_timeout(700)
    assert state()['tab'] == 'Selection' and state()['report_open'], state()
    assert state()['rect'], state()
    page.screenshot(path='/tmp/orbit-toolbar-selection.png')
    click('selection:copy')
    click('selection:clear')
    assert not state()['rect'] and state()['report_open'], state()
    click('Inspector')
    page.screenshot(path='/tmp/orbit-toolbar-inspector.png')
    page.keyboard.press('F2')
    page.wait_for_function('window.__orbit_self')
    page.keyboard.press('F2')
    page.wait_for_timeout(300)
    assert not errors, errors
    print('PASS single-row toolbar, process refresh/order, symbol progress, shortcuts and right-pane tools')
    browser.close()
