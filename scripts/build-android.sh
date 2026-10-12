#!/usr/bin/env bash
# Сборка APK скелета Android (arm64): cargo-ndk → libnrb_android.so (+ libc++_shared.so) → aapt2 link → zipalign → apksigner (отладочный ключ).
# Без Gradle: нужны Android SDK (build-tools, platforms), NDK r27, cargo-ndk, JDK (keytool). Использование:
#   scripts/build-android.sh [--release] [--install]     # --install: поставить на подключённое устройство через adb
# Результат: target/android/nrb-debug.apk (или nrb-release.apk — подписан тем же отладочным ключом, не для магазина).
set -eu
cd "$(dirname "$0")/.."
. scripts/android-env.sh
PROFILE=debug; CARGO_FLAGS=""; INSTALL=0
for a in "$@"; do case "$a" in --release) PROFILE=release; CARGO_FLAGS="--release";; --install) INSTALL=1;; esac; done
# Версии build-tools и платформы: заданные явно, иначе самые новые из установленных (на раннерах CI набор другой, чем у разработчика).
BT_VER="${BUILD_TOOLS:-$(ls "$ANDROID_HOME/build-tools" | sort -V | tail -1)}"
PLAT="${PLATFORM:-$(ls "$ANDROID_HOME/platforms" | sort -V | tail -1)}"
BT="$ANDROID_HOME/build-tools/$BT_VER"
JAR="$ANDROID_HOME/platforms/$PLAT/android.jar"
OUT=target/android; STAGE="$OUT/stage"
rm -rf "$STAGE"; mkdir -p "$STAGE"

# Отладочная информация Servo занимает десятки ГБ (первая сборка заполнила диск): для debug-профиля отключаем её.
export CARGO_PROFILE_DEV_DEBUG="${CARGO_PROFILE_DEV_DEBUG:-0}"

# 1. Нативная часть. Профиль release для Servo — десятки минут; debug быстрее, но .so на сотни МБ.
# shellcheck disable=SC2086
scripts/lowload.sh cargo ndk -t arm64-v8a -o "$STAGE/lib" build -p nrb-android $CARGO_FLAGS
LIBS="$STAGE/lib/arm64-v8a"
# Лишние cdylib зависимостей (libnetrunner_logger.so) приложению не нужны; C++ runtime нужен (mozjs/Servo), берём из NDK.
find "$LIBS" -name '*.so' ! -name 'libnrb_android.so' -delete
cp "$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so" "$LIBS/"
# Таблица символов не нужна на устройстве (отладочной информации и так нет): без неё .so в несколько раз меньше.
"$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-strip" --strip-unneeded "$LIBS/libnrb_android.so"
ls -la "$LIBS"

# 2. Манифест → пустой APK (aapt2), 3. нативные библиотеки внутрь (без сжатия: так их можно отображать в память напрямую).
rm -f "$OUT/app-unsigned.apk"
"$BT/aapt2" link -o "$OUT/app-unsigned.apk" --manifest android/AndroidManifest.xml -I "$JAR" \
  --min-sdk-version 24 --target-sdk-version 35 --version-code 1 --version-name 0.0.1
# Со сжатием: манифест просит распаковку библиотек при установке (extractNativeLibs), поэтому прямое отображение не нужно.
(cd "$STAGE" && zip -9 -qr "../app-unsigned.apk" lib)

# 4. Выравнивание и подпись отладочным ключом (создаётся при отсутствии).
KS="${KEYSTORE:-$HOME/.android/debug.keystore}"
if [ ! -f "$KS" ]; then
  mkdir -p "$(dirname "$KS")"
  keytool -genkeypair -keystore "$KS" -storepass android -keypass android -alias androiddebugkey -keyalg RSA -keysize 2048 \
    -validity 10000 -dname "CN=Android Debug,O=Android,C=US"
fi
APK="$OUT/nrb-$PROFILE.apk"
"$BT/zipalign" -f -p 4 "$OUT/app-unsigned.apk" "$OUT/app-aligned.apk"
"$BT/apksigner" sign --ks "$KS" --ks-pass pass:android --key-pass pass:android --out "$APK" "$OUT/app-aligned.apk"
"$BT/apksigner" verify "$APK" && echo "APK: $APK ($(du -h "$APK" | cut -f1))"

if [ "$INSTALL" = 1 ]; then
  ADB="$ANDROID_HOME/platform-tools/adb"
  "$ADB" devices
  "$ADB" install -r "$APK"
  "$ADB" shell am start -n org.netrunner.browser/android.app.NativeActivity
  echo "логи: $ADB logcat -s nrb RustStdoutStderr"
fi
