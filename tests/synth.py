"""Synthetic Panda fiber end faces with known axes, for testing.

style "mixed": faint thin rod outlines, shading patches, dust, bubbles,
streaks, debris, a dirty rim, neighbours touching and running off the frame.
style "relief": like an obliquely lit end face: embossed rods (a bright half
and a dark crescent rim on the far side), one side of the cladding fading
into dark shading with a soft edge, a glow outside the rim, a thick black
dirt streak along a rod, debris clusters, the fiber off the middle.
"""

import math

import cv2
import numpy as np


def _blur(m, s):
    return cv2.GaussianBlur(m, (0, 0), s) if s > 0 else m


def _dirt(rng, h, w, fx, fy, R, d, streak_near=None):
    dirt = np.zeros((h, w), np.float32)

    def spot(spread=0.88):
        t = rng.uniform(0, 2 * np.pi)
        q = R * math.sqrt(rng.uniform(0, spread))
        return fx + q * math.cos(t), fy + q * math.sin(t)

    for _ in range(int(rng.integers(5, 40) * d)):  # dust specks
        x, y = spot()
        cv2.circle(dirt, (int(x), int(y)), int(rng.uniform(2, 9)), float(rng.uniform(0.3, 0.9)), -1)
    for _ in range(int(rng.integers(0, 25) * d)):  # bubbles
        x, y = spot()
        br = rng.uniform(4, 14)
        cv2.circle(dirt, (int(x), int(y)), int(br), 0.8, 2)
        cv2.circle(dirt, (int(x), int(y)), max(1, int(br * 0.4)), -0.6, -1)
    for _ in range(int(rng.integers(1, 4) * d)):  # debris clusters
        x0, y0 = spot()
        for _ in range(rng.integers(20, 70)):
            cv2.ellipse(dirt, (int(x0 + rng.normal(0, 45)), int(y0 + rng.normal(0, 45))),
                        (int(rng.uniform(2, 7)), int(rng.uniform(1, 4))), rng.uniform(0, 180), 0, 360,
                        float(rng.uniform(0.3, 0.8)), -1)
    streaks = int(rng.integers(0, 3) * d)
    for k in range(streaks + (1 if streak_near is not None else 0)):
        if k == 0 and streak_near is not None:  # thick streak hugging a rod's outer side
            x, y, r, out = streak_near
            t0 = out + rng.uniform(-0.6, 0.6)
            pts = [(x + (r + 25) * math.cos(t0 + s), y + (r + 25) * math.sin(t0 + s)) for s in np.linspace(-0.5, 0.5, 8)]
            width = int(rng.uniform(10, 22))
        else:
            x, y = spot()
            pts = [(x, y)]
            t = rng.uniform(0, 2 * np.pi)
            for _ in range(rng.integers(5, 15)):
                t += rng.uniform(-0.6, 0.6)
                x, y = x + 12 * math.cos(t), y + 12 * math.sin(t)
                pts.append((x, y))
            width = int(rng.uniform(3, 9))
        cv2.polylines(dirt, [np.int32(pts)], False, 1.0, width)
    if rng.random() < 0.5 * d:  # dirt on the rim
        t = rng.uniform(0, 2 * np.pi)
        cv2.ellipse(dirt, (int(fx + R * math.cos(t)), int(fy + R * math.sin(t))), (int(R * 0.15), int(R * 0.05)),
                    math.degrees(t) + 90, 0, 360, 0.9, -1)
    return _blur(dirt, 1.0)


