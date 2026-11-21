#!/usr/bin/env bash
# Собирает macOS-приложение .app из готового бинарника.
#
# Голый бинарник на macOS ведёт себя не как приложение: окно не получает
# фокус по клику из Finder, нет иконки в Dock, нельзя закрепить в панели.
# Бандл — это каталог `Albus.app` с `Info.plist` и бинарником внутри
# `Contents/MacOS/`, поэтому скрипт просто раскладывает их по местам.
#
# Использование:
#   tools/macos-bundle.sh <путь к бинарнику> [каталог вывода]
#
# Пример:
#   tools/macos-bundle.sh dist/albus-macos-universal dist
set -euo pipefail

BIN="${1:?укажите путь к собранному бинарнику albus}"
OUT_DIR="${2:-dist}"
VERSION="${ALBUS_VERSION:-0.1.0}"
APP="$OUT_DIR/Albus.app"

if [ ! -f "$BIN" ]; then
  echo "нет такого файла: $BIN" >&2
  exit 1
fi

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cp "$BIN" "$APP/Contents/MacOS/albus"
chmod +x "$APP/Contents/MacOS/albus"

# Иконку кладём, если она есть в репозитории (iconutil собирает .icns из набора,
# но у нас только PNG — macOS примет его как есть в CFBundleIconFile с .png).
if [ -f assets/icon.png ]; then
  cp assets/icon.png "$APP/Contents/Resources/icon.png"
  ICON_KEY="<key>CFBundleIconFile</key><string>icon.png</string>"
else
  ICON_KEY=""
fi

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Albus Audio</string>
  <key>CFBundleDisplayName</key><string>Albus Audio</string>
  <key>CFBundleIdentifier</key><string>com.vdagger.albus</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleExecutable</key><string>albus</string>
  $ICON_KEY
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <!-- Микрофон и камера: без этих строк macOS молча не даёт доступ. -->
  <key>NSMicrophoneUsageDescription</key>
  <string>Albus передаёт голос собеседникам по P2P-соединению.</string>
  <key>NSCameraUsageDescription</key>
  <string>Albus может передавать изображение с камеры в эфир.</string>
  <key>NSLocalNetworkUsageDescription</key>
  <string>Albus ищет собеседников в локальной сети для прямого соединения.</string>
</dict>
</plist>
PLIST

# Ad-hoc подпись: без неё Gatekeeper может убить процесс при первом запуске.
# Настоящая подпись требует Developer ID и делается отдельно.
if command -v codesign >/dev/null 2>&1; then
  codesign --force --deep --sign - "$APP" || \
    echo "предупреждение: ad-hoc подпись не удалась, приложение может не запуститься" >&2
fi

echo "готово: $APP"
