# source scripts/android-env.sh — переменные для сборки под Android (NDK r27): компилятор, линкер и sysroot для bindgen (mozangle).
# Пути можно переопределить: ANDROID_HOME, NDK_VERSION.
export ANDROID_HOME="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}"
export NDK_VERSION="${NDK_VERSION:-27.0.12077973}"
# ANDROID_NDK_HOME можно задать снаружи (в CI путь даёт setup-ndk); иначе — NDK внутри SDK.
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$ANDROID_HOME/ndk/$NDK_VERSION}"
export ANDROID_NDK_ROOT="$ANDROID_NDK_HOME"
_NDK_BIN="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin"
_NDK_SYSROOT="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/sysroot"
export CC_aarch64_linux_android="$_NDK_BIN/aarch64-linux-android24-clang"
export CXX_aarch64_linux_android="$_NDK_BIN/aarch64-linux-android24-clang++"
export AR_aarch64_linux_android="$_NDK_BIN/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$_NDK_BIN/aarch64-linux-android24-clang"
export BINDGEN_EXTRA_CLANG_ARGS_aarch64_linux_android="--sysroot=$_NDK_SYSROOT --target=aarch64-linux-android24 -I$_NDK_SYSROOT/usr/include/aarch64-linux-android"