def make(seed: int, difficulty: float = 1.0, style: str = "mixed", w: int = 1920, h: int = 1080):
    """(image, truth): truth = dict(angle=deg -90..90, cx, cy, R, rods=[(x, y, r), ...])."""
    rng = np.random.default_rng(seed + (100000 if style == "relief" else 0))
    d = difficulty
    yy, xx = np.mgrid[:h, :w].astype(np.float32)
    R = rng.uniform(300, 370)
    if style == "relief":
        cx, cy = w / 2 + rng.uniform(-250, 250), h / 2 + rng.uniform(-40, 40)
        fibers = [(cx, cy, 1.0)]
    else:
        layout = rng.choice(["single", "single", "vgroove", "pair"])
        cx, cy = w / 2 + rng.uniform(-150, 150), h / 2 + rng.uniform(-60, 60)
        if layout == "pair":
            cx = w / 2 + rng.choice([-1, 1]) * rng.uniform(0, 0.4) * R
        fibers = [(cx, cy, 1.0)]
        if layout == "vgroove":
            fibers += [(cx - 2 * R, cy + rng.uniform(-20, 20), rng.uniform(0.4, 0.8)),
                       (cx + 2 * R, cy + rng.uniform(-20, 20), rng.uniform(0.3, 0.7))]
        elif layout == "pair":
            side = 1 if cx < w / 2 else -1
            fibers += [(cx + side * 2 * R, cy + rng.uniform(-15, 15), rng.uniform(0.7, 1.0))]

    img = np.full((h, w), rng.uniform(3, 15), np.float32)
    truth = None
    light = rng.uniform(0, 2 * np.pi)  # direction of the oblique light (relief)
    lx, ly = math.cos(light), math.sin(light)
    for k, (fx, fy, lit) in enumerate(fibers):
        level = rng.uniform(150, 215) * lit if style == "relief" else rng.uniform(110, 215) * lit
        angle = rng.uniform(-8, 8) if rng.random() < 0.7 else rng.uniform(-90, 90)
        a = math.radians(angle)
        s = R * rng.uniform(0.44, 0.54)
        rr = R * rng.uniform(0.25, 0.31)
        core = (fx + rng.uniform(-6, 6), fy + rng.uniform(-6, 6))
        rods = [(core[0] + sg * s * math.cos(a), core[1] - sg * s * math.sin(a), rr) for sg in (1, -1)]
        dist = np.hypot(xx - fx, yy - fy)
        clad = np.full((h, w), level, np.float32)
        if style == "relief":
            # One side fades into shade, softening the rim there.
            side = rng.uniform(0, 2 * np.pi)
            u = ((xx - fx) * math.cos(side) + (yy - fy) * math.sin(side)) / R
            ramp = np.clip((u - rng.uniform(-0.2, 0.3)) / 0.8, 0, 1)
            shade = ramp * ramp * (3 - 2 * ramp) * rng.uniform(0.4, 0.8) * d  # smooth
            clad *= np.clip(1 - shade, 0.2, 1)
            edge_soft = 4 + 30 * np.clip(u, 0, 1) * d
            disc = np.clip(0.5 + (R - dist) / (2 * edge_soft), 0, 1)
        else:
            tilt = rng.uniform(-0.15, 0.15) * (xx - fx) / R + rng.uniform(-0.15, 0.15) * (yy - fy) / R
            clad *= 1 + tilt - 0.12 * (dist / R) ** 2
            disc = np.clip(R - dist + 0.5, 0, 1)
            for _ in range(rng.integers(0, 3)):  # broad shading patches
                bx, by, _ = rods[rng.integers(0, 2)] if rng.random() < 0.6 else (
                    fx + rng.uniform(-0.6, 0.6) * R, fy + rng.uniform(-0.6, 0.6) * R, 0)
                br = R * rng.uniform(0.15, 0.35)
                blob = np.zeros((h, w), np.float32)
                cv2.circle(blob, (int(bx + rng.uniform(-0.2, 0.2) * R), int(by + rng.uniform(-0.2, 0.2) * R)), int(br), 1.0, -1)
                clad *= 1 - rng.uniform(0.2, 0.6) * d * _blur(blob, br * 0.6)
        for x, y, r in rods:
            rd = np.hypot(xx - x, yy - y)
            inside = np.clip(r - rd + 0.5, 0, 1)
            if style == "relief":
                # Embossed: brighter towards the light inside, a dark crescent rim on the far side.
                facing = ((xx - x) * lx + (yy - y) * ly) / np.maximum(rd, 1)
                rim = np.exp(-0.5 * ((rd - r) / rng.uniform(3, 7)) ** 2)
                clad += level * rng.uniform(0.03, 0.1) * inside * np.clip(-facing, 0, 1)
                clad -= level * rng.uniform(0.2, 0.45) * rim * np.clip(facing, 0, 1) ** rng.uniform(0.7, 2)
                clad += level * rng.uniform(0.05, 0.15) * rim * np.clip(-facing, 0, 1)
            else:
                clad *= 1 + rng.uniform(-0.08, 0.05) * inside
                line = np.exp(-0.5 * ((rd - r) / rng.uniform(1.0, 3.0)) ** 2)
                clad += rng.choice([-1, 1]) * rng.uniform(0.04, 0.15) * level * line
                if rng.random() < 0.6:
                    ca = rng.uniform(0, 2 * np.pi)
                    cres = inside * np.clip(((xx - x) * math.cos(ca) + (yy - y) * math.sin(ca)) / r - 0.3, 0, 1)
                    clad += rng.choice([-1, 1]) * rng.uniform(0.1, 0.3) * d * level * _blur(cres, 3)
        if rng.random() < 0.4:  # small core spot
            cd = np.hypot(xx - core[0], yy - core[1])
            clad -= 0.12 * level * np.clip(0.03 * R - cd, 0, 1)
        img = img * (1 - disc) + clad * disc
        if style == "relief":
            # A soft glow outside the rim on one side.
            g = rng.uniform(0, 2 * np.pi)
            gx_, gy_ = fx + (R + rng.uniform(20, 80)) * math.cos(g), fy + (R + rng.uniform(20, 80)) * math.sin(g)
            glow = np.exp(-0.5 * (((xx - gx_) / rng.uniform(60, 160)) ** 2 + ((yy - gy_) / rng.uniform(30, 60)) ** 2))
            img += glow * rng.uniform(30, 110) * (1 - disc)
        else:
            rim = np.exp(-0.5 * ((dist - R) / 4) ** 2)
            img -= 0.5 * level * rim * disc
            halo = np.exp(-0.5 * ((dist - R - 10) / 8) ** 2) * (rng.random((h, w)) < 0.15 * d)
            img += _blur(halo.astype(np.float32), 2) * level * 2.5
        if k == 0:
            truth = dict(angle=((angle + 90) % 180) - 90, cx=fx, cy=fy, R=R, rods=rods)

    fx, fy = fibers[0][0], fibers[0][1]
    near = None
    if style == "relief" and rng.random() < 0.8:
        x, y, r = truth["rods"][rng.integers(0, 2)]
        near = (x, y, r, math.atan2(y - fy, x - fx))
    level = float(np.median(img[np.hypot(xx - fx, yy - fy) < R * 0.7]))
    img -= _dirt(rng, h, w, fx, fy, R, d, near) * level * 0.85
    if style != "relief" and rng.random() < 0.3:
        t = rng.uniform(0, np.pi)
        img += 4 * np.sin((xx * math.cos(t) + yy * math.sin(t)) * 2 * np.pi / rng.uniform(8, 20))
    img = _blur(img, rng.uniform(0.6, 2.0))
    img += rng.normal(0, rng.uniform(2, 9), img.shape).astype(np.float32)
    if style == "relief" and rng.random() < 0.6:  # sharpened, like a camera's processing
        img = img + 0.8 * (img - _blur(img, 2.0))
    if style == "relief" and rng.random() < 0.6:  # contrast pushed up: cladding near white
        img = np.clip(img - 20, 0, None)  # black level clipped first, as a camera does
        img = 255 * np.clip(img / 235, 0, 1) ** rng.uniform(0.6, 0.85) * rng.uniform(1.0, 1.25)
    img = np.clip(img, 0, 255).astype(np.uint8)
    return cv2.cvtColor(img, cv2.COLOR_GRAY2BGR), truth
