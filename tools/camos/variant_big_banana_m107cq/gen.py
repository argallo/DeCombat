# Big Banana: procedural ripe banana peel, tileable 512x512 (whole-number frequencies + wrapped blobs)
import numpy as np, sys
from PIL import Image
N=512
rng=np.random.default_rng(11)
y,x=np.mgrid[0:N,0:N]/N
T=2*np.pi
base=np.array([0.63,0.51,0.05])
img=np.ones((N,N,3))*base
# lengthwise facets: 8 broad ridges with thin dark seams and a soft highlight on each facet
wob=0.004*np.sin(T*(2*x))+0.003*np.sin(T*(3*x+0.3))
ph=(8*(y+wob))%1.0
seam=np.exp(-((ph-0.5)/0.035)**2)          # thin dark seam line
hi=np.exp(-((ph-0.2)/0.14)**2)             # soft highlight off-centre
img*=(1-0.22*seam)[...,None]
img*=(1+0.10*hi)[...,None]
img[...,2]+=0.0*hi
# fine fibrous streaks along the length
fib=np.zeros((N,N))
for k in range(36):
    fy=rng.integers(30,110); fx=rng.integers(1,4); p=rng.random()*T; p2=rng.random()*T
    fib+=np.sin(T*fy*y+0.6*np.sin(T*fx*x+p2)+p)/36
img*=(1+0.45*fib)[...,None]
# green-tinged streaks: thin, soft, fading along the length
g=np.zeros((N,N))
for (fy,fx,p,a,w) in [(1,1,0.10,1.0,0.030),(1,2,0.47,0.9,0.022),(1,1,0.73,1.0,0.026),(1,3,0.30,0.7,0.016)]:
    c=(y - p - 0.02*np.sin(T*(fx*x+p*7)))%1.0
    d=np.minimum(c,1-c)
    fade=0.5+0.5*np.sin(T*(fx*x+p*3))
    g=np.maximum(g,a*np.exp(-(d/w)**2)*(0.35+0.65*fade))
green=np.array([0.36,0.55,0.06])
img=img*(1-0.9*g[...,None])+green*0.9*g[...,None]
# brown sugar spots, wrapped on the torus
def blob(cx,cy,rad,ax,soft):
    dx=(x-cx+0.5)%1-0.5; dy=(y-cy+0.5)%1-0.5
    d=np.sqrt((dx/ax)**2+(dy*ax)**2)/rad
    return np.clip((1.0-d)/soft+0.5,0,1)
spots=np.zeros((N,N)); halo=np.zeros((N,N))
for i in range(150):
    cx,cy=rng.random(),rng.random()
    rad=rng.choice([0.005,0.008,0.012,0.018],p=[0.35,0.35,0.2,0.1])
    ax=rng.uniform(1.0,1.7)
    k=rng.uniform(0.75,1.0)
    spots=np.maximum(spots,blob(cx,cy,rad,ax,0.6)*k)
    halo=np.maximum(halo,blob(cx,cy,rad*1.9,ax,1.2)*0.5*k)
for i in range(320):
    cx,cy=rng.random(),rng.random()
    spots=np.maximum(spots,blob(cx,cy,0.003,1.0,0.8)*rng.uniform(0.5,0.9))
halo_c=np.array([0.55,0.36,0.05])            # caramel ring around each sugar spot
h=np.clip(halo,0,1)[...,None]
img=img*(1-h*0.7)+halo_c*h*0.7
s=np.clip(spots,0,1)[...,None]
brown=np.array([0.34,0.21,0.05]); dark=np.array([0.19,0.11,0.02])
spc=brown*(1-s)+dark*s
m=np.clip(s*1.5,0,1)
img=img*(1-m)+spc*m
out=np.clip(img,0,1)
Image.fromarray((out*255+0.5).astype(np.uint8)).save(sys.argv[1] if len(sys.argv)>1 else 'paint.png')
