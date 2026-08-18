# План реализации: UX-улучшения SwiftScan (поиск больших/ненужных файлов)

Цель продукта: **очень быстрый поиск больших и ненужных файлов с удобным интерфейсом**.
Поиск похожих фото — не трогаем (позже будет удалён). Игры — не трогаем.

Этот план самодостаточен: исполнитель (другая модель) не видит исходный диалог,
поэтому здесь есть точные файлы, сигнатуры, крайние случаи и порядок действий.

---

## Обязательные правила (AGENTS.md)

- Перед изменением кода прочитать `ARCHITECTURE.md`.
- Никогда не делать сканы ФС/анализ дубликатов на egui-потоке обновления.
- Фоновые задачи — с `AtomicBool`-отменой и типизированными сообщениями из `model.rs`.
- Не добавлять зависимости, если хватает std или существующих крейтов.
- После изменений:

  ```text
  cargo fmt -- --check
  cargo check
  cargo test
  ```

- Удаление файлов — только через крейт `trash` и только после подтверждения в UI.

Рекомендуемый порядок: **Пакет 3 → Пакет 1 → Пакет 2 → Пакет 4 → (опц.) 5, 6**.
Каждый пакет заканчивается прогоном трёх команд верификации. Если git-репозиторий
рабочий — делать коммит после каждого пакета (сейчас `.git` пуст; при желании
инициализировать `git init` и закоммитить текущее состояние как baseline).

---

## Пакет 1. Массовые действия (самый частый сценарий)

Сейчас `selected_rows` (app.rs:542, Shift-клик) используется только для счётчика
«ВЫДЕЛЕНО N» (app.rs:1057). Удаление — только по одному файлу и заблокировано до
готовности индекса (`can_delete = self.file_index.is_some()`, app.rs:1084).

### 1.1 Состояние

В `ScannerApp` (app.rs:31) добавить:

```rust
pending_bulk_delete: Option<Vec<usize>>, // индексы в self.files
```

Инициализировать `None` в `new()` и сбрасывать в `start_scan()`.

### 1.2 Кнопка массового удаления

В полосе результатов (app.rs:1051-1074, блок `right_to_left` рядом с «ВЫДЕЛЕНО N»)
добавить кнопку:

- Текст: `В КОРЗИНУ N` (N = количество выбранных), зелёная/красная акцентная.
- `add_enabled(!self.selected_rows.is_empty() && self.file_index.is_some(), ...)`.
- При клике: `self.pending_bulk_delete = Some(self.selected_rows.iter().copied().collect());`
  (отфильтровать индексы, у которых `files[i].deleted` — пропустить).
- Рядом: маленькие кнопки «Выбрать все» и «Снять»:
  - «Выбрать все»: `self.selected_rows = self.visible.iter().copied().collect();`
    (все показанные строки). `selection_anchor` не трогать/сбросить.
  - «Снять»: `self.selected_rows.clear(); self.selection_anchor = None;`
- Счётчик «ВЫДЕЛЕНО N» оставить, он уже есть.

### 1.3 Обобщить диалог подтверждения

`delete_confirmation` (app.rs:1754) сейчас обрабатывает один `pending_delete: Option<usize>`.
Сделать общий метод:

```rust
fn confirm_delete_window(&mut self, ctx: &egui::Context, indices: Vec<usize>)
```

- Заголовок: `Переместить N файлов в корзину?`
- Тело: суммарный размер выбранных (`format_bytes`), первые 5 имён (остальные «…и ещё M»).
- Кнопки «Отмена» / «В корзину».
- Одиночное удаление перевести на тот же метод (`pending_delete` → `pending_bulk_delete`
  с вектором из одного индекса, либо убрать `pending_delete` вовсе — на выбор исполнителя,
  главное не дублировать логику).

### 1.4 Исполнение массового удаления

В методе подтверждения, при «В корзину»:

