#!/usr/bin/env python3
"""Печатает `.cargo/config.toml` с `[patch.crates-io]` для форка Servo (nrxp-servo): все крейты `servo*` из его workspace,
которые есть в Cargo.lock браузера (в любом виде). Патчить нужно ВСЕ сразу: если подменить только `servo-net`, он потянет
за собой path-версии `servo-net-traits` и др., и типы разойдутся с crates.io-версиями остального Servo.
Использование: gen-servo-patch.py <каталог форка> <Cargo.lock>"""
import json, re, subprocess, sys

fork, lock = sys.argv[1], sys.argv[2]
meta = json.loads(subprocess.run(
    ["cargo", "metadata", "--no-deps", "--format-version", "1", "--offline"],
    cwd=fork, check=True, capture_output=True, text=True).stdout)
members = {p["name"]: p["manifest_path"].replace("\\", "/").rsplit("/Cargo.toml", 1)[0] for p in meta["packages"]}

registry = set()  # имена пакетов из Cargo.lock
text = open(lock, encoding="utf-8").read()
for block in text.split("[[package]]"):
    name = re.search(r'name = "([^"]+)"', block)
    if name:
        registry.add(name.group(1))  # источник не важен: после подмены у этих пакетов в Cargo.lock его уже нет

print("# Сгенерировано scripts/setup-upstream.sh (scripts/gen-servo-patch.py) — не править руками, в git не хранится.")
print(f"# Servo из форка: {fork}")
print("[patch.crates-io]")
count = 0
for name in sorted(members):
    if name in registry and name.startswith("servo"):
        # Windows: `cargo metadata` отдаёт пути с обратной косой чертой, а в TOML-строке она — экранирование. Приводим к `/`
        # (Cargo принимает такие пути на всех ОС).
        path = members[name].replace("\\", "/")
        print(f'{name} = {{ path = "{path}" }}')
        count += 1
print(f"# подменено крейтов: {count}", file=sys.stderr)
