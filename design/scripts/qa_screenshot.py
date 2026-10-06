import asyncio, sys
from playwright.async_api import async_playwright
W=int(sys.argv[1]); out=sys.argv[2]
async def m():
    async with async_playwright() as p:
        b=await p.chromium.launch(); pg=await b.new_page(viewport={'width':W,'height':900})
        await pg.goto('file:///home/claude/kit/design/redesign/landing.html')
        await pg.evaluate("document.querySelectorAll('img').forEach(i=>i.loading='eager');document.querySelectorAll('.reveal').forEach(e=>e.classList.add('in'))")
        await pg.wait_for_timeout(1500)
        await pg.screenshot(path=out, full_page=True)
        # also: spin frames at three scroll positions
        if W>1000:
            for k,f in enumerate([0.15,0.5,0.85]):
                await pg.evaluate(f"(()=>{{const s=document.getElementById('spin');scrollTo(0,s.offsetTop+(s.offsetHeight-innerHeight)*{f})}})()")
                await pg.wait_for_timeout(700); await pg.screenshot(path=f'spin{k}.png')
        await b.close()
asyncio.run(m())
