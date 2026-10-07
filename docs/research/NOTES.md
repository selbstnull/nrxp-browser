# NOTES — факты, источники, результаты спайков

Дата первичного сбора: 2026-10-07. Всё, что не помечено «проверено», — со слов документации/поиска.

## netrunner-proxy (проверено по исходникам, `/home/k/netrunner/netrunner-proxy`)

- **Прямой доступ к потокам без TUN:** `netrunner_core::net::ClientHandler::connect(addr, decoy_sni, auth_token, identity,
rx_from_engine, tx_to_engine) -> Arc<Muxer>`; обмен `RawCastFrame` (`rawcast`): `connect/data/close`,
  `LocalProtocol::{Tcp,Udp}`. Пример использования — `tools/loadtest/src/main.rs`.
  `connect()` возвращается сразу; Connect-кадр до появления первой ноги теряется → ждать `Muxer::active_legs_count() > 0`.
- **DNS на выходной ноде:** в `Connect`/`UdpConnect` payload — строка `"host:port"` (`rawcast/adapter.rs`), сервер резолвит
  (`lookup_host` в `connection/handler.rs`).
- **UDP-кадры:** `Muxer::select_udp_leg` отдаёт кадр на нативную datagram-ногу, только если она свежая и кадр ≤
  `max_datagram_payload`; иначе по TCP-ногам (flowlet-балансировка). Следствие для HTTP/3 — ARCHITECTURE §4.
- **Авторизация подключаемая:** `trait AuthValidator` (`net/auth.rs`): `validate`, `report_usage(_batch)`,
  `report_node_health`, `list_mesh_peers`, `validate_mesh_peer`.
- **Mesh:** `NodeMesh`, `mesh_onion` (HPKE), `MeshPeer { node_id, host, port, decoy_sni, nrxp_secret, nrxp_static_public }` —
  пир, которого ingress **набирает сам**. Reverse-регистрации в коде нет (поиск `reverse` в `mesh.rs` пуст) → спайк S3.
- **Сервер — bin-крейт** (`server/src/{main,network,backend_client,…}.rs`); флаги: `--require-auth`, `--mesh-enabled`,
  `--decoy-host`, `--decoy-mode`, `--backend-url`; env: `PROXY_NRXP_SECRET`, `PROXY_NRXP_PRIVATE_KEY`, `PROXY_NRXP_STRICT`,
  `PROXY_INTERNAL_SECRET`. JSON-лог идёт в stdout, на диск не пишется (на диск раньше забивало диск ноды).
- **Провижининг нод в backend** (`netrunner-backend/src/modules/nodes/service/{ssh,container}.rs`): SSH + `docker run` через
  `init_node.sh`; хост-ключи пинить; мастер-ключ только на control-plane. `nrb deploy` делаем по тому же духу, но без docker.
- **masque-edge:** отдельный крейт HTTP/3 CONNECT / CONNECT-UDP (RFC 9298), приватные/loopback адреса отклоняются по умолчанию —
  образец политики egress.
- **Непушенная работа в соседних репо** не должна теряться — см. PROGRESS «Координация».

## Servo (`servo` 0.7.0 на crates.io, MPL-2.0, rust-version 1.88)

Проверено по исходникам крейта (`~/.cargo/registry/src/*/servo-0.7.0`):

- Публичные модули встраивания: `servo`, `webview`, `webview_delegate`, `servo_delegate`, `site_data_manager`,
  `network_manager` (кэш HTTP **только в памяти**, дисковый «в разработке»), `user_content_manager` (UserScript/UserStyleSheet,
  применяются после перезагрузки), `clipboard_delegate`, `gamepad_delegate`, `javascript_evaluator`.
- `WebViewDelegate`: `notify_{url,page_title,status_text,load_status,cursor,favicon,new_frame_ready,history,closed,crashed,
fullscreen}_changed`, `request_{navigation,create_new,permission,authentication,protocol_handler,unload,resize_to,move_to}`,
  `load_web_resource(webview, WebResourceLoad)` — перехват загрузок (`intercept`).
- Фичи: `default = [bundled, clipboard, js_jit, multiprocess]`; `default_web_features = [brotli-compression-stream, clipboard,
webcrypto, webgpu, webgl, webxr]`; `media-gstreamer`, `bluetooth`, `gamepad`, `js_backtrace`, …
- Пример `examples/winit_minimal.rs`: `ServoBuilder::default().event_loop_waker(..).build()`,
  `WebViewBuilder::new(&servo, rendering_context).url(..).delegate(..).build()`, `servo.spin_event_loop()`,
  `webview.paint()` + `rendering_context.present()`; в примере крипто-провайдер rustls — `aws_lc_rs`.
- **Не проверено (S1/S2):** как задаётся прокси; потоковость `intercept`.

