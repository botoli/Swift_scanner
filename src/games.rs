use eframe::egui::{self, Color32, RichText};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, PartialEq, Eq)]
enum GameKind {
    ClickSprint,
    Reaction,
    GuessNumber,
    HigherLower,
    RockPaperScissors,
    Coin,
    Dice,
    Math,
    Memory,
    TicTacToe,
    Snake,
    Dinosaur,
}

impl GameKind {
    const ALL: [Self; 12] = [
        Self::ClickSprint,
        Self::Reaction,
        Self::GuessNumber,
        Self::HigherLower,
        Self::RockPaperScissors,
        Self::Coin,
        Self::Dice,
        Self::Math,
        Self::Memory,
        Self::TicTacToe,
        Self::Snake,
        Self::Dinosaur,
    ];

    fn meta(self) -> (&'static str, &'static str) {
        match self {
            Self::ClickSprint => ("Клик-спринт", "Сделайте 30 кликов"),
            Self::Reaction => ("Реакция", "Дождитесь сигнала"),
            Self::GuessNumber => ("Угадай число", "Диапазон от 1 до 100"),
            Self::HigherLower => ("Больше или меньше", "Угадайте следующую карту"),
            Self::RockPaperScissors => ("Камень, ножницы, бумага", "Раунд против сканера"),
            Self::Coin => ("Монетка", "Орёл или решка"),
            Self::Dice => ("Кости", "Бросок против сканера"),
            Self::Math => ("Устный счёт", "Короткие примеры"),
            Self::Memory => ("Память", "Запомните четыре цифры"),
            Self::TicTacToe => ("Крестики-нолики", "Поле три на три"),
            Self::Snake => ("Змейка", "Стрелки или WASD"),
            Self::Dinosaur => ("Динозаврик", "Прыжки через препятствия"),
        }
    }
}

pub(crate) struct MiniGames {
    active: Option<GameKind>,
    rng_state: u64,
    score: u32,
    rounds: u32,
    input: String,
    message: String,
    target: u32,
    current: u32,
    reaction_wait_until: Option<Instant>,
    reaction_started: Option<Instant>,
    memory_visible_until: Option<Instant>,
    memory_answer_ready: bool,
    math_left: u32,
    math_right: u32,
    board: [u8; 9],
    snake: Vec<(i32, i32)>,
    snake_direction: (i32, i32),
    snake_food: (i32, i32),
    snake_last_tick: Instant,
    snake_over: bool,
    dino_y: f32,
    dino_velocity: f32,
    dino_obstacles: Vec<f32>,
    dino_last_frame: Instant,
    dino_spawn_in: f32,
    dino_distance: f32,
    dino_running: bool,
    dino_over: bool,
}

