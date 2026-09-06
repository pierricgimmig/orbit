# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license.
#
# Applies the shared Orbit blog look (blog-theme.css: light default, dark
# toggle, Roboto, Orbit blue) to a self-contained post by injecting a token
# override, the Roboto font link, and the theme toggle. Idempotent.
#   python3 tools/site/theme_blog.py docs/blog/NN-slug.html
import sys, re, pathlib
CSS = open(f"{pathlib.Path(__file__).parent}/blog-theme.css").read()
PREPAINT = ('<script>try{var t=localStorage.getItem("orbit-theme");'
            'if(t)document.documentElement.setAttribute("data-theme",t);}catch(e){}</script>')
ROBOTO = ('<link rel="stylesheet" href="https://fonts.googleapis.com/css2?'
          'family=Roboto:wght@400;500;700&family=Roboto+Mono:wght@400;500&display=swap">')
TOGGLE = '<button class="orbit-theme-toggle" id="orbit-theme-toggle" type="button" aria-label="Toggle colour theme">Dark</button>'
WIRE = ('<script>(function(){var b=document.getElementById("orbit-theme-toggle");if(!b)return;'
        'function cur(){var t=document.documentElement.getAttribute("data-theme");'
        'return t||"light";}'
        'function lab(){b.textContent=cur()==="dark"?"Light":"Dark";}lab();'
        'b.addEventListener("click",function(){var n=cur()==="dark"?"light":"dark";'
        'document.documentElement.setAttribute("data-theme",n);'
        'try{localStorage.setItem("orbit-theme",n);}catch(e){}lab();});})();</script>')

def inject(html):
    if 'orbit-theme-toggle' in html:
        return html  # idempotent
    # pre-paint script right after <head>
    html = re.sub(r'(<head[^>]*>)', r'\1\n' + PREPAINT, html, count=1)
    # Roboto fonts + override style just before </head> (override wins, last)
    inject_head = ROBOTO + '\n<style>\n' + CSS + '</style>\n</head>'
    html = html.replace('</head>', inject_head, 1)
    # toggle button + wiring before </body>
    html = html.replace('</body>', TOGGLE + '\n' + WIRE + '\n</body>', 1)
    return html

if __name__ == "__main__":
    for path in sys.argv[1:]:
        p = pathlib.Path(path)
        out = inject(p.read_text())
        p.write_text(out)
        print("themed", p.name)
