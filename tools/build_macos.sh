#!/usr/bin/env bash
# ==============================================================================
# bili-planner macOS 打包脚本：构建 .app 应用包与 .dmg 安装镜像
# 依赖环境：macOS 原生工具 (hdiutil, iconutil) + cargo + python3
# ==============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

cd "${ROOT_DIR}"

APP_NAME="bili-planner"
VERSION=$(grep -m1 '^version' Cargo.toml | awk -F '"' '{print $2}')
BUNDLE_ID="com.example.bili-planner"

echo "==> 1. 构建 Release 二进制文件..."
cargo build --release --bin "${APP_NAME}"

echo "==> 2. 检查并生成图标 (.icns)..."
if [ ! -f "icons/icon.icns" ]; then
    echo "    未检测到 icons/icon.icns，正在生成..."
    python3 tools/gen_icons.py
fi

# 输出目录
DIST_DIR="${ROOT_DIR}/target/release/bundle/osx"
APP_DIR="${DIST_DIR}/${APP_NAME}.app"
DMG_STAGING="${DIST_DIR}/dmg_staging"
DMG_FILE="${DIST_DIR}/${APP_NAME}_${VERSION}_aarch64.dmg"

echo "==> 3. 组装 ${APP_NAME}.app 包结构..."
rm -rf "${APP_DIR}" "${DMG_STAGING}" "${DMG_FILE}"
mkdir -p "${APP_DIR}/Contents/MacOS"
mkdir -p "${APP_DIR}/Contents/Resources"

# 复制二进制与图标
cp "target/release/${APP_NAME}" "${APP_DIR}/Contents/MacOS/${APP_NAME}"
chmod +x "${APP_DIR}/Contents/MacOS/${APP_NAME}"
if [ -f "icons/icon.icns" ]; then
    cp "icons/icon.icns" "${APP_DIR}/Contents/Resources/icon.icns"
fi

# 写入 Info.plist
cat <<PLIST > "${APP_DIR}/Contents/Info.plist"
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleDisplayName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleIdentifier</key>
    <string>${BUNDLE_ID}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleExecutable</key>
    <string>${APP_NAME}</string>
    <key>CFBundleIconFile</key>
    <string>icon.icns</string>
    <key>LSMinimumSystemVersion</key>
    <string>10.15</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key>
    <true/>
</dict>
</plist>
PLIST

echo "==> 已生成 .app: ${APP_DIR}"

echo "==> 4. 打包 .dmg 安装镜像..."
mkdir -p "${DMG_STAGING}"
cp -R "${APP_DIR}" "${DMG_STAGING}/"
ln -s /Applications "${DMG_STAGING}/Applications"

hdiutil create \
    -volname "${APP_NAME}" \
    -srcfolder "${DMG_STAGING}" \
    -ov \
    -format UDZO \
    "${DMG_FILE}"

rm -rf "${DMG_STAGING}"

echo "=============================================================================="
echo "🎉 打包完成！产物路径："
echo "   - 应用包 (.app): ${APP_DIR}"
echo "   - 安装包 (.dmg): ${DMG_FILE}"
echo "=============================================================================="
