#!/usr/bin/env python3
# 视频素材:图卡渲染(matplotlib) + 静帧草稿视频(ffmpeg concat)
import subprocess, pathlib, matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

OUT = pathlib.Path("/home/ez/桌面/kylin-mem/lymem/dist/submit/video-materials")
OUT.mkdir(parents=True, exist_ok=True)
ASSETS = pathlib.Path(__file__).resolve().parent / "assets"
from matplotlib import font_manager
for _fp in ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf"):
    try:
        font_manager.fontManager.addfont(_fp)
    except Exception:
        pass
plt.rcParams["font.sans-serif"] = ["Noto Sans CJK JP", "Droid Sans Fallback"]
plt.rcParams["axes.unicode_minus"] = False

DARK, ACCENT, AMBER, LIGHT, MUTED = "#152238", "#2563EB", "#FFD479", "#A8C0E8", "#7E96BC"

def card(path, draw, figsize=(12.8, 7.2)):
    fig = plt.figure(figsize=figsize, dpi=150)
    fig.patch.set_facecolor(DARK)
    ax = fig.add_axes([0, 0, 1, 1]); ax.set_facecolor(DARK); ax.axis("off")
    draw(ax)
    fig.savefig(path, facecolor=DARK); plt.close(fig)

def cover(ax):
    ax.text(0.5, 0.62, "麟忆尽智 lymem", ha="center", fontsize=64, color="white", weight="bold")
    ax.text(0.5, 0.50, "面向银河麒麟桌面的 OS Agent 端侧记忆系统", ha="center", fontsize=24, color=LIGHT)
    ax.text(0.5, 0.435, "多源融合灌入 · 四层记忆流转 · 冲突仲裁 · 经验复用 · 端侧轻量化", ha="center", fontsize=15, color=MUTED)
    stats = [("98.0%", "偏好提取准确率"), ("16.5ms", "检索延迟 p50"), ("+6.7pts", "Agent 任务提升"), ("100%", "功能离线可用")]
    for i, (n, l) in enumerate(stats):
        x = 0.14 + i * 0.19
        ax.text(x, 0.28, n, ha="center", fontsize=30, color=AMBER if i == 2 else "white", weight="bold")
        ax.text(x, 0.22, l, ha="center", fontsize=13, color=LIGHT)
    ax.text(0.5, 0.08, "挑战杯「揭榜挂帅」擂台赛 · 选题 XA-202612", ha="center", fontsize=12, color=MUTED)

def ending(ax):
    ax.text(0.5, 0.86, "端侧 · 离线 · 可验证的记忆底座", ha="center", fontsize=38, color="white", weight="bold")
    rows = [("偏好提取准确率", "≥85%", "98.0% ✅"), ("知识检索召回率(中文)", "≥85%", "96.1% ✅"),
            ("检索响应时间", "≤500ms", "16.5ms ✅"), ("知识冲突处理正确率", "≥88%", "93.3% ✅"),
            ("Agent 任务提升(经验复用)", "—", "+6.7pts ✅")]
    for i, (n, req, v) in enumerate(rows):
        y = 0.70 - i * 0.085
        ax.text(0.22, y, n, fontsize=16, color=LIGHT)
        ax.text(0.55, y, "要求 " + req, fontsize=16, color=MUTED, ha="center")
        ax.text(0.72, y, v, fontsize=16, color="#7EE2A8", weight="bold")
    ax.text(0.5, 0.10, "麟忆尽智 · 谢谢观看", ha="center", fontsize=24, color="white", weight="bold")

def arms(ax):
    ax.set_facecolor(DARK)
    ax.text(0.5, 0.90, "Agent 三臂对照:工具选择任务 EM(DSH + MiniMax-M3,n=30)", ha="center", fontsize=20, color="white")
    labels, vals, cols = ["裸 Agent", "+原始记忆检索", "+经验复用层"], [90.0, 86.7, 96.7], ["#3E5A8A", "#2A3F63", ACCENT]
    bars = ax.bar([0.18, 0.5, 0.82], vals, width=0.22, color=cols, tick_label=labels)
    for b, v in zip(bars, vals):
        ax.text(b.get_x() + b.get_width() / 2, v + 1.2, f"{v}%", ha="center", fontsize=22, color="white", weight="bold")
    ax.annotate("+6.7pts 零回退", xy=(0.82, 96.7), xytext=(0.60, 100.5), fontsize=18, color=AMBER,
                arrowprops=dict(arrowstyle="->", color=AMBER))
    ax.set_xlim(0, 1); ax.set_ylim(0, 108); ax.axis("off")

card(OUT / "f01-cover.png", cover)
card(OUT / "f10-ending.png", ending)
card(OUT / "f09b-arms.png", arms)

# ---- ffmpeg 静帧草稿:镜号→(图,时长s) ----
A = str(ASSETS)
seq = [
    (OUT / "f01-cover.png", 15), (ASSETS / "hub.png", 25), (ASSETS / "dash.png", 30),
    (ASSETS / "vault-ask.png", 40), (ASSETS / "prefs.png", 40), (ASSETS / "know.png", 50),
    (ASSETS / "forget-done.png", 40), (ASSETS / "arena.png", 30), (OUT / "f09b-arms.png", 40),
    (OUT / "f10-ending.png", 30),
]
lst = OUT / "concat.txt"
with lst.open("w") as f:
    for img, dur in seq:
        seg = OUT / ("seg_" + img.stem + ".mp4")
        subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-loop", "1", "-i", str(img),
                        "-t", str(dur), "-r", "30", "-vf", "scale=1920:1080:force_original_aspect_ratio=decrease,pad=1920:1080:(ow-iw)/2:(oh-ih)/2",
                        "-c:v", "libx264", "-pix_fmt", "yuv420p", str(seg)], check=True)
        f.write(f"file '{seg}'\n")
subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-f", "concat", "-safe", "0", "-i", str(lst),
                "-c", "copy", str(OUT / "演示视频-静帧草稿.mp4")], check=True)
for seg in OUT.glob("seg_*.mp4"):
    seg.unlink()
lst.unlink()
print("视频草稿 ✓", (OUT / "演示视频-静帧草稿.mp4").stat().st_size, "bytes")
