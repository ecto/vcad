# Mesh-grounded settle: drop a R25 sphere into the actual lower-half STL across
# the mouth, descend quasi-statically (z minimization with no penetration),
# report where it comes to rest. Catches anything analytic planes miss
# (bolt slots, neck relief interrupting the walls).
import numpy as np, trimesh
H="/Users/cam/Developer/ipse/.claude/worktrees/humanoid-viral-video-ideas-43367f/hardware/"
low=trimesh.load(H+"k1-cuff-lower.stl")
pq=trimesh.proximity.ProximityQuery(low)
R=25.0
def free_z(x,y):
    # lowest z for sphere center at (x,y) with dist(mesh, center)>=R
    lo,hi=-40.0,40.0
    for _ in range(60):
        m=(lo+hi)/2
        d=pq.signed_distance([[x,y,m]])[0]   # + inside mesh, - outside
        if -d>=R: hi=m
        else: lo=m
    return hi
def settle(x0,y0,step=0.8):
    x,y=x0,y0
    z=free_z(x,y)
    for _ in range(300):
        best=(x,y,z)
        for dx,dy in ((step,0),(-step,0),(0,step),(0,-step),(step,step),(-step,-step),(step,-step),(-step,step)):
            nz=free_z(x+dx,y+dy)
            if nz<best[2]-1e-4: best=(x+dx,y+dy,nz)
        if best==(x,y,z):
            if step<0.02: break
            step/=2; continue
        x,y,z=best
    return x,y,z
print("start (x0,y0) -> rest (x,y,z)   [seat: x=0, z=+0.42; y free along groove]")
for x0,y0 in ((0,-9),(20,-9),(-30,-9),(34,-9),(10,10),(-15,-30),(25,-35)):
    x,y,z=settle(float(x0),float(y0))
    ok = abs(x)<0.35 and abs(z-0.42)<0.35
    print("  (%6.1f,%6.1f) -> (%6.2f,%6.2f,%6.2f)  %s"%(x0,y0,x,y,z,"SEATED" if ok else "NOT SEATED"))
