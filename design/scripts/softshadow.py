"""Fade only the floor-shadow region (below the feet) toward left/right/bottom so it never shows a hard edge."""
import sys, numpy as np
from PIL import Image
def mask(w,h,start=0.62):
    yy,xx=np.mgrid[0:h,0:w].astype(np.float32)
    t=np.clip((yy/h-start)/(1-start),0,1)                       # 0 above the feet, 1 at bottom
    dx=np.abs(xx-w/2)/(w/2)
    hx=np.clip((1-dx)/0.35,0,1); hx=hx*hx*(3-2*hx)
    vb=np.clip((1-yy/h)/0.12,0,1); vb=vb*vb*(3-2*vb)
    return (1-t)+t*hx*vb
for src in sys.argv[1:]:
    im=Image.open(src).convert('RGBA'); a=np.asarray(im).astype(np.float32)
    a[...,3]*=mask(im.width,im.height); o=Image.fromarray(a.astype(np.uint8),'RGBA')
    base=src.rsplit('.',1)[0]
    if src.endswith('.png'): o.save(base+'.png',optimize=True)
    o.save(base+'.webp',quality=82,method=4)
