import sys, numpy as np
from PIL import Image
a = np.asarray(Image.open(sys.argv[1]).convert('RGB')).astype('i4'); b = np.asarray(Image.open(sys.argv[2]).convert('RGB')).astype('i4')
d = np.abs(a-b)
print('max diff', d.max(), 'mean', round(float(d.mean()),4), 'px>2:', int((d.max(axis=2)>2).sum()), 'px>8:', int((d.max(axis=2)>8).sum()), 'of', a.shape[0]*a.shape[1])
side = np.concatenate([a, b, np.clip(d*8,0,255)], axis=1).astype('u1')
Image.fromarray(side).save(sys.argv[3])
