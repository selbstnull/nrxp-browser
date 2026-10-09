# netrunner-browser

> ## ⚠️ Сырой проект, в активной разработке. Не проверено человеком.
>
> Весь код и документы в этом репозитории написаны ИИ-ассистентом (Claude) по ходу одной длинной сессии и **ни разу не
> проходили ревью живым человеком**. Автоматические проверки есть (тесты, GUI-сценарии, замеры — см. ниже), но это не
> замена ревью: возможны ошибки, дыры в безопасности и неверные допущения в документах.
> **Не использовать в проде, не доверять ему трафик и секреты.** API, форматы файлов и команды будут меняться без предупреждения.
> Часть заявленного не доделана или заблокирована ограничениями Servo — честный список в [docs/ITERATIONS.md](docs/ITERATIONS.md).

Браузер на Rust, который держится целиком на NRXP ([netrunner-proxy](https://github.com/nineAp/netrunner-core)) и
одновременно является узлом сети (ingress / egress) — задумано также с деплоем своего узла на VPS одной командой.
Движок — [Servo](https://servo.org) (0.7.0, встраивание через крейт `servo`), оболочка — winit + egui.

## Что есть на сегодня (честно)

| Что | Состояние |
|---|---|
| Рендер и открытие сайтов (Servo, headless-снимок и окно с вкладками) | работает; на корпусе из 57 сайтов открывается ~90–98% (часть — «partial»), JS-тяжёлые SPA и антибот — проблемные |
| Весь трафик движка через NRXP без TUN/root (локальный HTTP-CONNECT прокси + `NrxpTransport`) | работает на dev-узле; утечек в обход туннеля при проверенных сценариях не найдено (WebSocket в Servo обходил прокси — исправлено патчем Servo) |
| Диалоги страницы, `<select>`, контекстное меню, HTTP-авторизация, русские страницы ошибок | работает (GUI-сценарии в Xephyr) |
| Профиль: куки/localStorage/IndexedDB, история, закладки, восстановление вкладок, настройки | работает |
| Собственный узел: `nrb node init/run/invite/devices/revoke`, `nrb pair` (без бэкенда), окно «Узлы» в браузере (Ctrl+Shift+N: спаривание, выбор узла, свой узел, приглашения) | работает на loopback (тесты + `scripts/node-e2e.sh`, `scripts/gui-nodes.sh`: спаривание, отзыв, decoy, `snap` через свой узел); не проверено на реальном VPS; нет QR-кода |
| Деплой узла на VPS по SSH (`nrb deploy`) | работает на тестовом контейнере (systemd + sshd: установка, апгрейд, откат, удаление); **на реальном VPS не проверялось** |
| Reverse-egress (домашний `nrb` за NAT как выход: `nrb egress`, `nrb node invite --egress`, `exit = "require"`) | работает на loopback (3 процесса, `scripts/egress-e2e.sh`, тесты ядра); требует **патч ядра** `patches/proxy/0002` (не запушен, без ревью); не проверено на двух реальных хостах; нет лимитов, выбор выхода — один на узел |
| HTTP/3 поверх NRXP | не начато; в ядре потолок датаграммы 1150 B < 1200 B у QUIC — нужна правка ядра |
| Герметичный режим `--hermetic` (Linux): движок в пустом сетевом пространстве имён, прямых соединений нет по построению | работает (самопроверка `nrb hermetic-selftest`, `scripts/hermetic-e2e.sh`, `gui-hermetic.sh`); один процесс, без sandbox; зависит от разрешённых пользовательских namespaces |
| Блокировщик рекламы и трекеров (`nrb adblock update`, EasyList/EasyPrivacy, счётчик в тулбаре) | работает на сетевом уровне (юнит-тесты, `scripts/gui-adblock.sh`, проверка на cnn.com); нет косметических правил, скриптлетов, режима I |
| Пользовательские скрипты/стили (`<профиль>/userscripts/*.user.js`, `*.user.css`, `@match`/`@exclude`/`@run-at`) | работает (`scripts/gui-userscripts.sh`); без `GM_*` API и без управления в GUI |
| Загрузки файлов, поиск по странице, печать, менеджер паролей, MSE-видео, расширения | нет (ограничения Servo 0.7 или не сделано) |

План — [docs/ITERATIONS.md](docs/ITERATIONS.md) (рабочий файл), журнал — [docs/PROGRESS.md](docs/PROGRESS.md),
архитектура — [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), факты и результаты спайков — [docs/research/NOTES.md](docs/research/NOTES.md).

## Сборка

Нужны Linux (разрабатывалось на Ubuntu 24.04), Rust ≥ 1.88 и соседний репозиторий **netrunner-proxy** рядом (`../netrunner-proxy`):

```bash
scripts/check-env.sh                 # какие системные пакеты не хватает (clang, cmake, fontconfig, …)
scripts/setup-upstream.sh            # worktree netrunner-proxy с нашими патчами + servo-net с патчем WebSocket → .nrb-dev/
cargo build -p nrb --features servo  # с движком (первая сборка Servo — минуты, ~10 ГБ target/)
cargo build -p nrb                   # без движка: узел, fetch, corpus-оркестратор
```

Важно: `Cargo.lock` взят из архива `servo-0.7.0` — не делать `cargo update` вслепую (иначе падает сборка на пре-релизных крейтах).
Без `scripts/setup-upstream.sh` проект не соберётся (пути на `.nrb-dev/` и сгенерированный `.cargo/config.toml`). Servo берётся из форка `../nrxp-servo` (ветка `nrxp/0.7`, см. [docs/NRXP-SERVO.md](docs/NRXP-SERVO.md)), форк обязателен (в нём `NetworkConnector`, которого нет в crates.io). Патчи лежат в `patches/` (к `netrunner-proxy` и к Servo).

## Использование

```bash
nrb https://example.org                       # окно браузера (профиль ~/.local/share/netrunner-browser/profile)
nrb --private https://example.org             # без записи на диск
nrb snap https://example.org -o out.png       # headless-скриншот
nrb --node-addr 127.0.0.1:18443 <url>         # через узел NRXP (dev-узел: scripts/dev-node.sh)
nrb fetch https://example.org --node-addr …   # загрузка через NRXP без движка
nrb corpus --node-addr …                      # прогон корпуса из 57 сайтов, отчёт в out/corpus/
nrb node init --public-host 1.2.3.4 --port 443 && nrb node invite телефон && nrb node run   # свой узел
scripts/build-node-musl.sh && nrb deploy root@1.2.3.4 --binary target-release/x86_64-unknown-linux-musl/release/nrb   # поставить узел на VPS
```

## Проверки

```bash
cargo test --workspace                        # ~60 тестов (часть требует собранный netrunner-server рядом)
scripts/gui-smoke.sh                          # GUI-сценарии в вложенном Xephyr (не трогают рабочий стол)
scripts/gui-dialogs.sh
scripts/gui-profile.sh
scripts/leakwatch.sh /tmp/stop > leaks.txt &  # наблюдатель за прямыми соединениями процесса nrb
scripts/node-e2e.sh                           # сквозная проверка своего узла (loopback)
scripts/gui-nodes.sh                          # окно «Узлы»: два экземпляра браузера, спаривание и перезапуск через узел
scripts/hermetic-e2e.sh                       # герметичный режим: самопроверка отсутствия прямых соединений
scripts/gui-hermetic.sh                       # окно в герметичном режиме, потоки по сетевым пространствам имён
scripts/gui-adblock.sh                        # блокировщик рекламы: локальный сервер, правило /ads/*$script
scripts/gui-userscripts.sh                    # пользовательские скрипты и стили
scripts/egress-e2e.sh                         # reverse-egress: клиент → узел → домашний выход (3 процесса, loopback)
scripts/deploy-e2e.sh                         # деплой в тестовый контейнер с systemd (нужен docker)
```

## Устройство

`nrb` (бинарь) · `crates/nrb-engine` (трейт движка, омнибокс) · `nrb-engine-servo` (Servo, русские страницы ошибок) ·
`nrb-shell` (окно, вкладки, диалоги, библиотека) · `nrb-transport` (NRXP-потоки без TUN) · `nrb-net` (h1/h2 клиент, локальный прокси) ·
`nrb-profile` (история/закладки/сессия/настройки) · `nrb-node` (узел: ingress, реестр устройств, приглашения, домашний выход) · `nrb-adblock` (блокировщик) · `nrb-userscripts` (скрипты и стили) · `nrb-hermetic` (пустой netns для движка) · `nrb-deploy` (деплой узла по SSH).

## Лицензия

Окончательно не выбрана. В `Cargo.toml` указан MPL-2.0 (в репозитории лежат файлы и патчи Servo под MPL-2.0 —
`crates/nrb-engine-servo/resources/`, `patches/servo/`). Файла LICENSE пока нет.