1. `let mut deleted_count = 0; let mut failed = 0; let mut freed: u64 = 0;`
2. Для каждого индекса: пропустить, если `files[i].deleted`.
3. `trash::delete(&files[i].path)`:
   - Ok: `Arc::make_mut(&mut self.files)[i].deleted = true;`
     если `is_cleanup_candidate()` — вычесть из `analysis.cleanup_files/bytes`
     (saturating_sub), прибавить в `freed`.
   - Err: `failed += 1`.
4. `self.selected_rows.clear(); self.selection_anchor = None;`
5. `self.notice = Some((формат итога, false, Instant::now()))`:
   - все успешны: `Файлов перемещено в корзину: N (размер)` (для 1 — «Файл перемещён…»);
   - есть ошибки: `Перемещено N, не удалось: M`.
6. `self.refresh_visible();`

Замечание: `pending_bulk_delete = None` в конце, чтобы окно не висело.

### 1.5 Крайние случаи

- Индекс может содержать уже удалённые строки — они отфильтрованы `file_matches`
  (фильтр `!file.deleted` уже есть), сортировки пересчитывать не нужно.
- `trash::delete` на залоченном файле — не ронять приложение, только счётчик ошибок.
- Пока `file_index.is_none()` (скан идёт) — массовое удаление недоступно, как и одиночное.
- После удаления категорийные счётчики (см. Пакет 2) тоже надо уменьшить, если они уже
  реализованы.

---

## Пакет 2. Фильтр по категориям мусора

Категории уже вычисляются в `cleanup.rs` (`CleanupCategory`: Temporary, Cache, Logs,
CrashDump, Backup, Development, Installer, Archive, LargeOld, Protected, Ordinary),
но в UI их нельзя отфильтровать.

### 2.1 Модель

- `model.rs:145`: добавить `Hash` в derive для `CleanupCategory`
  (`#[derive(Clone, Copy, PartialEq, Eq, Hash)]`).
- В `ScannerApp` добавить:
  ```rust
  category_filter: Option<CleanupCategory>,
  category_stats: HashMap<CleanupCategory, (u64, u64)>, // файлы, байты — по всем файлам
  ```
  Инициализировать пустыми/`None` в `new()` и сбрасывать в `start_scan()`.

### 2.2 Подсчёт по категориям (инкрементально, не на UI-потоке полным проходом)

- В `poll_scan` при обработке `ScanMessage::Batch` (app.rs:206) в цикле по файлам
  пакета дополнительно:
  ```rust
  let entry = self.category_stats.entry(file.cleanup.category).or_default();
  entry.0 += 1;
  entry.1 = entry.1.saturating_add(file.size);
  ```
- При удалении (Пакет 1.4): для каждого успешно удалённого файла уменьшить
  соответствующую категорию (saturating_sub). Или пересчитать лениво — но проще
  инкрементально.
- При `start_scan()` — сброс.

### 2.3 Предикат

Чтобы не менять сигнатуру `file_matches` везде (filtering.rs:3), добавить отдельный
предикат:

```rust
// filtering.rs
pub(crate) fn file_matches_category(file: &FileEntry, category: Option<CleanupCategory>) -> bool {
    match category {
        None => true,
        Some(category) => file.cleanup.category == category && file.is_cleanup_candidate(),
    }
}
```

Проверять в двух местах:
- `index.rs:query_with_limit` — добавить параметр `category: Option<CleanupCategory>`
  в `query_page`/`query_with_limit` и добавить `.filter(|i| file_matches_category(&files[*i], category))`
  рядом с существующим `file_matches`. Обновить вызовы в `app.rs:501` и в тестах
  `tests.rs:49,63` (передать `None`).
- `app.rs:load_visible_page` (ветка без индекса, app.rs:510-524) — тот же фильтр.

### 2.4 UI: чипы категорий

В сайдбаре под вкладкой «Умная очистка» (app.rs:703-712) добавить блок «КАТЕГОРИИ»:

- Сетка чипов (2 колонки), по одной на категорию из `category_stats`, у которых
  `count > 0` и категория не `Protected`/`Ordinary`.
- Каждый чип: `Кэш · 12 ГБ` (label + `format_count`/`format_bytes`).
- Выбор чипа: `self.category_filter = Some(category); self.view = ViewMode::Cleanup;
  self.refresh_visible();`
