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
    <!-- macOS 15+ 的「本地网络」隐私保护：飞牛影视服务器通常是 192.168.x.x /
         .local 这类局域网地址，缺少这段说明系统会静默拒绝连接（表现为「连不上
         网络」）。有了它首次访问局域网时才会弹出授权弹窗。 -->
    <key>NSLocalNetworkUsageDescription</key>
    <string>需要访问局域网内的飞牛影视服务器，以读取影视库与剧集信息。</string>
    <!-- 放宽 ATS：飞牛接口是明文 http。Rust 原生 socket 本身不受 ATS 约束，
         这里一并声明，避免将来接入系统网络栈时被拦。 -->
    <key>NSAppTransportSecurity</key>
    <dict>
        <key>NSAllowsArbitraryLoads</key>
        <true/>
        <key>NSAllowsLocalNetworking</key>
        <true/>
    </dict>
</dict>
</plist>
PLIST

echo "==> 3.1 对 .app 做 ad-hoc 签名（关键步骤）..."
# ---------------------------------------------------------------------------
# 为什么必须签名：
# `cargo build` 产出的 Mach-O 只带 linker 的临时 ad-hoc 签名，直接拷进 .app 后
# codesign 会看到 `Identifier=bili_planner-<hash>`、`Info.plist=not bound`、
# `Sealed Resources=none`。此时 Info.plist 不在签名封装内，TCC 读不到
# NSLocalNetworkUsageDescription，macOS 15+ 会**静默拒绝**该 App 访问局域网，
# 于是飞牛服务器永远连不上（而 `cargo run` 走的是终端已授权的本地网络权限，
# 表现正常）。
# 重新签名后标识变为 CFBundleIdentifier、Info.plist 被绑定、资源被 seal，
# 授权弹窗才会正常出现且授予结果能持久化（不会因为每次重新构建而失效）。
# ---------------------------------------------------------------------------
rm -rf "${APP_DIR}/Contents/_CodeSignature"
codesign --force --deep --sign - --timestamp=none "${APP_DIR}"
codesign --verify --deep --strict --verbose=2 "${APP_DIR}" 2>&1 | tail -3

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
