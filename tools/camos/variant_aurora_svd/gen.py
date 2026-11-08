import numpy as np
from PIL import Image
import os
N=512
rng=np.random.default_rng(7)
y,x=np.mgrid[0:N,0:N]/N   # u=x, v=y in 0..1, torus
TAU=2*np.pi
def pnoise(freqs, amp_decay=0.6, seed=0, axis=x):
    r=np.random.default_rng(seed); out=np.zeros_like(axis)
    a=1.0
    for f in freqs:
        out+=a*np.sin(TAU*f*axis+r.uniform(0,TAU)); a*=amp_decay
    return out
# sky: navy with a slight vertical gradient (periodic)
sky=np.array([0.04,0.08,0.24])
img=np.zeros((N,N,3))+sky
img*= (0.9+0.1*np.cos(TAU*y))[...,None]
green=np.array([0.15,0.90,0.38]); teal=np.array([0.08,0.66,0.70]); violet=np.array([0.50,0.18,0.70])
curtains=[(0.12,1,0.22),(0.45,2,0.20),(0.76,3,0.22)]
for i,(base,seed,depth) in enumerate(curtains):
    # wavy lower edge line, periodic in x; also slight tilt via integer freq
    edge=base+0.05*pnoise([1,2,3,5,8],0.6,seed)
    d=(y-edge)%1.0           # distance below... we want curtain above edge: use (edge - y)
    up=(edge-y)%1.0          # 0 at edge, grows upward
    h=depth*(1+0.35*pnoise([2,3,7],0.6,seed+10))
    t=np.clip(up/h,0,None)
    inten=np.where(up<0.5, (np.exp(-t*0.7)*1.3)*(1-np.exp(-up*160)), 0)
    # below edge: sharp fade
    below=(y-edge)%1.0
    inten=inten+np.where(below<0.5,np.exp(-below*60)*0.45,0)
    # rays: vertical streaks
    rays=0.45+0.55*np.clip(pnoise([5,8,13,19,29],0.75,seed+20)/2.0,-1,1)
    rays=np.clip(rays,0,1)**1.5
    inten*=rays*(0.75+0.25*pnoise([4,6],0.6,seed+30))
    inten=np.clip(inten,0,1)
    # colour by height in curtain: green low, teal mid, violet top
    tt=np.clip(t/1.5,0,1)[...,None]
    col=np.where(tt<0.5, green*(1-tt*2)+teal*(tt*2), teal*(1-(tt-0.5)*2)+violet*((tt-0.5)*2))
    a=(inten*(1-0.35*np.clip(t/1.5,0,1)))[...,None]
    img=img*(1-a)+col*a
# stars
for _ in range(70):
    cx,cy=rng.uniform(0,1,2); s=rng.uniform(0.0015,0.0035); b=rng.uniform(0.5,1)
    dx=((x-cx+0.5)%1)-0.5; dy=((y-cy+0.5)%1)-0.5
    g=np.exp(-(dx*dx+dy*dy)/(2*s*s))*b*0.75
    img=img*(1-g[...,None])+np.array([0.75,0.78,0.85])*g[...,None]
img=np.clip(img,0,1)
out=os.path.dirname(os.path.abspath(__file__))
Image.fromarray((img*255).astype(np.uint8)).save(os.path.join(out,'paint.png'))
t=np.tile(img,(2,2,1)); Image.fromarray((t*255).astype(np.uint8)).save(os.path.join(out,'tile_check.png'))