- Чип «Все»: `category_filter = None`.
- Выбранный чип подсвечивается (как `mode_tab`, app.rs:783).
- Счётчик в заголовке вкладки «Умная очистка» остаётся общим (без учёта категории).

### 2.5 Крайние случаи

- Чипы во время скана обновляются инкрементально из батчей — это ок.
- `LargeOld` может пересекаться с другими категориями (LargeOld — fallback-ветка,
  так что пересечений нет: категория одна на файл).
- После смены фильтра `loaded_rows` сбрасывать через `refresh_visible()` (уже так).

---

## Пакет 3. Быстрые действия в таблице (низкое усилие, высокая польза)

Всё в `app.rs`, цикл отрисовки строк (app.rs:1109-1287).

### 3.1 Тултипы с полным путём

- Имя (`egui::Label`, app.rs:1157): добавить `.on_hover_text(path.to_string_lossy().into_owned())`.
- Путь (app.rs:1164): `.on_hover_text(полный путь)`.
- В заголовках колонок — можно не добавлять.

### 3.2 Двойной клик — открыть файл

У `selection_response` (app.rs:1265) добавить:

```rust
if selection_response.double_clicked() {
    self.notice = Some(match open_path(&path) { Ok(()) => ("Файл открыт".into(), false, Instant::now()), Err(e) => (e, true, Instant::now()) });
}
```

### 3.3 Правый клик — контекстное меню

Использовать `selection_response.context_menu(|ui| { ... })` (метод `Response::context_menu`
есть в egui 0.31) с теми же пунктами, что в меню «•••» (app.rs:1204-1251):
«Открыть файл», «Показать в папке», «Переместить в корзину». Логику вынести в общий
метод `row_context_menu(&mut self, ui: &mut egui::Ui, index: usize)`, чтобы «•••» и
правый клик не дублировали код.

### 3.4 Клавиатура

В `eframe::App::update` (app.rs:1818), в начале, перед отрисовкой панелей:

```rust
if !ctx.wants_keyboard_input() {   // чтобы не срабатывало при вводе в TextEdit
    let (ctrl_a, delete, escape, ctrl_f) = ctx.input(|i| (
        i.modifiers.command && i.key_pressed(egui::Key::A),
        i.key_pressed(egui::Key::Delete),
        i.key_pressed(egui::Key::Escape),
        i.modifiers.command && i.key_pressed(egui::Key::F),
    ));
    if ctrl_a { self.selected_rows = self.visible.iter().copied().collect(); }
    if delete && self.file_index.is_some() && !self.selected_rows.is_empty() {
        self.pending_bulk_delete = Some(self.selected_rows.iter().copied().collect());
    }
    if escape { self.query.clear(); self.min_mb.clear(); self.max_mb.clear(); self.category_filter = None; self.selected_rows.clear(); self.refresh_visible(); }
    if ctrl_f { self.search_focus = true; }
}
```

- Поле `search_focus: bool` добавить в `ScannerApp`, сброс в `new()`/`start_scan()`.
- В `central_panel` (app.rs:957) у `TextEdit` ответа: если `self.search_focus`,
  вызвать `response.request_focus()` и `self.search_focus = false`.

### 3.5 Крайние случаи

- `ctx.wants_keyboard_input()` — единственный гвард; Delete не должен удалять файл,
  пока пользователь печатает в поле поиска.
- Ctrl+A выделяет только видимые (загруженные) строки — для первой версии достаточно.
  (Опционально позже: `FileIndex`-метод для выделения всех результатов фильтра.)

---

## Пакет 4. Первый запуск и выбор папки

### 4.1 Запоминание последней папки

Без новых зависимостей (правило AGENTS.md): обычный текстовый файл.

- Путь: `%LOCALAPPDATA%\SwiftScan\last_root.txt` (папку `%LOCALAPPDATA%\SwiftScan`
  создавать через `std::fs::create_dir_all`).
