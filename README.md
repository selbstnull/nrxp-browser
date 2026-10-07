# netrunner-browser

Браузер на Rust, который держится целиком на NRXP (`netrunner-proxy`) и одновременно является узлом сети
(ingress / egress) с возможностью поставить свой узел на VPS одной командой.

* Архитектура: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
* **План разработки (рабочий файл): [docs/ITERATIONS.md](docs/ITERATIONS.md)**
* Журнал выполнения: [docs/PROGRESS.md](docs/PROGRESS.md)
* Факты, источники, спайки: [docs/research/NOTES.md](docs/research/NOTES.md)

Приоритет: чтобы рендерило, работало и открывало сайты (I1–I3), затем туннель (I4–I5), затем узлы и деплой.

## Быстрый старт

```bash
scripts/check-env.sh            # чего не хватает для сборки Servo
cargo check --workspace         # каркас без движка
cargo run -p nrb -- --help
```

Крейты: `nrb` (бинарь), `crates/nrb-engine` (trait Engine), `crates/nrb-engine-servo` (реализация на Servo, за фичей `servo`).
