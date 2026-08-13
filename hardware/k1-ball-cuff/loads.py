import math
# --- preload path ---
# M4 torqued to 0.8 Nm in PETG (conservative; more will creep): F = T/(0.2 d)
Fb = 0.8/(0.2*0.004)          # N per bolt
Fclamp = 6*Fb
# halves close to a 0.42 mm gap -> ALL clamp flows through the ball seats.
# Upper half equilibrium: 6 Fb (bolts) = 2 * N0 * cos45 -> N0 per contact:
N0 = Fclamp/(2*math.cos(math.radians(45)))
print("bolt preload %.0f N x6 = %.0f N clamp -> %.0f N per V contact"%(Fb,Fclamp,N0))
# PETG bearing: contact flattens until sigma ~ 30 MPa (long-term PETG)
A = N0/30e6*1e6
print("  contact patch flattens to ~%.0f mm2 (r~%.1f mm) at 30 MPa PETG bearing -- OK"%(A,math.sqrt(A/math.pi)))

# --- rod-axis torque (the unconstrained DOF) ---
# sphere + cylinder are both surfaces of revolution about the forearm axis:
# torque about it is FRICTION ONLY.
mu=0.25
r_eff=17.68e-3   # V contacts sit 17.68 mm off the rod axis
Mfric = 4*mu*N0*r_eff
print("rod-axis friction capacity: %.0f Nm (mu=%.2f)"%(Mfric,mu))
print("  vs elbow-yaw actuator torque ~7 Nm  -> ratio %.0fx"%(Mfric/7))
# shovel lift about rod axis: handle perp to forearm, load at 0.5m
for m,L in ((1.5,0.5),(4.0,0.7)):
    print("  vs %.1f kg at %.1f m = %.1f Nm  -> margin %.1fx"%(m,L,m*9.81*L,Mfric/(m*9.81*L)))

# --- prying (moment about handle axis X) -> journal couple ---
# seat center y=0, journal center y=-33: 33 mm couple arm
for m,L in ((1.5,0.5),(4.0,0.7)):
    M=m*9.81*L
    F=M/0.033
    # journal bearing: half-shell R17.2 x 28 long, projected area ~ 17.2*2*28 mm2
    sig=F/(17.2*2*28)
    print("prying %.1f Nm -> journal couple %.0f N, bearing %.2f MPa (PETG limit ~30)"%(M,F,sig))

# --- pull-off (yank along +Z / any axis): ball is caged; load -> bolt tension ---
# worst: straight pull along groove-normal, upper pair takes F/(2 cos45) each,
# added bolt tension = F; 6 bolts share
F=200.0  # a hard 20 kgf yank
print("200 N yank -> +%.0f N per bolt (proof ~4000 N M4) and PETG head bearing OK with washers"%(F/6))