- Чтение в `ScannerApp::new` (app.rs:82): если файл есть и `root.is_dir()` — взять его,
  иначе fallback: `USERPROFILE` (или `SystemDrive` + `\`), иначе `C:\`.
- Запись: в `start_scan()` (после валидации root) — `std::fs::write`.
- UTF-8 пути на Windows: `to_string_lossy()` достаточно для конфига (пути из диалога
  почти всегда корректные UTF-16→UTF-8; лоссов принимаем, файл служебный).

### 4.2 Быстрые пресеты в сайдбаре

Под блоком «ОБЛАСТЬ СКАНИРОВАНИЯ» (app.rs:606-625) добавить ряд маленьких кнопок:

- `C:\` — `std::env::var("SystemDrive").unwrap_or("C:".into()) + "\\"`
- `Дом` — `USERPROFILE`
- `Загрузки` — `USERPROFILE\Downloads` (если `is_dir()`)
- `Temp` — `LOCALAPPDATA\Temp`

Каждая кнопка: `self.root = ...; self.scan_total_bytes = scan_target_capacity(&self.root); self.start_scan();`

### 4.3 Автоскан после выбора папки

`choose_folder` (app.rs:136): после успешного `pick_folder()` вместо только
`self.root = folder; ...` вызвать `self.start_scan()`.

### 4.4 Валидация и сообщение об ошибке

В начале `start_scan()` (app.rs:146):

```rust
if !self.root.is_dir() {
    self.notice = Some(("Папка недоступна или не существует".to_owned(), true, Instant::now()));
    return;
}
```

### 4.5 (Опционально) Стартовое пустое состояние

Если `files.is_empty() && !scanning` — в центральной панели вместо пустой таблицы
показать подсказку: «Выберите папку и нажмите „Сканировать папку"» + большая кнопка
выбора папки. Низкое усилие, заметно улучшает первое впечатление.

### 4.6 Крайние случаи

- `pick_folder` отменён (None) — ничего не делать.
- Сканирование уже идёт, а пользователь выбрал новую папку: `start_scan()` сам
  вызывает `cancel_jobs()` (app.rs:147) — поведение уже корректное.
- `scan_target_capacity` возвращает `Option` — уже обрабатывается.

---

## Пакет 5 (опционально, пока фото-поиск не удалён). Индексация фото — по кнопке

Сейчас визуальная индексация стартует автоматически после обычного индекса
(app.rs:287 `self.start_image_indexing()` в `poll_index`).

- Убрать авто-вызов из `poll_index`.
- В сайдбаре, во вкладке «Похожие фото» (app.rs:827), если `image_engine.is_none()`
  и не идёт индексация — показывать кнопку «ИНДЕКСИРОВАТЬ ФОТО»,
  по клику `self.start_image_indexing()`.
- Больше ничего в этой фиче не менять (удаление фичи — отдельная задача).

---

## Пакет 6 (опционально). Микрооптимизации

### 6.1 Использовать готовый `path_lower`

`filtering.rs:17-18`:

```rust
// было:
|| file.path.to_string_lossy().to_lowercase().contains(query)
// стало:
|| file.path_lower.contains(query)
```

Семантика идентична (query уже в нижнем регистре), экономит аллокации на каждый файл.

### 6.2 Поиск по расширению

В `file_matches` (filtering.rs:16-18) добавить ветку: если запрос начинается с `.`,
искать `file.name_lower.ends_with(query)` (например `.iso`). Это покрывает частый
сценарий «найти все архивы» без отдельного фильтра по типу.

---

## Проверка после каждого пакета

```text
cargo fmt -- --check
cargo check
cargo test
```

Ручной смоук-тест (Windows):

1. Запуск → стартовая папка = сохранённая/домашняя; пресеты в сайдбаре работают.
2. Скан папки с мусором: чипы категорий показывают файлы/байты, клик фильтрует таблицу.
3. Выделить несколько строк (Shift-клик) → «В корзину N» → одно подтверждение с
   суммарным размером → файлы в корзине, счётчики уменьшились, строки исчезли.
4. Тултип пути, двойной клик открывает файл, правый клик — меню, Ctrl+A/Delete/Esc/Ctrl+F.
5. Выбор недоступной папки (например, удалённой) → сообщение об ошибке, скан не стартует.
