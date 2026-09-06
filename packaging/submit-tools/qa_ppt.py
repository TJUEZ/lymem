#!/usr/bin/env python3
# PPT Code QA:文本溢出/越界/重叠检查(pptx 技能 §10)
import sys
from pptx import Presentation
from pptx.util import Emu

PATH = sys.argv[1] if len(sys.argv) > 1 else "项目报告-麟忆尽智lymem.pptx"
prs = Presentation(PATH)
SW, SH = prs.slide_width, prs.slide_height
EMU_IN = 914400

def walk(shapes):
    for sp in shapes:
        if sp.shape_type == 6:
            yield from walk(sp.shapes)
        else:
            yield sp

def est_lines(text, font_pt, width_in):
    # CJK 为主:每字宽 ≈ font_pt/72 inch;中英混合按 0.72 折算
    if not text: return 0
    per_line = max(1, int(width_in / (font_pt / 72 * 0.95)))
    lines = 0
    for para in text.split("\n"):
        lines += max(1, -(-len(para) // per_line))
    return lines

issues = 0
for idx, slide in enumerate(prs.slides, 1):
    boxes = []
    for sp in walk(slide.shapes):
        try:
            l, t, w, h = sp.left, sp.top, sp.width, sp.height
        except Exception:
            continue
        if l is None: continue
        # 越界
        if l < -9525 or t < -9525 or l + w > SW + 9525 or t + h > SH + 9525:
            print(f"[越界] S{idx} {sp.shape_type} name={sp.name!r} box=({l/EMU_IN:.2f},{t/EMU_IN:.2f},{w/EMU_IN:.2f},{h/EMU_IN:.2f})")
            issues += 1
        # 溢出估算
        if sp.has_text_frame:
            txt = "\n".join(p.text for p in sp.text_frame.paragraphs)
            if not txt.strip(): continue
            sizes = [r.font.size.pt for p in sp.text_frame.paragraphs for r in p.runs if r.font.size]
            fpt = max(sizes) if sizes else 14
            lines = est_lines(txt, fpt, w / EMU_IN)
            need_h = lines * fpt / 72 * 1.28
            if need_h > h / EMU_IN * 1.22 and h > 0.2 * EMU_IN:
                print(f"[溢出?] S{idx} {txt[:22]!r}… font={fpt:.0f}pt lines≈{lines} need≈{need_h:.2f}in box_h={h/EMU_IN:.2f}in")
                issues += 1
            boxes.append((l, t, w, h, txt[:12]))
    # 重叠(文本框两两相交,面积交叠 >30% 且都非空)
    for i in range(len(boxes)):
        for j in range(i + 1, len(boxes)):
            a, b = boxes[i], boxes[j]
            ox = max(0, min(a[0]+a[2], b[0]+b[2]) - max(a[0], b[0]))
            oy = max(0, min(a[1]+a[3], b[1]+b[3]) - max(a[1], b[1]))
            inter = ox * oy
            small = min(a[2]*a[3], b[2]*b[3])
            if inter > 0 and small and inter / small > 0.45 and a[4] and b[4]:
                print(f"[重叠] S{idx} {a[4]!r} × {b[4]!r} 交叠 {inter/small:.0%}")
                issues += 1
print(f"\n共 {len(prs.slides.__iter__.__self__._sldIdLst)} 页,问题 {issues} 处" if issues else f"\n共 {len(list(prs.slides))} 页,无问题 ✓")
