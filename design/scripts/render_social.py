import asyncio
from playwright.async_api import async_playwright
from PIL import Image
B='/home/claude/kit/site/brand/'
async def m():
    async with async_playwright() as p:
        b=await p.chromium.launch(); pg=await b.new_page(viewport={'width':1300,'height':1800})
        await pg.goto('file:///home/claude/kit/design/scripts/social.html'); await pg.wait_for_timeout(1200)
        await (await pg.query_selector('#og')).screenshot(path=B+'social/og-image.png')
        await (await pg.query_selector('#i1024')).screenshot(path=B+'logo/app-icon-1024.png', omit_background=True)
        await b.close()
asyncio.run(m())
im=Image.open(B+'logo/app-icon-1024.png')
for s in (512,180): im.resize((s,s),Image.LANCZOS).save(B+f'logo/app-icon-{s}.png')
async def fav():
    async with async_playwright() as p:
        b=await p.chromium.launch()
        for s in (16,32):
            pg=await b.new_page(viewport={'width':s,'height':s},device_scale_factor=1)
            await pg.set_content(f'<body style="margin:0"><img src="file://{B}logo/favicon.svg" width={s} height={s}></body>'); await pg.wait_for_timeout(200)
            await pg.screenshot(path=B+f'logo/favicon-{s}.png', omit_background=True)
        await b.close()
asyncio.run(fav())
Image.open(B+'social/og-image.png').convert('RGB').resize((600,315)).save('og_s.jpg')
