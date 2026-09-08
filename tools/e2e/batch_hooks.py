#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Browser regression: batch hook actions on selected Functions rows.
Run against a service embedding the current viewer; API fixtures avoid ptrace.
Requires playwright and Chromium.
"""
import argparse
import json
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--url', default='http://127.0.0.1:44771/')
parser.add_argument('--chromium', default='/usr/bin/google-chrome')
args = parser.parse_args()
from playwright.sync_api import sync_playwright
with sync_playwright() as p:
 b=p.chromium.launch(executable_path=args.chromium,headless=True,args=['--no-sandbox','--enable-unsafe-swiftshader'])
 page=b.new_page(viewport={'width':1440,'height':950})
 page.route('**/api/processes',lambda r:r.fulfill(json=[{'pid':42,'name':'fixture'}]))
 page.route('**/api/functions/search?*',lambda r:r.fulfill(json={'pid':42,'status':'ready','functions':[{'function_id':i,'name':f'function_{i}','module':'fixture','size':16} for i in range(1,9)]}))
 page.route('**/api/symbols/**',lambda r:r.fulfill(json={'pid':42,'status':'ready','function_count':8,'module_count':1}))
 page.goto(args.url)
 page.wait_for_function('window.__orbit_ui')
 page.wait_for_timeout(1000)
 def rows():
  v=page.evaluate('window.__orbit_ui');return json.loads(v) if isinstance(v,str) else v
 def rect(label):return next(r for r in rows() if r[0]==label)
 def click(label,button='left'):
  r=rect(label);page.mouse.click(r[1]+r[3]/2,r[2]+r[4]/2,button=button);page.wait_for_timeout(300)
 page.mouse.click(280,50);page.wait_for_timeout(300)
 page.mouse.click(110,100);page.wait_for_timeout(500)
 click('Report');click('Functions');page.wait_for_timeout(700)
 click('hook:function_1')  # A mixed selection must hook all, not toggle each.
 a=rect('fn:function_1'); z=rect('fn:function_3')
 page.mouse.move(a[1]+50,a[2]+a[4]/2);page.mouse.down()
 page.mouse.move(z[1]+50,z[2]+z[4]/2,steps=15);page.mouse.up();page.wait_for_timeout(400)
 assert any(r[0] == 'Hook 3' for r in rows()), rows()
 click('fn:function_2',button='right');click('menu:hook')
 def hooks():
  v=page.evaluate('window.__orbit_sel');v=json.loads(v) if isinstance(v,str) else v
  return sorted(v['hooks'])
 assert hooks()==[1,2,3], hooks()
 click('hook:function_2');assert hooks()==[],hooks()
 click('hook:function_2');assert hooks()==[1,2,3],hooks()
 click('fn:function_5',button='right');click('menu:hook');assert hooks()==[1,2,3,5],hooks()
 print('PASS batch context menu, checkbox hook/unhook, unselected row isolation')
 b.close()
