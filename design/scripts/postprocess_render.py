import sys, numpy as np
from PIL import Image
src, dst = sys.argv[1], sys.argv[2]
im = np.asarray(Image.open(src).convert('RGBA')).astype(np.float32)
a = im[..., 3] / 255.0
a = np.clip((a - 0.02) / 0.98, 0, 1)                      # alpha floor
h, w = a.shape
yy, xx = np.mgrid[0:h, 0:w]
ex = np.minimum(xx, w - 1 - xx) / (0.10 * w); ey = np.minimum(yy, h - 1 - yy) / (0.10 * h)
vig = np.clip(np.minimum(ex, ey), 0, 1); vig = vig * vig * (3 - 2 * vig)
a = a * vig
im[..., 3] = a * 255
ys, xs = np.where(a > 0.01)
pad = int(0.03 * max(h, w))
y0, y1 = max(ys.min() - pad, 0), min(ys.max() + pad, h); x0, x1 = max(xs.min() - pad, 0), min(xs.max() + pad, w)
out = Image.fromarray(im[y0:y1, x0:x1].astype(np.uint8), 'RGBA')
out.save(dst + '.png', optimize=True); out.save(dst + '.webp', quality=88, method=6)
print(dst, out.size)
