#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Browser regression: batch hook actions on selected Functions rows.
Run against a dedicated test service embedding the current viewer.
API fixtures avoid ptrace; the Live check starts and stops demo events.
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
 page.route('**/api/functions/search?*',lambda r:r.fulfill(json={'pid':42,'status':'ready','functions':[{'function_id':i,'name':f'function_{i}','module':f'module_{9-i}','size':i*16} for i in range(1,9)]}))
 page.route('**/api/symbols/**',lambda r:r.fulfill(json={'pid':42,'status':'ready','function_count':8,'module_count':1}))
 page.route('**/api/sampling/tree?*',lambda r:r.fulfill(json={'samples':100,'roots':[
  {'kind':'function','function_id':100+i,'name':f'tree_{i}','module':f'module_{4-i}',
   'inclusive_percent':100-i*10,'exclusive':i,'of_parent_percent':100-i*10} for i in range(1,4)]}))
 page.route('**/api/symbols/modules?*',lambda r:r.fulfill(json={'pid':42,'modules':[{'name':'beta','path':'/a','function_count':2},{'name':'alpha','path':'/z','function_count':9}]}))
 page.goto(args.url)
 page.wait_for_function('window.__orbit_ui')
 page.wait_for_timeout(1000)
 def rows():
  v=page.evaluate('window.__orbit_ui');return json.loads(v) if isinstance(v,str) else v
 def rect(label):
  page.wait_for_function("label => { let v=window.__orbit_ui; v=typeof v==='string'?JSON.parse(v):v; return v?.some(r=>r[0]===label); }",arg=label)
  return next(r for r in rows() if r[0]==label)
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
 # Every visible Functions column sorts in both directions, including module.
 click('sort:module')
 assert [r[0] for r in rows() if r[0].startswith('fn:')] == [f'fn:function_{i}' for i in range(8,0,-1)]
 click('sort:module')
 assert [r[0] for r in rows() if r[0].startswith('fn:')] == [f'fn:function_{i}' for i in range(1,9)]
 for tab in ['Top-down','Bottom-up']:
  click(tab);page.wait_for_timeout(700)
  clear=next((r for r in rows() if r[0]=='Clear' and r[2]>100),None)
  if clear: page.mouse.click(clear[1]+clear[3]/2,clear[2]+clear[4]/2);page.wait_for_timeout(300)
  # Click a selected tree row's checkbox and context menu, not the toolbar.
  click('hook:tree_1')
  a=rect('tree:tree_1');z=rect('tree:tree_3')
  page.mouse.move(a[1]+8,a[2]+a[4]/2);page.mouse.down()
  page.mouse.move(z[1]+8,z[2]+z[4]/2,steps=15);page.mouse.up();page.wait_for_timeout(400)
  click('tree:tree_2',button='right');click('menu:hook')
  assert set([101,102,103]) <= set(hooks()), (tab,hooks())
  click('hook:tree_2');assert not set([101,102,103]) & set(hooks()), (tab,hooks())
  click('sort:module')
  assert [r[0] for r in rows() if r[0].startswith('tree:')] == ['tree:tree_3','tree:tree_2','tree:tree_1']
  click('sort:module')
  assert [r[0] for r in rows() if r[0].startswith('tree:')] == ['tree:tree_1','tree:tree_2','tree:tree_3']
  click('sort:inclusive') # Restore the tree's default order for the next tab.
 click('Modules');page.wait_for_timeout(400)
 click('sort:module');assert [r[0] for r in rows() if r[0].startswith('module:')]==['module:alpha','module:beta']
 click('sort:module');assert [r[0] for r in rows() if r[0].startswith('module:')]==['module:beta','module:alpha']
 click('sort:path');assert [r[0] for r in rows() if r[0].startswith('module:')]==['module:beta','module:alpha']
 click('sort:symbols');assert [r[0] for r in rows() if r[0].startswith('module:')]==['module:alpha','module:beta']
 # Exercise Live headers with real demo events, then freeze their statistics.
 page.request.post(args.url.rstrip('/')+'/api/demo/start',data=json.dumps({'scopes_per_sec':1000}),headers={'Content-Type':'application/json'})
 page.wait_for_timeout(1800)
 page.request.post(args.url.rstrip('/')+'/api/demo/stop',data='{}',headers={'Content-Type':'application/json'})
 page.route('**/api/processes',lambda r:r.fulfill(json=[]))
 click('Refresh');page.wait_for_timeout(500)
 click('Live');page.wait_for_timeout(500)
 click('sort:function')
 names=[r[0] for r in rows() if r[0].startswith('live:')]
 assert len(names)>1 and names==sorted(names,key=str.lower),names
 click('sort:function')
 assert [r[0] for r in rows() if r[0].startswith('live:')]==list(reversed(names))
 for column in ['type','count','total','avg','min','max','std dev','module']:
  click('sort:'+column)
 # Give the demo scopes real symbol identities, as a captured target would.
 # Selecting another process must also discard the old process's hook IDs.
 live_names=sorted(set(label[5:] for label in names))
 fixture=[{'function_id':200+i,'name':name,'module':'demo','size':16} for i,name in enumerate(live_names)]
 page.route('**/api/processes',lambda r:r.fulfill(json=[{'pid':1,'name':'demo'}]))
 page.route('**/api/functions/search?*',lambda r:r.fulfill(json={'pid':1,'status':'ready','functions':fixture}))
 page.route('**/api/symbols/**',lambda r:r.fulfill(json={'pid':1,'status':'ready','function_count':len(fixture),'module_count':1}))
 click('Refresh');page.wait_for_timeout(300)
 page.mouse.click(280,50);page.wait_for_timeout(300)
 page.mouse.click(110,100);page.wait_for_timeout(800)
 assert hooks()==[], hooks()
 click('sort:function');page.wait_for_timeout(300)
 hook_rows=[r for r in rows() if r[0].startswith('hook:')]
 assert len(hook_rows)>=3, rows()
 targets=hook_rows[:3]
 expected=sorted(next(f['function_id'] for f in fixture if f['name']==r[0][5:]) for r in targets)
 click(targets[0][0])
 a=rect('live:'+targets[0][0][5:]);z=rect('live:'+targets[2][0][5:])
 page.mouse.move(a[1]+8,a[2]+a[4]/2);page.mouse.down()
 page.mouse.move(z[1]+8,z[2]+z[4]/2,steps=15);page.mouse.up();page.wait_for_timeout(400)
 click('live:'+targets[1][0][5:],button='right');click('menu:hook')
 assert hooks()==expected, hooks()
 click(targets[1][0]);assert hooks()==[],hooks()
 click(targets[1][0]);assert hooks()==expected,hooks()
 click('sort:hook')
 # Scope actions always act on that scope, even with a report selection.
 # Sweep the first demo thread's timeline until a scope is hit.
 timeline_right=rect('sort:hook')[1]-12
 lane=next(r for r in rows() if r[0]=='row:thread:1:100')
 x0=lane[1]+lane[3]
 found=False
 for frac in [0.5,0.3,0.7,0.1,0.9]:
  for dy in [0.3,0.4,0.5,0.6,0.2,0.7]:
   page.mouse.click(x0+(timeline_right-x0)*frac,lane[2]+lane[4]*dy,button='right');page.wait_for_timeout(500)
   if any(r[0]=='menu:hook' for r in rows()):
    before=set(hooks());click('menu:hook');after=set(hooks())
    assert len(before.symmetric_difference(after))==1,(before,after)
    page.mouse.click(x0+(timeline_right-x0)*frac,lane[2]+lane[4]*dy,button='right');page.wait_for_timeout(150)
    click('menu:hook');assert set(hooks())==before,hooks()
    found=True;break
   page.keyboard.press('Escape')
  if found:break
 assert found,'No scope hook menu found on the demo thread'
 print('PASS batch hooks in Functions/trees/Live, sorting, process isolation, and timeline hook/unhook')
 b.close()
