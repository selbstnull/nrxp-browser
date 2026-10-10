# Платформы: что работает, что проверено, что мешает

Дата: 2026-10-10. Разработка и все проверки велись на **одной** машине: Ubuntu 24.04, x86_64, X11. Ниже честно: что подтверждено
запуском, что только скомпилировано, что не пробовалось и почему.

| ОС | Состояние | Подтверждено | Не подтверждено / блокер |
|---|---|---|---|
| **Linux x86_64** | всё работает | сборка с движком, 136 тестов, GUI-сценарии (Xephyr), узел, деплой, герметичный режим, видео, звук (PipeWire), WebRTC relay | Wayland, другие дистрибутивы, aarch64 |
| **Windows x86_64** | код без движка готов к сборке | `cargo check -p nrb --target x86_64-pc-windows-gnu` проходит (узел, туннель, загрузки, обновления, профиль); правила путей (`nrb/src/platform.rs`) покрыты тестами | **движок Servo не собирался**: нужен целевой `x86_64-pc-windows-msvc` (mozjs не поддерживает gnu — проверено: `mozjs_sys` падает), то есть сборка только на Windows/MSVC; окно, ввод, GStreamer и WebRTC на Windows не запускались |
| **macOS (arm64/x86_64)** | код без движка готов | правила путей (`~/Library/Application Support`, `~/Downloads`, `open`) покрыты тестами | не собиралось вообще (нужен macOS SDK; в среде его нет); движок и GUI не проверены |
| **Android arm64** | **APK собирается** (`scripts/build-android.sh`: debug, 63 МБ, подписан отладочным ключом; на устройстве не запускался) | `cargo check -p nrb --target aarch64-linux-android` (без движка) и `cargo check -p nrb-engine-servo --target aarch64-linux-android` (с Servo) проходят с NDK r27 | нет точки входа и упаковки приложения (ниже); нет эмулятора и устройства в среде — ничего не запускалось |
| **iOS** | **не выполнимо на этом движке** | — | у Servo нет порта на iOS (в его дереве `ios` встречается только в списках исключений мультипроцесса), у `surfman`/WebRender нет iOS-бэкенда; см. ниже |

## Что сделано для переносимости

* `nrb/src/platform.rs` — единственное место с различиями ОС: каталог данных (`XDG_DATA_HOME`/`~/.local/share`, macOS
  `~/Library/Application Support`, Windows `%LOCALAPPDATA%`), каталог загрузок, домашний каталог (`USERPROFILE` на Windows),
  чем открыть папку (`xdg-open`/`open`/`explorer`). `NRB_DATA_DIR` и `NRB_DOWNLOADS_DIR` перекрывают всё (для Android:
  оболочка приложения задаёт пути, домашнего каталога в обычном смысле нет). Правила проверяются тестами на любой машине.
* `nrb-hermetic` на не-Linux отказывается включаться (а не молча деградирует); добавлена недостающая заглушка, из-за которой
  не собиралось под Windows.
* `.github/workflows/platforms.yml` — матрица Linux/Windows/macOS (`portable` — тесты крейтов без движка; `engine` — сборка с
  движком, только вручную) и `android-check`. **На раннерах не запускалась** — первые прогоны потребуют правок. Для `engine`
  нужен опубликованный форк `nrxp-servo` (пока он только локальный).

## Windows и macOS: что осталось

Servo официально собирается на обеих (servoshell). Работы: запустить `platforms.yml` на раннерах GitHub, исправить то, что
вылезет (скрипты `setup-upstream.sh`/`gen-servo-patch.py` написаны под bash+Linux; на Windows нужен Git Bash); проверить окно и
ввод (winit/egui кроссплатформенны, но тут ничего не запускалось); GStreamer для `--features media` ставится отдельно
(MSVC-сборка / `brew install gstreamer`); `libnice` для WebRTC входит в сборки GStreamer, но имя библиотеки
(`libnice-10.dll`, `libnice.10.dylib`) заложено по документации, не проверено. `nrb deploy` вызывает SSH-клиент — на Windows
нужен OpenSSH. Герметичный режим только Linux.

## Android: скелет приложения и что дальше

