#!/usr/bin/env bash
# 麟忆尽智 · 用户级卸载（保留数据目录 ~/.local/share/lymem）
set -u
"$HOME/.local/lib/lymem/lymem-ctl" stop 2>/dev/null || pkill -u "$USER" -f lymem-server 2>/dev/null || true
rm -f "$HOME/.local/lib/lymem/lymem-server" \
      "$HOME/.local/lib/lymem/lymem-cli" \
      "$HOME/.local/lib/lymem/lymem-ctl" \
      "$HOME/.local/bin/lymem" \
      "$HOME/.local/share/applications/lymem.desktop" \
      "$HOME/.config/autostart/lymem-autostart.desktop" \
      "$HOME/.local/share/icons/hicolor/scalable/apps/lymem.svg"
rmdir "$HOME/.local/lib/lymem" 2>/dev/null || true
echo "已卸载（数据与配置保留: ~/.local/share/lymem、~/.config/lymem/env）"
