#!/usr/bin/env bash
# 组装提交包 dist/submit/(含源码包、文档、PPT、视频草稿、评测材料)
set -euo pipefail
REPO=$(cd "$(dirname "$0")/.." && pwd)          # lymem 仓根
BM=$(cd "$REPO/.." && pwd)/benchmark             # benchmark 仓根
OUT="$REPO/dist/submit"
TOOLS="$REPO/packaging/submit-tools"

cd "$OUT"
# 1) 生成 PPT / 技术方案 docx+pdf / 视频草稿(可重复执行)
export NODE_PATH="${NODE_PATH:-$(npm root -g)}"
node "$TOOLS/gen_ppt.js"
python3 "$TOOLS/build_submit_docs.py"
python3 "$TOOLS/build_video_draft.py"

# 2) 源码包(git archive:干净工作树,不含 dist/构建产物)
mkdir -p "$OUT/源代码"
git -C "$REPO" archive --format=tar.gz --prefix="lymem-src/" -o "$OUT/源代码/lymem-源码.tar.gz" HEAD
git -C "$BM"   archive --format=tar.gz --prefix="benchmark-src/" -o "$OUT/源代码/benchmark-评测源码.tar.gz" HEAD

# 3) 评测材料汇总
mkdir -p 评测材料
cp "$BM/results/REPORT.md" 评测材料/评测总报告-v4.md
cp "$BM/results/AGENT_EVAL.md" "$BM/results/PERF.md" 评测材料/
cp "$REPO/docs/适配测试报告.md" "$REPO/docs/设计文档.md" 评测材料/ 2>/dev/null || true

# 4) 目录归位
mkdir -p "1-项目报告PPT" "2-技术方案及测试结果" "4-效果演示视频"
mv -f "项目报告-麟忆尽智lymem.pptx" 1-项目报告PPT/ 2>/dev/null || true
mv -f "技术方案及测试结果-麟忆尽智lymem.docx" "技术方案及测试结果-麟忆尽智lymem.pdf" 2-技术方案及测试结果/ 2>/dev/null || true
mv -f video-materials/演示视频-静帧草稿.mp4 4-效果演示视频/ 2>/dev/null || true
rm -f _submit.html

echo "提交包就绪: $OUT"
find "$OUT" -maxdepth 2 -not -path '*/assets*' -not -name 'assets' | sort
