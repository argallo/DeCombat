# Liquid Mercury: procedural, tileable molten-chrome bands. Original art, numpy only.
import sys, numpy as np
from PIL import Image
N=512; TAU=2*np.pi
y,x=np.mgrid[0:N,0:N]/N
rng=np.random.default_rng(int(sys.argv[1]) if len(sys.argv)>1 else 7)
def field(x,y,terms,amp0=1.0):
    f=np.zeros_like(x)
    for (a,b),amp in terms:
        ph=rng.uniform(0,TAU)
        f+=amp*np.sin(TAU*(a*x+b*y)+ph)
    return f
# domain warp (integer frequencies keep it on the torus)
w1=field(x,y,[((1,0),1),((0,1),1),((1,1),.7),((2,-1),.5),((1,2),.4)])
w2=field(x,y,[((1,0),1),((0,1),1),((1,-1),.7),((-1,2),.5),((2,1),.4)])
k=0.10
xu=x+k*w1; yv=y+k*w2
base=field(xu,yv,[((1,1),1.0),((2,0),.6),((0,2),.6),((1,-2),.35),((3,1),.2)])
base=(base-base.min())/(base.max()-base.min())
# reflection bands: a few fat bands across the warped field
t=0.5+0.5*np.sin(TAU*3.5*base+1.3)          # band phase
t=t**1.4
# chrome ramp: violet shadow -> gunmetal -> blue steel -> mid highlight
stops=np.array([0.0,0.30,0.62,0.86,1.0])
cols=np.array([[0.11,0.15,0.21],[0.16,0.21,0.27],[0.19,0.27,0.36],[0.27,0.35,0.43],[0.31,0.38,0.45]])
out=np.stack([np.interp(t,stops,cols[:,c]) for c in range(3)],-1)
# thin bright rim at the edge of each highlight band (the liquid "lip")
lip=np.exp(-((t-0.78)/0.05)**2)
out+=lip[...,None]*np.array([0.03,0.035,0.04])
# faint violet in the deepest troughs
deep=np.clip((0.25-t)/0.25,0,1)
out+=deep[...,None]*np.array([0.02,-0.01,0.025])
out=np.clip(out,0,1)
Image.fromarray((out*255).astype(np.uint8)).save('paint.png')
t2=Image.open('paint.png'); big=Image.new('RGB',(N*2,N*2))
for i in range(2):
    for j in range(2): big.paste(t2,(i*N,j*N))
big.resize((512,512)).save('tile_check.png')
