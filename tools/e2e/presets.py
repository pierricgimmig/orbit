#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Browser regression for portable, composable instrumentation presets.
Run against a service embedding the current viewer. API fixtures avoid attaching.
"""
import argparse
import json
from urllib.parse import urlparse, parse_qs
from playwright.sync_api import sync_playwright

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--url', default='http://127.0.0.1:44771/')
parser.add_argument('--chromium', default='/usr/bin/google-chrome')
args = parser.parse_args()

def preset(name, names):
    return dict(version=1, name=name, functions=[dict(module='Game.so', name=n) for n in names])

def upload(name, data):
    return dict(name=name + '.orbit-preset.json', mimeType='application/json',
                buffer=json.dumps(data).encode())

with sync_playwright() as pw:
    browser = pw.chromium.launch(executable_path=args.chromium, headless=True,
                                args=['--no-sandbox', '--enable-unsafe-swiftshader'])
    page = browser.new_page(viewport={'width': 1400, 'height': 900})
    page.route('**/api/status', lambda r: r.fulfill(json=dict(hooks=True, capturing=False, demo=False, wire='packed')))
    page.route('**/api/processes', lambda r: r.fulfill(json=[dict(pid=42,name='game',cpu=30),dict(pid=43,name='other-install',cpu=10)]))
    page.route('**/api/symbols/load', lambda r: r.fulfill(body=''))
    def symbols(r):
        pid = int(parse_qs(urlparse(r.request.url).query)['pid'][0])
        r.fulfill(json=dict(pid=pid,status='ready',function_count=5,module_count=1,elapsed_ms=1))
    page.route('**/api/symbols/status?*', symbols)
    def catalogue(pid):
        return [dict(function_id=pid*100+i, module='/different/install/Game.so', name=n, size=4)
                for i,n in enumerate(['physics','shared','rendering','ambiguous','ambiguous'],1)]
    page.route('**/api/functions/search?*', lambda r: r.fulfill(json=dict(pid=42,status='ready',functions=catalogue(42))))
    resolutions = []
    held = []
    hold_next = False
    def resolve(r):
        global hold_next
        body = r.request.post_data_json
        resolutions.append(body)
        reply = dict(pid=body['pid'],status='ready',functions=catalogue(body['pid']))
        if hold_next:
            held.append((r, reply))
            hold_next = False
        else:
            r.fulfill(json=reply)
    page.route('**/api/functions/resolve', resolve)
    errors = []
    page.on('pageerror', lambda e: errors.append(str(e)))
    page.goto(args.url)
    page.wait_for_function('window.__orbit_ui')
    def rows():
        return json.loads(page.evaluate('window.__orbit_ui'))
    def state():
        return json.loads(page.evaluate('window.__orbit_sel'))
    def click(label):
        page.wait_for_function("s => JSON.parse(window.__orbit_ui).some(r => r[0] === s)", arg=label)
        _,x,y,w,h = next(r for r in rows() if r[0] == label)
        page.mouse.click(x+w/2,y+h/2)
        page.wait_for_timeout(200)
    def hooks(expected):
        page.wait_for_function("ids => JSON.stringify(JSON.parse(window.__orbit_sel).hooks.slice().sort()) === JSON.stringify(ids.slice().sort())", arg=expected)
    def load(files):
        with page.expect_file_chooser() as chooser:
            click('preset:load')
        chooser.value.set_files(files)
        page.wait_for_timeout(250)
    click('More')
    click('Presets')
    physics = preset('unreal-physics',['physics','shared','missing','ambiguous'])
    rendering = preset('unreal-rendering',['rendering','shared'])
    load([upload('physics',physics),upload('rendering',rendering)])
    assert not resolutions, 'must wait for a selected process'
    click('Process')
    click('process:42')
    hooks([4201,4202,4203])
    assert any('1 missing, 1 ambiguous' in r[0] for r in rows()), rows()
    assert len(resolutions[-1]['functions']) == 5, resolutions
    click('preset:apply')
    hooks([4201,4202,4203])
    with page.expect_download() as download:
        click('preset:save')
    saved = json.loads(open(download.value.path()).read())
    assert saved['version'] == 1
    assert len(saved['functions']) == 3
    assert all(set(f)=={'module','name'} and f['module']=='Game.so' for f in saved['functions'])
    assert 'function_id' not in json.dumps(saved) and '/different/' not in json.dumps(saved)
    # Invalid imports leave the existing selection and loaded presets intact.
    load([upload('bad',dict(version=99,name='bad',functions=[]))])
    hooks([4201,4202,4203])
    page.wait_for_function("JSON.parse(window.__orbit_ui).some(r => r[0] === 'preset:error')")
    # A response for the old process arriving last must not add its addresses.
    hold_next = True
    click('preset:apply')
    page.wait_for_timeout(300)
    assert held
    click('Process')
    click('process:43')
    hooks([4301,4302,4303])
    held[0][0].fulfill(json=held[0][1])
    page.wait_for_timeout(300)
    hooks([4301,4302,4303])
    load([upload('exported',saved)])
    hooks([4301,4302,4303])
    page.screenshot(path='/tmp/orbit-presets.png')
    assert not errors, errors
    print('PASS: multi-file presets, export, additive/idempotent resolution, missing/ambiguous symbols, invalid input, and process switching')
    browser.close()
