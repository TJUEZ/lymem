#!/usr/bin/env python3
# 组装《技术方案及测试结果》:设计文档 + 评测总报告(一~十) + 适配测试报告 + 用户手册
# 产出:docx(pandoc) + pdf(HTML → Edge print-to-pdf)
import re, subprocess, pathlib

ROOT = pathlib.Path(__file__).resolve().parents[2]
BM = ROOT.parent / "benchmark"
OUT = ROOT / "dist" / "submit"
OUT.mkdir(parents=True, exist_ok=True)

def demote(md: str) -> str:
    """全部标题降一级(#→##),便于挂在部分标题之下。"""
    return re.sub(r"(?m)^#", "#", md)

def report_body() -> str:
    """评测总报告:跳过迭代日志节(〇一 v3 增量 / 〇 质量修正),保留 一~十。"""
    md = (BM / "results" / "REPORT.md").read_text(encoding="utf-8")
    lines = md.splitlines()
    # 找到 "## 一、" 的起始,丢弃其之前除主标题外的内容
    start = next(i for i, l in enumerate(lines) if l.startswith("## 一、"))
    title = lines[0].replace("评测总报告 v4", "评测总报告(效果验证)v4")
    return title + "\n\n" + "\n".join(lines[start:])

parts = [
    ("第一部分 · 技术方案(需求分析 / 软件架构 / 核心算法设计 / 技术实现)", (ROOT / "docs" / "设计文档.md").read_text(encoding="utf-8")),
    ("第二部分 · 效果验证报告(测试数据集 / 对比实验设计 / 量化指标分析)", report_body()),
    ("第三部分 · 银河麒麟桌面操作系统适配测试报告", (ROOT / "docs" / "适配测试报告.md").read_text(encoding="utf-8")),
    ("第四部分 · 安装部署指南与操作说明(用户手册)", (ROOT / "packaging" / "README.md").read_text(encoding="utf-8")),
]

cover = """% 麟忆尽智 lymem · 技术方案及测试结果
% 面向银河麒麟桌面的 OS Agent 记忆优化及高效应用(选题 XA-202612)

::: {.center}
**版本 0.1.0 · 2026-09**

提报单位:(学校全称)  团队成员:(待填写)  指导教师:(待填写)
:::

\\newpage

"""

body = cover
for title, md in parts:
    body += f"\n\n# {title}\n\n" + demote(md)

master = OUT / "技术方案及测试结果-麟忆尽智lymem.md"
master.write_text(body, encoding="utf-8")

# ---- docx(pandoc) ----
import pypandoc
pypandoc.convert_file(str(master), "docx", outputfile=str(OUT / "技术方案及测试结果-麟忆尽智lymem.docx"),
                      extra_args=["--toc", "--toc-depth=2", "-V", "lang=zh-CN"])
print("docx ✓")

# ---- HTML → pdf(Edge print-to-pdf) ----
css = """
body{font-family:'Noto Sans CJK SC','WenQuanYi Micro Hei',sans-serif;font-size:11.5pt;line-height:1.65;color:#1a1a1a;max-width:170mm;margin:0 auto;padding:12mm 6mm;}
h1{font-size:19pt;border-bottom:2px solid #1E3A5F;padding-bottom:6px;margin-top:28px;}
h2{font-size:15pt;color:#1E3A5F;margin-top:22px;}
h3{font-size:12.5pt;margin-top:16px;}
table{border-collapse:collapse;width:100%;font-size:9.5pt;margin:10px 0;}
th,td{border:1px solid #c9d4e6;padding:4px 7px;text-align:left;vertical-align:top;}
th{background:#EEF3FB;}
code,pre{font-family:monospace;font-size:9pt;background:#f5f7fa;}
pre{padding:8px;white-space:pre-wrap;}
blockquote{border-left:3px solid #2563EB;margin:8px 0;padding:2px 12px;color:#475569;}
"""
html_body = pypandoc.convert_file(str(master), "html", extra_args=["--toc", "--toc-depth=2", "--standalone", f"--css=/dev/null", "--metadata", "title=技术方案及测试结果"])
html_body = html_body.replace("</head>", f"<style>{css}</style></head>")
html_path = OUT / "_submit.html"
html_path.write_text(html_body, encoding="utf-8")
subprocess.run(["microsoft-edge", "--headless", "--disable-gpu", "--no-pdf-header-footer",
                f"--print-to-pdf={OUT}/技术方案及测试结果-麟忆尽智lymem.pdf", str(html_path)], check=True, timeout=180)
print("pdf ✓")
