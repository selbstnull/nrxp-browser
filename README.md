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
| Собственный узел: `nrb node init/run/invite/devices/revoke`, `nrb pair` (без бэкенда) | **в разработке**: ядро и тесты есть, сквозной скрипт `scripts/node-e2e.sh` ещё имеет один падающий шаг |
| Деплой узла на VPS по SSH (`nrb deploy`) | не начато |
| Reverse-egress (домашний браузер за NAT как выход) | не начато (спайк по коду: реализуемо) |
| HTTP/3 поверх NRXP | не начато; в ядре потолок датаграммы 1150 B < 1200 B у QUIC — нужна правка ядра |
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
Без `scripts/setup-upstream.sh` проект не соберётся (пути на `.nrb-dev/`). Патчи лежат в `patches/` (к `netrunner-proxy` и к Servo).

## Использование

```bash
nrb https://example.org                       # окно браузера (профиль ~/.local/share/netrunner-browser/profile)
nrb --private https://example.org             # без записи на диск
nrb snap https://example.org -o out.png       # headless-скриншот
nrb --node-addr 127.0.0.1:18443 <url>         # через узел NRXP (dev-узел: scripts/dev-node.sh)
nrb fetch https://example.org --node-addr …   # загрузка через NRXP без движка
nrb corpus --node-addr …                      # прогон корпуса из 57 сайтов, отчёт в out/corpus/
nrb node init --public-host 1.2.3.4 --port 443 && nrb node invite телефон && nrb node run   # свой узел (в разработке)
```

## Проверки

```bash
cargo test --workspace                        # ~60 тестов (часть требует собранный netrunner-server рядом)
scripts/gui-smoke.sh                          # GUI-сценарии в вложенном Xephyr (не трогают рабочий стол)
scripts/gui-dialogs.sh
scripts/gui-profile.sh
scripts/leakwatch.sh /tmp/stop > leaks.txt &  # наблюдатель за прямыми соединениями процесса nrb
scripts/node-e2e.sh                           # сквозная проверка своего узла (в разработке)
```

## Устройство

`nrb` (бинарь) · `crates/nrb-engine` (трейт движка, омнибокс) · `nrb-engine-servo` (Servo, русские страницы ошибок) ·
`nrb-shell` (окно, вкладки, диалоги, библиотека) · `nrb-transport` (NRXP-потоки без TUN) · `nrb-net` (h1/h2 клиент, локальный прокси) ·
`nrb-profile` (история/закладки/сессия/настройки) · `nrb-node` (узел: ingress, реестр устройств, приглашения).

## Лицензия

Окончательно не выбрана. В `Cargo.toml` указан MPL-2.0 (в репозитории лежат файлы и патчи Servo под MPL-2.0 —
`crates/nrb-engine-servo/resources/`, `patches/servo/`). Файла LICENSE пока нет.