impl MiniGames {
    pub(crate) fn new() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0x9E37_79B9_7F4A_7C15, |value| value.as_nanos() as u64);
        Self {
            active: None,
            rng_state: seed | 1,
            score: 0,
            rounds: 0,
            input: String::new(),
            message: String::new(),
            target: 0,
            current: 0,
            reaction_wait_until: None,
            reaction_started: None,
            memory_visible_until: None,
            memory_answer_ready: false,
            math_left: 0,
            math_right: 0,
            board: [0; 9],
            snake: Vec::new(),
            snake_direction: (1, 0),
            snake_food: (16, 8),
            snake_last_tick: Instant::now(),
            snake_over: false,
            dino_y: 0.0,
            dino_velocity: 0.0,
            dino_obstacles: Vec::new(),
            dino_last_frame: Instant::now(),
            dino_spawn_in: 1.4,
            dino_distance: 0.0,
            dino_running: false,
            dino_over: false,
        }
    }

    pub(crate) fn ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.active.is_some() {
            self.show_active(ui, ctx);
        } else {
            self.show_catalog(ui);
        }
    }

    fn open(&mut self, game: GameKind) {
        self.active = Some(game);
        self.score = 0;
        self.rounds = 0;
        self.input.clear();
        self.message.clear();
        self.reaction_wait_until = None;
        self.reaction_started = None;
        self.memory_visible_until = None;
        self.memory_answer_ready = false;
        self.board = [0; 9];
        self.target = self.random_below(100) + 1;
        self.current = self.random_below(13) + 1;
        self.math_left = self.random_below(40) + 10;
        self.math_right = self.random_below(20) + 1;
        if game == GameKind::Snake {
            self.reset_snake();
        }
        if game == GameKind::Dinosaur {
            self.reset_dinosaur();
        }
    }

    fn random_below(&mut self, upper: u32) -> u32 {
        self.rng_state ^= self.rng_state << 13;
        self.rng_state ^= self.rng_state >> 7;
        self.rng_state ^= self.rng_state << 17;
        (self.rng_state % u64::from(upper.max(1))) as u32
    }

    fn show_catalog(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("12 КОРОТКИХ ИГР")
                .size(9.0)
                .strong()
                .color(crate::BLUE),
        );
        ui.add_space(4.0);
        ui.label(
            RichText::new("Игры доступны в любое время и не мешают сканированию.")
                .size(13.0)
                .color(crate::INK),
        );
        ui.add_space(14.0);

        let mut chosen = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.columns(2, |columns| {
                    for (index, game) in GameKind::ALL.into_iter().enumerate() {
                        let column = &mut columns[index % 2];
                        if game_card(column, index + 1, game).clicked() {
                            chosen = Some(game);
                        }
                        column.add_space(8.0);
                    }
                });
            });
        if let Some(game) = chosen {
            self.open(game);
        }
    }

    fn show_active(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let game = self.active.expect("active game is checked");
        let (title, description) = game.meta();
        let mut go_back = false;
        let mut restart = false;
        ui.horizontal(|ui| {
            if ui.button("К КАТАЛОГУ").clicked() {
                go_back = true;
            }
            if ui.button("НАЧАТЬ ЗАНОВО").clicked() {
                restart = true;
            }
        });
        if go_back {
            self.active = None;
            return;
        }
        if restart {
            self.open(game);
        }

        ui.add_space(14.0);
        ui.label(RichText::new(title).size(24.0).strong().color(crate::INK));
        ui.label(RichText::new(description).size(11.0).color(crate::MUTED));
        ui.add_space(18.0);

        egui::Frame::new()
            .fill(crate::SURFACE)
            .stroke(egui::Stroke::new(1.0_f32, crate::LINE))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::same(18))
            .show(ui, |ui| match game {
                GameKind::ClickSprint => {
                    ui.label(format!("Клики: {} / 30", self.score));
                    if ui
                        .add_sized(
                            [220.0, 72.0],
                            egui::Button::new(RichText::new("КЛИК").size(18.0).strong()),
                        )
                        .clicked()
                        && self.score < 30
                    {
                        self.score += 1;
                        if self.score == 30 {
                            self.message = "Финиш. Серия завершена.".to_owned();
                        }
                    }
                }
                GameKind::Reaction => {
                    let now = Instant::now();
                    if self.reaction_wait_until.is_none() {
                        if ui.button("НАЧАТЬ").clicked() {
                            let delay = 700 + self.random_below(2_300) as u64;
                            self.reaction_wait_until = Some(now + Duration::from_millis(delay));
                            self.reaction_started = None;
                            self.message = "Ждите смены сигнала...".to_owned();
                        }
                    } else if now < self.reaction_wait_until.expect("wait time exists") {
                        if ui
                            .add_sized([220.0, 72.0], egui::Button::new("ЖДИТЕ"))
                            .clicked()
                        {
                            self.reaction_wait_until = None;
                            self.message = "Слишком рано. Попробуйте ещё раз.".to_owned();
                        }
                        ctx.request_repaint_after(Duration::from_millis(16));
                    } else {
                        let started = *self.reaction_started.get_or_insert(now);
                        if ui
                            .add_sized(
                                [220.0, 72.0],
                                egui::Button::new(
                                    RichText::new("ЖМИТЕ").strong().color(Color32::WHITE),
                                )
                                .fill(Color32::from_rgb(91, 201, 151)),
                            )
                            .clicked()
                        {
                            self.message = format!("Реакция: {} мс", started.elapsed().as_millis());
                            self.reaction_wait_until = None;
                            self.reaction_started = None;
                        }
                        ctx.request_repaint_after(Duration::from_millis(16));
                    }
                }
                GameKind::GuessNumber => {
                    ui.label(format!("Попыток: {}", self.rounds));
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [120.0, 30.0],
                            egui::TextEdit::singleline(&mut self.input).hint_text("1–100"),
                        );
                        if ui.button("ПРОВЕРИТЬ").clicked() {
                            if let Ok(value) = self.input.trim().parse::<u32>() {
                                self.rounds += 1;
                                self.message = if value < self.target {
                                    "Загаданное число больше.".to_owned()
                                } else if value > self.target {
                                    "Загаданное число меньше.".to_owned()
                                } else {
                                    "Угадано. Начат новый раунд.".to_owned()
                                };
                                if value == self.target {
                                    self.score += 1;
                                    self.target = self.random_below(100) + 1;
                                    self.rounds = 0;
                                }
                                self.input.clear();
                            } else {
                                self.message = "Введите целое число.".to_owned();
                            }
                        }
                    });
                    ui.label(format!("Победы: {}", self.score));
                }
                GameKind::HigherLower => {
                    ui.label(RichText::new(format!("Текущая карта: {}", self.current)).size(20.0));
                    let mut choice = None;
                    ui.horizontal(|ui| {
                        if ui.button("СЛЕДУЮЩАЯ БОЛЬШЕ").clicked() {
                            choice = Some(true);
                        }
                        if ui.button("СЛЕДУЮЩАЯ МЕНЬШЕ").clicked() {
                            choice = Some(false);
                        }
                    });
                    if let Some(higher) = choice {
                        let mut next = self.random_below(13) + 1;
                        while next == self.current {
                            next = self.random_below(13) + 1;
                        }
                        let correct =
                            (higher && next > self.current) || (!higher && next < self.current);
                        if correct {
                            self.score += 1;
                            self.message = format!("Верно: выпало {next}.");
                        } else {
                            self.score = 0;
                            self.message = format!("Не угадано: выпало {next}.");
                        }
                        self.current = next;
                    }
                    ui.label(format!("Серия: {}", self.score));
                }
                GameKind::RockPaperScissors => {
                    let mut choice = None;
                    ui.horizontal(|ui| {
                        for (index, label) in ["КАМЕНЬ", "НОЖНИЦЫ", "БУМАГА"].iter().enumerate()
                        {
                            if ui.button(*label).clicked() {
                                choice = Some(index as u32);
                            }
                        }
                    });
                    if let Some(player) = choice {
                        let computer = self.random_below(3);
                        let names = ["камень", "ножницы", "бумага"];
                        let won = matches!((player, computer), (0, 1) | (1, 2) | (2, 0));
                        self.rounds += 1;
                        if won {
                            self.score += 1;
                        }
                        let result = if player == computer {
                            "ничья"
                        } else if won {
                            "победа"
                        } else {
                            "поражение"
                        };
                        self.message =
                            format!("Сканер выбрал {}: {result}.", names[computer as usize]);
                    }
                    ui.label(format!("Победы: {} / {}", self.score, self.rounds));
                }
                GameKind::Coin => {
                    let mut choice = None;
                    ui.horizontal(|ui| {
                        if ui.button("ОРЁЛ").clicked() {
                            choice = Some(0);
                        }
                        if ui.button("РЕШКА").clicked() {
                            choice = Some(1);
                        }
                    });
                    if let Some(player) = choice {
                        let result = self.random_below(2);
                        self.rounds += 1;
                        if player == result {
                            self.score += 1;
                            self.message = "Угадано.".to_owned();
                        } else {
                            self.message = "Не угадано.".to_owned();
                        }
                    }
                    ui.label(format!("Угадано: {} / {}", self.score, self.rounds));
                }
                GameKind::Dice => {
                    if ui.button("БРОСИТЬ КОСТИ").clicked() {
                        let player = self.random_below(6) + 1;
                        let computer = self.random_below(6) + 1;
                        self.rounds += 1;
                        if player > computer {
                            self.score += 1;
                        }
                        let result = if player > computer {
                            "победа"
                        } else if player == computer {
                            "ничья"
                        } else {
                            "поражение"
                        };
                        self.message = format!("Вы: {player}, сканер: {computer} — {result}.");
                    }
                    ui.label(format!("Победы: {} / {}", self.score, self.rounds));
                }
                GameKind::Math => {
                    ui.label(
                        RichText::new(format!("{} + {} = ?", self.math_left, self.math_right))
                            .size(20.0),
                    );
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [120.0, 30.0],
                            egui::TextEdit::singleline(&mut self.input).hint_text("Ответ"),
                        );
                        if ui.button("ОТВЕТИТЬ").clicked() {
                            self.rounds += 1;
                            if self.input.trim().parse::<u32>().ok()
                                == Some(self.math_left + self.math_right)
                            {
                                self.score += 1;
                                self.message = "Верно.".to_owned();
                            } else {
                                self.message = format!(
                                    "Правильный ответ: {}.",
                                    self.math_left + self.math_right
                                );
                            }
                            self.input.clear();
                            self.math_left = self.random_below(40) + 10;
                            self.math_right = self.random_below(20) + 1;
                        }
                    });
                    ui.label(format!("Верно: {} / {}", self.score, self.rounds));
                }
                GameKind::Memory => {
                    let now = Instant::now();
                    if let Some(until) = self.memory_visible_until {
                        if now < until {
                            ui.label(
                                RichText::new(format!("{:04}", self.target))
                                    .size(36.0)
                                    .strong(),
                            );
                            ui.label("Запоминайте...");
                            ctx.request_repaint_after(Duration::from_millis(50));
                        } else {
                            self.memory_visible_until = None;
                            self.memory_answer_ready = true;
                            ctx.request_repaint();
                        }
                    } else if self.memory_answer_ready {
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                [120.0, 30.0],
                                egui::TextEdit::singleline(&mut self.input)
                                    .hint_text("Четыре цифры"),
                            );
                            if ui.button("ПРОВЕРИТЬ").clicked() {
                                self.rounds += 1;
                                if self.input.trim().parse::<u32>().ok() == Some(self.target) {
                                    self.score += 1;
                                    self.message = "Точно.".to_owned();
                                } else {
                                    self.message = format!("Число было {:04}.", self.target);
                                }
                                self.input.clear();
                                self.memory_answer_ready = false;
                            }
                        });
                    } else if ui.button("ПОКАЗАТЬ ЧИСЛО").clicked() {
                        self.target = self.random_below(9_000) + 1_000;
                        self.memory_visible_until = Some(now + Duration::from_secs(3));
                        self.message.clear();
                    }
                    ui.label(format!("Верно: {} / {}", self.score, self.rounds));
                }
                GameKind::TicTacToe => {
                    let winner = tic_tac_toe_winner(&self.board);
                    let game_over = winner.is_some() || self.board.iter().all(|cell| *cell != 0);
                    let mut clicked_cell = None;
                    egui::Grid::new("tic-tac-toe")
                        .spacing([6.0, 6.0])
                        .show(ui, |ui| {
                            for index in 0..9 {
                                let label = match self.board[index] {
                                    1 => "X",
                                    2 => "O",
                                    _ => " ",
                                };
                                if ui
                                    .add_enabled(
                                        !game_over && self.board[index] == 0,
                                        egui::Button::new(RichText::new(label).size(22.0))
                                            .min_size(egui::Vec2::splat(54.0)),
                                    )
                                    .clicked()
                                {
                                    clicked_cell = Some(index);
                                }
                                if index % 3 == 2 {
                                    ui.end_row();
                                }
                            }
                        });
                    if let Some(index) = clicked_cell {
                        self.board[index] = 1;
                        if tic_tac_toe_winner(&self.board) == Some(1) {
                            self.message = "Вы победили.".to_owned();
                        } else if !self.board.iter().all(|cell| *cell != 0) {
                            let free = self
                                .board
                                .iter()
                                .enumerate()
                                .filter_map(|(index, cell)| (*cell == 0).then_some(index))
                                .collect::<Vec<_>>();
                            let computer_index =
                                free[self.random_below(free.len() as u32) as usize];
                            self.board[computer_index] = 2;
                            if tic_tac_toe_winner(&self.board) == Some(2) {
                                self.message = "Сканер победил.".to_owned();
                            } else if self.board.iter().all(|cell| *cell != 0) {
                                self.message = "Ничья.".to_owned();
                            }
                        } else {
                            self.message = "Ничья.".to_owned();
                        }
                    }
                }
                GameKind::Snake => self.show_snake(ui, ctx),
                GameKind::Dinosaur => self.show_dinosaur(ui, ctx),
            });

        if !self.message.is_empty() {
            ui.add_space(12.0);
            ui.label(RichText::new(&self.message).size(12.0).color(crate::BLUE));
        }
    }

    fn reset_snake(&mut self) {
        self.snake = vec![(7, 7), (6, 7), (5, 7), (4, 7)];
        self.snake_direction = (1, 0);
        self.snake_food = (16, 8);
        self.snake_last_tick = Instant::now();
        self.snake_over = false;
        self.score = 0;
        self.message.clear();
    }

    fn show_snake(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        const COLUMNS: i32 = 24;
        const ROWS: i32 = 16;
        let requested_direction = ui.input(|input| {
            if input.key_pressed(egui::Key::ArrowUp) || input.key_pressed(egui::Key::W) {
                Some((0, -1))
            } else if input.key_pressed(egui::Key::ArrowDown) || input.key_pressed(egui::Key::S) {
                Some((0, 1))
            } else if input.key_pressed(egui::Key::ArrowLeft) || input.key_pressed(egui::Key::A) {
                Some((-1, 0))
            } else if input.key_pressed(egui::Key::ArrowRight) || input.key_pressed(egui::Key::D) {
                Some((1, 0))
            } else {
                None
            }
        });
        if let Some(direction) = requested_direction
            && (direction.0 + self.snake_direction.0 != 0
                || direction.1 + self.snake_direction.1 != 0)
        {
            self.snake_direction = direction;
        }

        let now = Instant::now();
        if !self.snake_over
            && now.duration_since(self.snake_last_tick) >= Duration::from_millis(135)
        {
            let head = self.snake[0];
            let next = (
                head.0 + self.snake_direction.0,
                head.1 + self.snake_direction.1,
            );
            if next.0 < 0
                || next.0 >= COLUMNS
                || next.1 < 0
                || next.1 >= ROWS
                || self.snake.contains(&next)
            {
                self.snake_over = true;
                self.message = format!("Столкновение. Счёт: {}.", self.score);
            } else {
                self.snake.insert(0, next);
                if next == self.snake_food {
                    self.score += 1;
                    for _ in 0..128 {
                        let candidate = (
                            self.random_below(COLUMNS as u32) as i32,
                            self.random_below(ROWS as u32) as i32,
                        );
                        if !self.snake.contains(&candidate) {
                            self.snake_food = candidate;
                            break;
                        }
                    }
                } else {
                    self.snake.pop();
                }
            }
            self.snake_last_tick = now;
        }

        ui.label(format!("Счёт: {}", self.score));
        ui.label(
            RichText::new("Управление: стрелки или WASD")
                .size(10.0)
                .color(crate::MUTED),
        );
        ui.add_space(8.0);
        let cell = (ui.available_width().min(760.0) / COLUMNS as f32)
            .floor()
            .clamp(12.0, 28.0);
        let board_size = egui::vec2(cell * COLUMNS as f32, cell * ROWS as f32);
        let (rect, _) = ui.allocate_exact_size(board_size, egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 3.0, crate::CANVAS);
        painter.rect_stroke(
            rect,
            3.0,
            egui::Stroke::new(1.0_f32, crate::LINE),
            egui::StrokeKind::Inside,
        );
        let cell_rect = |position: (i32, i32)| {
            egui::Rect::from_min_size(
                rect.min + egui::vec2(position.0 as f32 * cell, position.1 as f32 * cell),
                egui::Vec2::splat(cell),
            )
            .shrink(1.5)
        };
        painter.rect_filled(
            cell_rect(self.snake_food),
            3.0,
            Color32::from_rgb(244, 174, 91),
        );
        for (index, segment) in self.snake.iter().enumerate() {
            painter.rect_filled(
                cell_rect(*segment),
                2.0,
                if index == 0 {
                    Color32::from_rgb(91, 201, 151)
                } else {
                    crate::BLUE
                },
            );
        }
        if self.snake_over {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "СТОЛКНОВЕНИЕ",
                egui::FontId::monospace(18.0),
                crate::INK,
            );
        } else {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }

    fn reset_dinosaur(&mut self) {
        self.dino_y = 0.0;
        self.dino_velocity = 0.0;
        self.dino_obstacles.clear();
        self.dino_last_frame = Instant::now();
        self.dino_spawn_in = 1.4;
        self.dino_distance = 0.0;
        self.dino_running = false;
        self.dino_over = false;
        self.score = 0;
        self.message.clear();
    }

    fn show_dinosaur(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.label(format!("Дистанция: {}", self.dino_distance as u32));
        ui.label(
            RichText::new("Прыжок: пробел, стрелка вверх или клик по полю")
                .size(10.0)
                .color(crate::MUTED),
        );
        ui.add_space(8.0);
        let canvas_size = egui::vec2(ui.available_width().min(820.0).max(420.0), 280.0);
        let (rect, response) = ui.allocate_exact_size(canvas_size, egui::Sense::click());
        let jump = response.clicked()
            || ui.input(|input| {
                input.key_pressed(egui::Key::Space)
                    || input.key_pressed(egui::Key::ArrowUp)
                    || input.key_pressed(egui::Key::W)
            });
        let now = Instant::now();
        let delta = now
            .duration_since(self.dino_last_frame)
            .as_secs_f32()
            .min(0.05);
        self.dino_last_frame = now;

        if jump && !self.dino_over {
            self.dino_running = true;
            if self.dino_y <= 0.5 {
                self.dino_velocity = 470.0;
            }
        }
        if self.dino_running && !self.dino_over {
            self.dino_velocity -= 1_150.0 * delta;
            self.dino_y = (self.dino_y + self.dino_velocity * delta).max(0.0);
            if self.dino_y == 0.0 && self.dino_velocity < 0.0 {
                self.dino_velocity = 0.0;
            }
            let speed = 285.0 + (self.dino_distance * 0.22).min(150.0);
            for obstacle in &mut self.dino_obstacles {
                *obstacle -= speed * delta;
            }
            self.dino_obstacles.retain(|position| *position > -32.0);
            self.dino_spawn_in -= delta;
            if self.dino_spawn_in <= 0.0 {
                self.dino_obstacles.push(rect.width() + 24.0);
                self.dino_spawn_in = 1.05 + self.random_below(110) as f32 / 100.0;
            }
            self.dino_distance += delta * 10.0;
            let collided = self
                .dino_obstacles
                .iter()
                .any(|position| *position < 96.0 && *position + 22.0 > 55.0 && self.dino_y < 43.0);
            if collided {
                self.dino_over = true;
                self.message = format!("Столкновение. Дистанция: {}.", self.dino_distance as u32);
            }
        }

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 3.0, Color32::from_rgb(20, 25, 33));
        painter.rect_stroke(
            rect,
            3.0,
            egui::Stroke::new(1.0_f32, crate::LINE),
            egui::StrokeKind::Inside,
        );
        let ground_y = rect.bottom() - 34.0;
        painter.line_segment(
            [
                egui::pos2(rect.left(), ground_y),
                egui::pos2(rect.right(), ground_y),
            ],
            egui::Stroke::new(2.0_f32, crate::MUTED),
        );
        let dino_x = rect.left() + 58.0;
        let dino_base = ground_y - self.dino_y;
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(dino_x, dino_base - 31.0), egui::vec2(29.0, 27.0)),
            1.0,
            crate::INK,
        );
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(dino_x + 20.0, dino_base - 45.0),
                egui::vec2(25.0, 20.0),
            ),
            1.0,
            crate::INK,
        );
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(dino_x - 13.0, dino_base - 23.0),
                egui::vec2(17.0, 8.0),
            ),
            1.0,
            crate::INK,
        );
        for leg_x in [dino_x + 4.0, dino_x + 20.0] {
            painter.rect_filled(
                egui::Rect::from_min_size(egui::pos2(leg_x, dino_base - 7.0), egui::vec2(6.0, 9.0)),
                0.0,
                crate::INK,
            );
        }
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(dino_x + 37.0, dino_base - 40.0),
                egui::Vec2::splat(3.0),
            ),
            0.0,
            crate::CANVAS,
        );
        for obstacle in &self.dino_obstacles {
            let x = rect.left() + *obstacle;
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(x + 7.0, ground_y - 42.0),
                    egui::vec2(9.0, 42.0),
                ),
                1.0,
                Color32::from_rgb(91, 201, 151),
            );
            painter.rect_filled(
                egui::Rect::from_min_size(egui::pos2(x, ground_y - 28.0), egui::vec2(23.0, 8.0)),
                1.0,
                Color32::from_rgb(91, 201, 151),
            );
        }
        painter.text(
            rect.right_top() + egui::vec2(-12.0, 12.0),
            egui::Align2::RIGHT_TOP,
            format!("{:05}", self.dino_distance as u32),
            egui::FontId::monospace(14.0),
            crate::MUTED,
        );
        if !self.dino_running {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "ПРОБЕЛ ИЛИ КЛИК — СТАРТ",
                egui::FontId::monospace(16.0),
                crate::INK,
            );
        } else if self.dino_over {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "СТОЛКНОВЕНИЕ",
                egui::FontId::monospace(18.0),
                crate::INK,
            );
        } else {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
}

