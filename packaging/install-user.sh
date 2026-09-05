#!/usr/bin/env bash
# 麟忆尽智 · 免 root 用户级安装（麒麟/Linux 桌面）
# 用法: ./install-user.sh [--no-autostart]
set -euo pipefail
SRC=$(cd "$(dirname "$0")" && pwd)

LIB_DIR="$HOME/.local/lib/lymem"
BIN_DIR="$HOME/.local/bin"
ICON_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"
APP_DIR="$HOME/.local/share/applications"
AUTOSTART_DIR="$HOME/.config/autostart"
CONF_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/lymem"

AUTOSTART=1
[ "${1:-}" = "--no-autostart" ] && AUTOSTART=0

mkdir -p "$LIB_DIR" "$BIN_DIR" "$ICON_DIR" "$APP_DIR" "$CONF_DIR" "$AUTOSTART_DIR"

install -m 0755 "$SRC/lymem-server" "$LIB_DIR/lymem-server"
install -m 0755 "$SRC/lymem-ctl"    "$LIB_DIR/lymem-ctl"
install -m 0644 "$SRC/icon.svg"     "$ICON_DIR/lymem.svg"
[ -f "$SRC/env.example" ] && [ ! -f "$CONF_DIR/env" ] && install -m 0644 "$SRC/env.example" "$CONF_DIR/env"

ln -sf "$LIB_DIR/lymem-ctl" "$BIN_DIR/lymem"

cat > "$APP_DIR/lymem.desktop" <<EOF
[Desktop Entry]
Type=Application
Version=1.0
Name=麟忆尽智
GenericName=Agent 记忆管理台
Comment=端侧长期记忆：灌入、检索、偏好、冲突仲裁、遗忘与评测
Exec=$LIB_DIR/lymem-ctl open
Icon=lymem
Terminal=false
Categories=Utility;System;Office;
StartupNotify=false
EOF

if [ "$AUTOSTART" = 1 ]; then
cat > "$AUTOSTART_DIR/lymem-autostart.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=麟忆尽智 后台服务
Exec=$LIB_DIR/lymem-ctl start
Icon=lymem
Terminal=false
X-GNOME-Autostart-enabled=true
EOF
fi

update-desktop-database "$APP_DIR" >/dev/null 2>&1 || true

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) echo "提示: 建议把 $BIN_DIR 加入 PATH 以便终端使用 lymem 命令" ;;
esac

echo "安装完成:"
echo "  · 应用菜单 → 「麟忆尽智」：启动服务并打开管理台"
echo "  · 终端命令: ~/.local/bin/lymem open   （或 start/stop/status/logs）"
echo "  · 配置文件: $CONF_DIR/env   （可填 LLM Key，解锁问答生成与偏好提取）"
echo "  · 数据目录: ~/.local/share/lymem （备份整目录即可迁移）"
$LIB_DIR/lymem-ctl start || true
