#!/usr/bin/env bash
# 一键构建发布产物：lymem-server release 二进制 → deb 包 + 免 root 便携包
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PKG="$ROOT/packaging"

# shellcheck disable=SC1091
source "$HOME/.cargo/env"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/../lymem-target}"
VER=$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n1)

echo "==> 构建 lymem-server + lymem-cli $VER（release）"
cargo build --release -p lymem-server -p lymem-cli
BIN="$CARGO_TARGET_DIR/release/lymem-server"
CLI_BIN="$CARGO_TARGET_DIR/release/lymem"
strip "$BIN" 2>/dev/null || echo "（无 strip，跳过）"
strip "$CLI_BIN" 2>/dev/null || true

OUT="$ROOT/dist"
rm -rf "$OUT"; mkdir -p "$OUT"

echo "==> 组装免 root 便携包"
NAME="lymem-${VER}-linux-amd64"
STAGE="$OUT/$NAME"; mkdir -p "$STAGE"
install -m 0755 "$BIN"                 "$STAGE/lymem-server"
install -m 0755 "$CLI_BIN"            "$STAGE/lymem-cli"
install -m 0755 "$PKG/lymem-ctl"       "$STAGE/lymem-ctl"
install -m 0755 "$PKG/install-user.sh" "$STAGE/install-user.sh"
install -m 0755 "$PKG/uninstall-user.sh" "$STAGE/uninstall-user.sh"
install -m 0644 "$PKG/icon.svg"        "$STAGE/icon.svg"
install -m 0644 "$PKG/env.example"     "$STAGE/env.example"
install -m 0644 "$PKG/README.md"       "$STAGE/README.md"
tar -C "$OUT" -czf "$OUT/${NAME}.tar.gz" "$NAME"
rm -rf "$STAGE"

echo "==> 组装 deb 包"
FS="$OUT/.debroot"
rm -rf "$FS"
mkdir -p "$FS/opt/lymem" "$FS/etc/lymem" "$FS/etc/xdg/autostart" \
         "$FS/usr/share/applications" "$FS/usr/share/icons/hicolor/scalable/apps" \
         "$FS/usr/share/doc/lymem"
install -m 0755 "$BIN"           "$FS/opt/lymem/lymem-server"
install -m 0755 "$CLI_BIN"      "$FS/opt/lymem/lymem-cli"
install -m 0755 "$PKG/lymem-ctl" "$FS/opt/lymem/lymem-ctl"
install -m 0644 "$PKG/env.example" "$FS/etc/lymem/env"
install -m 0644 "$PKG/icon.svg"    "$FS/usr/share/icons/hicolor/scalable/apps/lymem.svg"
install -m 0644 "$PKG/README.md"   "$FS/usr/share/doc/lymem/README.md"

cat > "$FS/usr/share/applications/lymem.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Version=1.0
Name=麟忆尽智
GenericName=Agent 记忆管理台
Comment=端侧长期记忆：灌入、检索、偏好、冲突仲裁、遗忘与评测
Exec=/opt/lymem/lymem-ctl open
Icon=lymem
Terminal=false
Categories=Utility;System;Office;
StartupNotify=false
EOF

cat > "$FS/etc/xdg/autostart/lymem-autostart.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=麟忆尽智 后台服务
Exec=/opt/lymem/lymem-ctl start
Icon=lymem
Terminal=false
X-GNOME-Autostart-enabled=true
EOF

SIZE=$(du -sk "$FS" | cut -f1)
mkdir -p "$FS/DEBIAN"
sed -e "s/PLACEHOLDER_VERSION/$VER/" "$PKG/deb/control" \
  | sed -e "s/^Maintainer:.*/&\nInstalled-Size: $SIZE/" > "$FS/DEBIAN/control"
install -m 0755 "$PKG/deb/postinst" "$FS/DEBIAN/postinst"
install -m 0755 "$PKG/deb/prerm"    "$FS/DEBIAN/prerm"
install -m 0644 "$PKG/deb/conffiles" "$FS/DEBIAN/conffiles"

dpkg-deb --build -Zxz "$FS" "$OUT/lymem_${VER}_amd64.deb"
rm -rf "$FS"

echo "==> 产物:"
ls -lh "$OUT"

# 若仓库外存在赛题交付目录，同步可直接安装的发行包。
MATERIALS="$ROOT/../提交材料"
FINAL="$MATERIALS/最终交付物"
if [ -d "$MATERIALS" ]; then
  mkdir -p "$FINAL/3-安装包"
  cp -f "$OUT/${NAME}.tar.gz" "$FINAL/3-安装包/"
  cp -f "$OUT/lymem_${VER}_amd64.deb" "$FINAL/3-安装包/"
  {
    echo "lymem 安装包"
    echo "版本: $VER"
    echo "便携包: ${NAME}.tar.gz"
    echo "deb 包: lymem_${VER}_amd64.deb"
    echo "正式环境默认使用银河麒麟向量后端；无运行时时可在 env 中设置 LYMEM_EMBEDDER=hash 进行离线演示。"
  } > "$FINAL/3-安装包/README.txt"
  {
    echo "lymem 最终交付物清单"
    echo "更新日期: $(date '+%Y-%m-%d %H:%M:%S %z')"
    echo
    find "$FINAL" -maxdepth 3 -type f -printf '%P\t%k KB\n' | sort
  } > "$FINAL/提交清单.txt"
fi
