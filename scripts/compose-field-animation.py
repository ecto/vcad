#!/usr/bin/env python3
"""Encode the bracket power-cycle animation.

  scripts/compose-field-animation.py FRAMES_DIR OUT_BASENAME

FRAMES_DIR/png/C_####.png are the composed frames FieldAnimationTests writes
(temperature | coupled von Mises, colorbars, caption — drawn in AppKit
because Homebrew's default ffmpeg has no drawtext). Writes OUT_BASENAME.mp4
(H.264) and OUT_BASENAME.gif. Needs ffmpeg.
"""
import os, subprocess, sys

d, out = sys.argv[1], sys.argv[2]
src = os.path.join(d, "png", "C_%04d.png")
subprocess.run(["ffmpeg", "-v", "error", "-y", "-framerate", "8", "-i", src, "-c:v", "libx264", "-pix_fmt", "yuv420p",
                "-crf", "20", "-movflags", "+faststart", out + ".mp4"], check=True)
subprocess.run(["ffmpeg", "-v", "error", "-y", "-framerate", "8", "-i", src, "-vf",
                "scale=960:-1:flags=lanczos,split[s0][s1];[s0]palettegen=max_colors=128[p];[s1][p]paletteuse=dither=bayer",
                "-loop", "0", out + ".gif"], check=True)
print(out + ".mp4", out + ".gif")
