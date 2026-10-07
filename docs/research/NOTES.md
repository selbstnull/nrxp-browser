# NOTES — факты, источники, результаты спайков

Дата первичного сбора: 2026-10-07. Всё, что не помечено «проверено», — со слов документации/поиска.

## netrunner-proxy (проверено по исходникам, `/home/k/netrunner/netrunner-proxy`)

* **Прямой доступ к потокам без TUN:** `netrunner_core::net::ClientHandler::connect(addr, decoy_sni, auth_token, identity,
  rx_from_engine, tx_to_engine) -> Arc<Muxer>`; обмен `RawCastFrame` (`rawcast`): `connect/data/close`,
  `LocalProtocol::{Tcp,Udp}`. Пример использования — `tools/loadtest/src/main.rs`.
  `connect()` возвращается сразу; Connect-кадр до появления первой ноги теряется → ждать `Muxer::active_legs_count() > 0`.
* **DNS на выходной ноде:** в `Connect`/`UdpConnect` payload — строка `"host:port"` (`rawcast/adapter.rs`), сервер резолвит
  (`lookup_host` в `connection/handler.rs`).
* **UDP-кадры:** `Muxer::select_udp_leg` отдаёт кадр на нативную datagram-ногу, только если она свежая и кадр ≤
  `max_datagram_payload`; иначе по TCP-ногам (flowlet-балансировка). Следствие для HTTP/3 — ARCHITECTURE §4.
* **Авторизация подключаемая:** `trait AuthValidator` (`net/auth.rs`): `validate`, `report_usage(_batch)`,
  `report_node_health`, `list_mesh_peers`, `validate_mesh_peer`.
* **Mesh:** `NodeMesh`, `mesh_onion` (HPKE), `MeshPeer { node_id, host, port, decoy_sni, nrxp_secret, nrxp_static_public }` —
  пир, которого ingress **набирает сам**. Reverse-регистрации в коде нет (поиск `reverse` в `mesh.rs` пуст) → спайк S3.
* **Сервер — bin-крейт** (`server/src/{main,network,backend_client,…}.rs`); флаги: `--require-auth`, `--mesh-enabled`,
  `--decoy-host`, `--decoy-mode`, `--backend-url`; env: `PROXY_NRXP_SECRET`, `PROXY_NRXP_PRIVATE_KEY`, `PROXY_NRXP_STRICT`,
  `PROXY_INTERNAL_SECRET`. JSON-лог идёт в stdout, на диск не пишется (на диск раньше забивало диск ноды).
* **Провижининг нод в backend** (`netrunner-backend/src/modules/nodes/service/{ssh,container}.rs`): SSH + `docker run` через
  `init_node.sh`; хост-ключи пинить; мастер-ключ только на control-plane. `nrb deploy` делаем по тому же духу, но без docker.
* **masque-edge:** отдельный крейт HTTP/3 CONNECT / CONNECT-UDP (RFC 9298), приватные/loopback адреса отклоняются по умолчанию —
  образец политики egress.
* **Непушенная работа в соседних репо** не должна теряться — см. PROGRESS «Координация».

## Servo (`servo` 0.7.0 на crates.io, MPL-2.0, rust-version 1.88)

Проверено по исходникам крейта (`~/.cargo/registry/src/*/servo-0.7.0`):
* Публичные модули встраивания: `servo`, `webview`, `webview_delegate`, `servo_delegate`, `site_data_manager`,
  `network_manager` (кэш HTTP **только в памяти**, дисковый «в разработке»), `user_content_manager` (UserScript/UserStyleSheet,
  применяются после перезагрузки), `clipboard_delegate`, `gamepad_delegate`, `javascript_evaluator`.
* `WebViewDelegate`: `notify_{url,page_title,status_text,load_status,cursor,favicon,new_frame_ready,history,closed,crashed,
  fullscreen}_changed`, `request_{navigation,create_new,permission,authentication,protocol_handler,unload,resize_to,move_to}`,
  `load_web_resource(webview, WebResourceLoad)` — перехват загрузок (`intercept`).
* Фичи: `default = [bundled, clipboard, js_jit, multiprocess]`; `default_web_features = [brotli-compression-stream, clipboard,
  webcrypto, webgpu, webgl, webxr]`; `media-gstreamer`, `bluetooth`, `gamepad`, `js_backtrace`, …
* Пример `examples/winit_minimal.rs`: `ServoBuilder::default().event_loop_waker(..).build()`,
  `WebViewBuilder::new(&servo, rendering_context).url(..).delegate(..).build()`, `servo.spin_event_loop()`,
  `webview.paint()` + `rendering_context.present()`; в примере крипто-провайдер rustls — `aws_lc_rs`.
* **Не проверено (S1/S2):** как задаётся прокси; потоковость `intercept`.

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
* Настройки — поля `Preferences`: `network_http_proxy_uri`, `network_https_proxy_uri` (пустая строка = без прокси),
  `network_http_no_proxy`; задаются через `ServoBuilder::preferences(..)` или `Servo::set_preference(name, PrefValue)`.
* Реализация (`servo-net/connector.rs::ProxyConnector`): `hyper_util::client::proxy::matcher` + `Tunnel` → **только HTTP CONNECT**
  (с Basic-авторизацией из URL), **SOCKS нет**. Следовательно локальный прокси `nrb-net` обязан говорить HTTP CONNECT
  (SOCKS5 — опционально, для других клиентов). Имя хоста уходит прокси как есть → DNS на выходной ноде.
* Прочее полезное: `network_use_webpki_roots` (корни webpki вместо системного хранилища — **включаем**),
  `network_http_disk_cache`/`network_http_disk_cache_size` (дисковый кэш управляется prefs), `user_agent`.
* **Осталось проверить:** WebSocket, media (GStreamer `souphttpsrc` имеет собственный сетевой стек — вероятный обход),
  Web/Service Workers, favicon, devtools. Тест — I5 (tcpdump во время прогона корпуса).
* HTTP/3 в `servo-net` нет (в `Cargo.toml` нет quinn/h3) → h3 возможен только в режиме I (перехват), I10.