fn game_card(ui: &mut egui::Ui, number: usize, game: GameKind) -> egui::Response {
    let (title, description) = game.meta();
    let response = egui::Frame::new()
        .fill(crate::SURFACE)
        .stroke(egui::Stroke::new(1.0_f32, crate::LINE))
        .corner_radius(3.0)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_min_height(58.0);
            ui.set_min_width((ui.available_width() - 8.0).max(240.0));
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{number:02}"))
                        .monospace()
                        .size(16.0)
                        .strong()
                        .color(crate::BLUE),
                );
                ui.vertical(|ui| {
                    ui.label(RichText::new(title).size(12.0).strong().color(crate::INK));
                    ui.label(RichText::new(description).size(10.0).color(crate::MUTED));
                });
            });
        })
        .response
        .interact(egui::Sense::click());
    if response.hovered() {
        ui.painter().rect_stroke(
            response.rect,
            3.0,
            egui::Stroke::new(1.0_f32, crate::BLUE),
            egui::StrokeKind::Inside,
        );
    }
    response
}

fn tic_tac_toe_winner(board: &[u8; 9]) -> Option<u8> {
    const LINES: [[usize; 3]; 8] = [
        [0, 1, 2],
        [3, 4, 5],
        [6, 7, 8],
        [0, 3, 6],
        [1, 4, 7],
        [2, 5, 8],
        [0, 4, 8],
        [2, 4, 6],
    ];
    LINES.iter().find_map(|line| {
        let value = board[line[0]];
        (value != 0 && value == board[line[1]] && value == board[line[2]]).then_some(value)
    })
}
