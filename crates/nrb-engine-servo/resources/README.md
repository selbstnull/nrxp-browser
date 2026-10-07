# Ресурсы Servo

Файлы скопированы из `servo-default-resources 0.7.0` (MPL-2.0, https://github.com/servo/servo) без изменений, кроме страниц
ошибок, переведённых и оформленных под браузер: `neterror.html`, `badcert.html`, `crash.html`.
Они подключаются через собственный `ResourceReader` (`src/resources.rs`), поэтому у Servo отключена фича `bundled`
(встроенный ридер) — иначе зарегистрировались бы два ридера.

При обновлении Servo: сверить набор `Resource` с `embedder_traits::resources::Resource` и обновить файлы из нового
`servo-default-resources`.