**Сделано (2026-10-10):** крейт `crates/nrb-android` (`android_main`, NativeActivity без Java), `nrb-shell` принимает `AndroidApp` (`run_android`), тач-события уходят в Servo, кнопка «Назад» (назад по истории, без истории — сворачивание), приостановка/возврат поверхности (пока без пересоздания контекста), каталоги данных из приложения; `scripts/build-android.sh` собирает APK без Gradle (cargo-ndk → aapt2 → zipalign → apksigner; `--install` ставит через adb). Проверено: `libnrb_android.so` экспортирует `ANativeActivity_onCreate` и `android_main`, `aapt2 dump badging` показывает launchable-activity и arm64-v8a. **Не проверено ничем: запуск на устройстве.** Скелет ходит в сеть напрямую (без туннеля NRXP), без загрузок, без WebRTC. Остаётся то, что ниже:

### Что осталось

Подтверждено только то, что код (включая Servo) проходит `cargo check` под arm64. Чтобы это стало приложением:

1. **Точка входа.** `winit` на Android требует фичу `android-native-activity` (или `game-activity`) и `#[no_mangle] fn
   android_main(app: AndroidApp)`; `nrb-shell` сейчас собирается без неё (проверено: `cargo check -p nrb-shell --target
   aarch64-linux-android` падает на `android-activity`). Нужен `cdylib`-крейт и вынос сборки конфигурации из `nrb/src/gui.rs`
   (туннель, загрузки, профиль) в библиотеку.
2. **Жизненный цикл.** На Android окно и GL-контекст исчезают и появляются (`Suspended`/`Resumed`) — оболочка этого не умеет;
   Servo `WindowRenderingContext` надо пересоздавать.
3. **Ввод.** Тач (прокрутка, жесты, долгое нажатие вместо правой кнопки), экранная клавиатура/IME (в winit на Android IME
   частичный), кнопка «Назад», буфер обмена, выбор файлов, разрешения.
4. **Упаковка.** Gradle-проект, манифест, подпись, `cargo-ndk` (инструмент и SDK/NDK 27 на машине есть), иконки; `.so` Servo
   большие (сотни МБ в debug).
5. **Данные и сеть.** Каталоги из `app.internal_data_path()` через `NRB_DATA_DIR`; туннель NRXP на Android тот же код (проверен
   компиляцией), но фоновая работа/энергосбережение — отдельная тема.
6. **Медиа.** `--features media` на Android требует GStreamer Android SDK и `servo-media-gstreamer` в варианте
   `render-android` — отдельная интеграция; без неё `<video>` не играет.
7. **Проверка.** Нужны эмулятор (system image ARM64/x86_64) или устройство: в среде их нет.

Оценка: это отдельный проект на недели, а не доработка; результат нельзя считать готовым, пока он не запущен на устройстве.

## iOS

**С Servo — не выполнимо:** порта нет; пришлось бы писать бэкенд GL/Metal для WebRender и `surfman`, окно/ввод, JIT-политику
(на iOS запрещён JIT вне WebKit — у `mozjs` придётся отключать `js_jit`, производительность резко ниже) и пройти ограничения
App Store: сторонние движки браузера разрешены только по entitlement'у BrowserEngineKit (в основном ЕС, DMA).

**Реалистичная альтернатива (отдельный проект, не из этого репозитория):** нативное приложение на Swift с `WKWebView`
(системный WebKit, как у любого браузера на iOS) и туннелем NRXP как **сетевым расширением** (`NEPacketTunnelProvider`/
прокси-настройка) на ядре `netrunner-core`, которое уже собирается для мобильных клиентов (`netrunner-app`). Движковые
возможности этого репозитория (блокировщик, скрипты, поиск, печать…) там пришлось бы переписывать поверх WebKit API. Сборка и проверка
требуют macOS/Xcode и аккаунта разработчика Apple — в этой среде ничего из этого нет.

## Как проверить на своих машинах

```bash
cargo test -p nrb --bins platform                              # правила путей по ОС (любая ОС)
cargo check -p nrb --target x86_64-pc-windows-gnu              # код без движка под Windows (нужен mingw)
# Android (NDK r27): CC/AR/linker из NDK, для bindgen — --sysroot NDK
cargo check -p nrb-engine-servo --target aarch64-linux-android
```