Внешние источники: [Servo 0.1.0 на crates.io](https://servo.org/blog/2026/04/13/servo-0.1.0-release/),
[July in Servo](https://servo.org/blog/2026/08/31/july-in-servo/), [ServoDelegate](https://doc.servo.org/servo/trait.ServoDelegate.html),
[Servo Multimedia/MSE](https://nlnet.nl/project/Servo-multimedia/), [Blitz](https://cdn.jsdelivr.net/gh/dioxuslabs/blitz@main/README.md),
[Boa v0.21](https://boajs.dev/blog/2025/10/22/boa-release-21).

Пробел веб-платформы по состоянию на 2026-10 (из поиска, не из тестов): MSE нет; WebRTC — зачатки; Service Workers — частично;
IndexedDB — экспериментально; расширений нет; multiprocess ≠ site-per-process.

## Окружение разработки (проверено 2026-10-07)

Ubuntu 24.04, 16 ядер, 38 ГиБ ОЗУ, 207 ГиБ свободно, DISPLAY=:1, rustc 1.95, есть passwordless sudo.
Не было: `clang`, `cmake`, `libfontconfig1-dev`, `libunwind-dev`, `libudev-dev`, `libvulkan-dev`, `libgles2-mesa-dev`, GStreamer-dev.

## Спайки (результаты)

Формат: `### S<N> — вопрос` → дата, метод, вердикт (да/нет/частично), что это меняет в ITERATIONS.md.

### S1 — прокси в Servo 0.7 (частичный ответ по исходникам, 2026-10-07; на живом трафике ещё не проверено)

- Настройки — поля `Preferences`: `network_http_proxy_uri`, `network_https_proxy_uri` (пустая строка = без прокси),
  `network_http_no_proxy`; задаются через `ServoBuilder::preferences(..)` или `Servo::set_preference(name, PrefValue)`.
- Реализация (`servo-net/connector.rs::ProxyConnector`): `hyper_util::client::proxy::matcher` + `Tunnel` → **только HTTP CONNECT**
  (с Basic-авторизацией из URL), **SOCKS нет**. Следовательно локальный прокси `nrb-net` обязан говорить HTTP CONNECT
  (SOCKS5 — опционально, для других клиентов). Имя хоста уходит прокси как есть → DNS на выходной ноде.
- Прочее полезное: `network_use_webpki_roots` (корни webpki вместо системного хранилища — **включаем**),
  `network_http_disk_cache`/`network_http_disk_cache_size` (дисковый кэш управляется prefs), `user_agent`.
- **Осталось проверить:** WebSocket, media (GStreamer `souphttpsrc` имеет собственный сетевой стек — вероятный обход),
  Web/Service Workers, favicon, devtools. Тест — I5 (tcpdump во время прогона корпуса).
- HTTP/3 в `servo-net` нет (в `Cargo.toml` нет quinn/h3) → h3 возможен только в режиме I (перехват), I10.

### S0 — собирается ли Servo на этой машине (2026-10-07) — **да**

- `servo = "=0.7.0"` (features: `bundled, js_jit, webcrypto, webgl`), `cargo build -p nrb --features servo` (debug): ~2 мин 44 с на 16 ядрах
  после загрузки зависимостей; `target/` ≈ см. PROGRESS; бинарь debug ≈ 1,1 ГБ.
- **Критично: `Cargo.lock`.** Без lock-файла cargo берёт свежий пре-релиз `pkcs1 0.8.0-rc.5`, несовместимый с `rsa 0.10.0-rc.18` → сборка падает.
  Решение: взять `Cargo.lock` из архива `servo-0.7.0` (лежит рядом с исходниками в `~/.cargo/registry/src/*/servo-0.7.0/Cargo.lock`) как основу
  нашего lock-файла и **не делать `cargo update` вслепую**. При смене версии Servo — повторить.
- crates.io CDN нестабилен на больших крейтах (`servo-fonts` 6,4 МБ → HTTP 500/таймауты) → `scripts/fetch-deps.sh` с повторами.
- Системные пакеты, которые пришлось поставить: `clang cmake libclang-dev llvm-dev libfontconfig1-dev libunwind-dev libudev-dev libgles2-mesa-dev libvulkan-dev`.
- Встраивание: headless-снимок требует, чтобы встраиватель вызывал `WebView::paint()` по `notify_new_frame_ready`
  (автомат скриншота в `servo-paint/screenshot.rs` иначе не завершается); `SoftwareRenderingContext::new(PhysicalSize)` работает без дисплея;
  `dpi` — отдельная зависимость (Servo её не реэкспортирует); `rustls` провайдер обязателен (`aws_lc_rs`).

## Находки корпуса (I3, 2026-10-07, Servo 0.7.0 debug)

См. ITERATIONS.md → I3 «Итог». Кратко: ~89% сайтов что-то рисуют; полностью «готовыми» по критерию Servo — 70%; JS-тяжёлые SPA и антибот — основная зона проблем;
`nytimes.com` — переполнение стека `StyleThread` (проверить на release).

### S4 — секрет + strict без бэкенда (2026-10-07) — **да**

Тест `strict_node_admits_only_clients_with_credentials` (crates/nrb-transport/tests/loopback.rs): узел `PROXY_NRXP_SECRET`+`PROXY_NRXP_PRIVATE_KEY`+`PROXY_NRXP_STRICT=true`, без `--require-auth`/backend.
Верный клиент → эхо работает; анонимный → не подключился / нет данных; верный публичный ключ + чужой секрет → отвергнут.

### S5 — datagram-нога (2026-10-07) — частично

`MAX_DATAGRAM_LEG_PAYLOAD = 1150` (core/src/net/constants.rs:52); `select_udp_leg` отдаёт кадр на нативную ногу только если `payload_len <= max_datagram_payload`.
QUIC-клиент шлёт Initial ≥ 1200 байт → всегда по TCP-ногам. Нужна правка потолка в ядре либо отказ от h3 по UDP-ноге. На живой ноге не мерили.

### Ядро: `Connect` терялся (2026-10-07)

`Muxer::send_to_network`: `Connect*` не входил в `is_critical` → `control_tx.try_send` → при `Full` кадр молча отбрасывался (WARN раз в секунду). Воспроизводится на 16+ одновременных открытиях.
Патч в `patches/proxy/0001-fix-muxer-Connect.patch`.
Попутно: `NetworkConfig::init_global(mtu)` обязателен до первого подключения (паника «Global NetworkConfig is not initialized»); мы зовём его один раз с MTU 1450 (`init_core` в nrb-transport).
Кредиты: `NrxpStream` обязан отдавать `RawCastFrame::credit` по мере чтения и слать первый грант (`CreditReceiver::initial_offset()`) сразу после `Connect`; отправлять грант надо отдельной задачей (см. итог I4).

### S1 — итог (2026-10-07)

HTTP/HTTPS/fetch/XHR/подресурсы/favicon/воркеры — через прокси Servo (проверено `leakwatch`: внешних сокетов нет). **WebSocket** — прямой `TcpStream::connect` в `servo-net/websocket_loader.rs` (+локальный DNS) → патч.
Сам прокси: только HTTP CONNECT с Basic-auth из URL; SOCKS не поддерживается; `network_http_no_proxy` игнорируется патчем WS.
Метод проверки на будущее при апгрейде Servo: `scripts/leakwatch.sh` + `tests/fixtures/ws.html` + прогон части корпуса; после апгрейда пересобрать патч (`patches/servo`) — формат файла `websocket_loader.rs` может поменяться.

### S3 — reverse-egress (2026-10-07) — **да**

Вердикт и план из четырёх патчей ядра — в ITERATIONS.md → I5.s «Итог». Ключевое наблюдение: `TunnelEngine` строится одинаково на обеих сторонах; серверная роль — это `StreamHandler` с `RemoteOpener`.
Не проверено запуском (только чтение кода) — первая задача I9 — игрушечный стенд из двух процессов.

### S3 — reverse-egress: прототип запуском (2026-10-07) — **работает**
Вердикт из чтения кода подтверждён. Правки ядра (одним патчем `patches/proxy/0002`): (1) `RemoteOpener` получил `reverse_egress` (ingress: куда направлять потоки клиентов) и `egress_policy` (выход: что разрешено); (2) принимающий для `egress:` держит `opener=None` и регистрирует *вид* принятой сессии как `MeshPeerSession` (`owns_muxer=false`, чтобы Drop не гасил сессию, принадлежащую `SessionManager`); (3) мост из `open_via_mesh` вынесен в `bridge_via_peer` и используется и mesh, и reverse-egress; (4) подтверждение соединения от выхода — обычный `PONG` (`mesh_peer:true` без `mesh_route`).
Нюансы: `connect_egress` не поднимает датаграммную ногу (серверный датаграммный handler иначе обслуживал бы цели не по политике — пока TCP-only); регистрация/снятие идут по `session_id` (ноги одной сессии вызывают `register` многократно — идемпотентно), снятие — после `SESSION_CLEANUP_DELAY` без ног; роль выхода без `require_auth` не выдаётся вообще; `AuthValidator::validate_egress` по умолчанию отказывает — боевой бэкенд-валидатор роль не получает, пока её не реализуют. Поток `Connect` от дозвонившегося отвергается (нет opener) — тест.
Риск, оставшийся открытым: в режимах prefer/require выбирается один «последний подключённый» выход на весь узел — компрометированный токен выхода видит трафик всех клиентов узла (см. I9.2).
