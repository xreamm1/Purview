# Проверки первого этапа concurrency migration

Upstream baseline: `1532c9c1530b423ab366792d8a80256914fa909e`.
Коммит реализованного code patch: `f3401307a`.
Дата: 2026-11-06. Среда: Windows x86_64 MSVC, AMD Ryzen 5 5500,
6 физических / 12 логических ядер, rustc 1.99.0, cargo 1.99.0.
Использован существующий Cargo.lock, версии зависимостей не менялись.
Debug test profile; это не performance benchmark.

## Фактически выполнено

| Команда | Результат |
|---|---|
| `cargo check --locked -p horizon-client --no-default-features --lib` | exit 0; первичная проверка интеграции |
| `cargo clippy --locked -p horizon-client --no-default-features --lib -- -D warnings` | exit 0; после правок review |
| `cargo test --locked -p horizon-client-lib --no-default-features --lib ingress -- --test-threads=4` | exit 0, 7 passed, 0 failed, 2017 filtered; test execution 0.13 s |
| `cargo test --locked -p horizon-client-lib --no-default-features --lib lan:: -- --test-threads=4` | exit 0, 330 passed, 0 failed, 2 ignored, 1692 filtered; test execution 10.37 s |
| `cargo test --locked -p horizon-world --lib level:: -- --test-threads=4` | exit 0, 107 passed, 0 failed, 254 filtered; test execution 0.48 s |
| `cargo fmt --all -- --check` | exit 0 |
| `git diff --check` | exit 0; Git сообщает о нормализации LF/CRLF |
| `python tools/check_public_tree.py` | exit 0, Public tree check passed |
| `python tools/check_public_tree.py --history` | exit 0; также повторено pre-push hook после загрузки полной upstream истории |
| `python tools/parity.py --changed origin/main --list` | exit 0; 450 of 453 harnesses would run, только выбор, без исполнения |
| `python tools/pr_ready.py --quick --base origin/main` | exit 0; 3 052 Rust tests, 696 Python tests, workspace Clippy, fmt, public-tree — pass; Cargo.lock unchanged |

7 ingress tests входят в сетевые 330, а они входят в 3 052 полного self-check;
эти результаты нельзя суммировать как различные тесты.
Первый запуск более узкого фильтра `lan::ingress::tests` прошёл 5 тестов;
после review добавились concurrent producer/consumer stress и реальный PlayConn test.
Все семь вошли в последующую проверку.

## Что проверяют новые тесты

* `a_flood_is_rejected_without_consuming_another_players_credit`: Full у одного
  соединения, доступность квоты второго, FIFO Left после принятых пакетов,
  ограниченный poll и повторное использование credit.
* `byte_limits_and_global_limits_release_exactly_once`: per-connection/global
  body-byte limits, освобождение после consumer poll/drop, failed send,
  корректные lifetime peak/rejected counters.
* `global_count_is_bounded_even_for_empty_packets`: packet-count cap работает
  независимо от bytes; переполнение не требует большого payload allocation.
* `concurrent_producers_keep_their_order_and_deliver_once`: четыре producer,
  порядок каждого, отсутствие повторной выдачи.
* `consuming_while_producers_retry_full_credit_does_not_lose_packets`: четыре
  producer по 10 000 пакетов, consumer работает одновременно, повторные Full
  возвращают команду, FIFO/однократность и возврат квот в ноль.
* `lan_uses_admission_and_leaves_work_for_the_next_poll`: реальный Lan start/poll,
  remainder в следующем pass, отказ на `usize::MAX` без integer overflow.
* `ingress_full_kicks_instead_of_reporting_a_closed_server`: реальный
  `PlayConn::packet` выбирает переводимый `disconnect.exceeded_packet_rate`,
  снова принимает после drain и закрывается после consumer drop.

Проверку выполнил также отдельный рецензент. Он исправил неточную терминологию
byte counters: это `queued_body_bytes`/`peak_body_bytes` распакованного тела,
не wire bytes, не compressed bytes и не heap/RSS. Добавлены два последних
concurrency/integration сценария. Новых dependencies, atomics или unsafe нет.

## Ограничения доказательств

Подготовленных `.local/minecraft` и `.local/horizon-client/26.3` на машине нет.
Vanilla differential harnesses, полноценные игровые LAN e2e, MSPT/TPS и core-scaling
workloads **не запускались**. Время unit test execution не является MSPT или
оценкой ускорения сервера. Значения caps и целевые acceptance thresholds в
[архитектурном разборе](concurrency-audit.md) — параметры/цели, не benchmark results.

В тестах байты некоторых пакетов заданы синтетически: они проверяют accounting,
а не compression ratio или фактический heap footprint декодера. Новый реальный
PlayConn test дополнительно проверяет вызов admission после decode и путь disconnect.
Control events не ограничены packet budget; другая очередь integrated tasks
и per-dimension routing ещё не изменены. Не проверялись crash recovery и entity
transfer guarantees будущей архитектуры — в коде этого этапа их ещё нет.

Широкий self-check завершился с exit 0 за 15 min 44 s; captured base — origin/main
at `1532c9c15`. Workspace Clippy: `--workspace --all-targets --locked -- -D warnings`.
Результат из `tools/pr_ready.py`:

| Check | Result |
|---|---|
| Cargo.lock | unchanged |
| Public tree | pass |
| Python tests | 696 passed (4 min 19 s) |
| rustfmt | pass (58 s) |
| clippy | pass (6 min 28 s) |
| tests: horizon-client, horizon-client-lib | 3 052 passed (7 min 51 s) |
| doc tests | 0 passed, command pass (14 s) |
| content root tests | not run: content root untouched |
| tokens.css / launcher | not run: untouched |
| Parity | not run (`--quick`), required before a compatibility claim |

Self-check scope был вычислен до последних редакционных дополнений документации;
проверенный код совпадает с code commit `f3401307a`. После него менялись только
Markdown-файлы с аудитом и результатами. Длительности — wall time локальной проверки,
не игровые performance numbers.

Публикация patch в fork не означает production-ready или завершённую региональную
многопоточность. Для release нужны нагрузочные и vanilla проверки следующего gate.

## История и публикация

Рабочая папка изначально была пустым Git repository. В неё получен upstream main
на указанном baseline, затем перед публикацией догружена полная история:
shallow push в пустой GitHub repository отклонялся из-за недостающих объектов.
Проверка всех исторических путей выполнена с локальным `diff.renames=false`, чтобы
Git не скачивал исторические blobs по одному для similarity detection; removed
и added пути всё равно проверяются. История не переписывалась, hooks не обходились.
Свежие upstream commits, полученные вместе с историей, в patch не merged:
родитель code commit остаётся исследованным `1532c9c...`.
