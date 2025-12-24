use crate::config::{Config, OperationType};
use crate::file_ops::{FileManager, OperationResult};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::{Backend, CrosstermBackend},
    crossterm,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, List, ListItem, ListState, Paragraph, Row, Table, Tabs},
};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

pub enum InputMode {
    Normal,
    EditingOperation,
    EditingSource,
    EditingDestination,
    EditingType,
}

pub struct App {
    pub config: Config,
    pub current_tab: usize,
    pub operations_state: ListState,
    pub input_mode: InputMode,
    pub operation_fields: Vec<(String, String, OperationType)>,
    pub current_field: usize,
    pub results: Vec<OperationResult>,
    pub show_results: bool,
    pub new_operation: (String, String, OperationType),
    pub editing_operation: (String, String, String, OperationType), // Fixed: 4-element tuple
    pub message: String,
    pub message_timer: usize,
    pub report_dir: PathBuf,
    pub show_details: bool,
    pub selected_result: Option<usize>,
    pub details_scroll: u16,
    pub edit_buffer: String,
    pub edit_cursor_position: usize,
}

impl App {
    pub fn new(config: Config, report_dir: &str) -> Self {
        let operation_fields = config
            .operations
            .iter()
            .map(|op| {
                (
                    op.name.clone(),
                    op.origin.to_string_lossy().to_string(),
                    op.destination.to_string_lossy().to_string(),
                    op.operation_type.clone(),
                )
            })
            .fold(Vec::new(), |mut acc, (name, src, dst, op_type)| {
                acc.push((name.clone(), src.clone(), op_type.clone()));
                acc.push((name, dst, op_type));
                acc
            });

        Self {
            config,
            current_tab: 0,
            operations_state: ListState::default(),
            input_mode: InputMode::Normal,
            operation_fields,
            current_field: 0,
            results: Vec::new(),
            show_results: false,
            new_operation: (String::new(), String::new(), OperationType::Copy),
            editing_operation: (
                String::new(),
                String::new(),
                String::new(),
                OperationType::Copy,
            ), // Fixed
            message: String::new(),
            message_timer: 0,
            report_dir: PathBuf::from(report_dir),
            show_details: false,
            selected_result: None,
            details_scroll: 0,
            edit_buffer: String::new(),
            edit_cursor_position: 0,
        }
    }

    pub fn show_message(&mut self, msg: String) {
        self.message = msg;
        self.message_timer = 100;
    }

    pub fn next_tab(&mut self) {
        self.current_tab = (self.current_tab + 1) % 4;
    }

    pub fn previous_tab(&mut self) {
        self.current_tab = if self.current_tab == 0 {
            3
        } else {
            self.current_tab - 1
        };
    }

    pub fn next_operation(&mut self) {
        let i = match self.operations_state.selected() {
            Some(i) => {
                if i >= self.config.operations.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.operations_state.select(Some(i));
    }

    pub fn previous_operation(&mut self) {
        let i = match self.operations_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.config.operations.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.operations_state.select(Some(i));
    }

    pub fn execute_operations(&mut self) {
        let callback: Arc<dyn Fn(String) + Send + Sync> = Arc::new(|msg| {
            println!("Progress: {}", msg);
        });

        let results = FileManager::execute_operations(&self.config.operations, &self.config.global_rate_limit, Some(callback));

        self.results = results;
        self.show_results = true;

        self.generate_reports();

        self.show_message("Operations completed! Reports saved.".to_string());
    }

    fn generate_reports(&mut self) {
        let summary_report = FileManager::generate_report(&self.results);

        let summary_path = self.report_dir.join("operation_summary.txt");
        if let Err(e) = std::fs::write(&summary_path, &summary_report) {
            self.show_message(format!("Warning: Could not save summary report: {}", e));
        } else {
            self.show_message(format!(
                "Summary report saved to {}",
                summary_path.display()
            ));
        }

        if let Err(e) = FileManager::generate_detailed_report(&self.results, &self.report_dir) {
            self.show_message(format!(
                "Warning: Could not generate detailed report: {}",
                e
            ));
        }

        match FileManager::save_operation_reports_to_destinations(&self.results) {
            Ok(saved_paths) => {
                for path in saved_paths {
                    println!("{}", path);
                }
                self.show_message(format!(
                    "{} operation reports saved to destination folders",
                    self.results.len()
                ));
            }
            Err(e) => {
                self.show_message(format!("Warning: Could not save operation reports: {}", e));
            }
        }

        match FileManager::save_file_list_reports(&self.results) {
            Ok(saved_paths) => {
                for path in saved_paths {
                    println!("{}", path);
                }
                self.show_message("File list reports saved".to_string());
            }
            Err(e) => {
                self.show_message(format!("Warning: Could not save file list reports: {}", e));
            }
        }
    }

    pub fn next_result(&mut self) {
        if self.results.is_empty() {
            self.selected_result = None;
            return;
        }

        let i = match self.selected_result {
            Some(i) => {
                if i >= self.results.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.selected_result = Some(i);
        self.details_scroll = 0;
    }

    pub fn previous_result(&mut self) {
        if self.results.is_empty() {
            self.selected_result = None;
            return;
        }

        let i = match self.selected_result {
            Some(i) => {
                if i == 0 {
                    self.results.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.selected_result = Some(i);
        self.details_scroll = 0;
    }

    pub fn scroll_details_up(&mut self) {
        if self.details_scroll > 0 {
            self.details_scroll -= 1;
        }
    }

    pub fn scroll_details_down(&mut self) {
        self.details_scroll += 1;
    }

    pub fn toggle_details(&mut self) {
        self.show_details = !self.show_details;
        if self.show_details && self.selected_result.is_none() && !self.results.is_empty() {
            self.selected_result = Some(0);
        }
    }

    pub fn start_editing(&mut self) {
        if let Some(selected_idx) = self.operations_state.selected() {
            if selected_idx < self.config.operations.len() {
                let op = &self.config.operations[selected_idx];
                self.editing_operation = (
                    op.name.clone(),
                    op.origin.to_string_lossy().to_string(),
                    op.destination.to_string_lossy().to_string(),
                    op.operation_type.clone(),
                );
                self.input_mode = InputMode::EditingOperation;
                self.edit_buffer = op.name.clone();
                self.edit_cursor_position = self.edit_buffer.len();
            }
        }
    }

    pub fn handle_edit_input(&mut self, c: char) {
        if self.edit_cursor_position <= self.edit_buffer.len() {
            self.edit_buffer.insert(self.edit_cursor_position, c);
            self.edit_cursor_position += 1;
        }
    }

    pub fn handle_backspace(&mut self) {
        if self.edit_cursor_position > 0 && self.edit_cursor_position <= self.edit_buffer.len() {
            self.edit_buffer.remove(self.edit_cursor_position - 1);
            self.edit_cursor_position -= 1;
        }
    }

    pub fn handle_delete(&mut self) {
        if self.edit_cursor_position < self.edit_buffer.len() {
            self.edit_buffer.remove(self.edit_cursor_position);
        }
    }

    pub fn move_cursor_left(&mut self) {
        if self.edit_cursor_position > 0 {
            self.edit_cursor_position -= 1;
        }
    }

    pub fn move_cursor_right(&mut self) {
        if self.edit_cursor_position < self.edit_buffer.len() {
            self.edit_cursor_position += 1;
        }
    }

    pub fn move_cursor_home(&mut self) {
        self.edit_cursor_position = 0;
    }

    pub fn move_cursor_end(&mut self) {
        self.edit_cursor_position = self.edit_buffer.len();
    }

    pub fn save_edit(&mut self) {
        if let Some(selected_idx) = self.operations_state.selected() {
            if selected_idx < self.config.operations.len() {
                match self.input_mode {
                    InputMode::EditingOperation => {
                        self.editing_operation.0 = self.edit_buffer.clone();
                        self.config.operations[selected_idx].name = self.edit_buffer.clone();
                    }
                    InputMode::EditingSource => {
                        self.editing_operation.1 = self.edit_buffer.clone();
                        self.config.operations[selected_idx].origin =
                            PathBuf::from(&self.edit_buffer);
                    }
                    InputMode::EditingDestination => {
                        self.editing_operation.2 = self.edit_buffer.clone();
                        self.config.operations[selected_idx].destination =
                            PathBuf::from(&self.edit_buffer);
                    }
                    InputMode::EditingType => {
                        if self.edit_buffer.to_lowercase() == "copy" {
                            self.editing_operation.3 = OperationType::Copy;
                            self.config.operations[selected_idx].operation_type =
                                OperationType::Copy;
                        } else if self.edit_buffer.to_lowercase() == "move" {
                            self.editing_operation.3 = OperationType::Move;
                            self.config.operations[selected_idx].operation_type =
                                OperationType::Move;
                        }
                    }
                    InputMode::Normal => {}
                }

                self.input_mode = InputMode::Normal;
                self.edit_buffer.clear();
                self.edit_cursor_position = 0;
                self.show_message("Operation updated".to_string());
            }
        }
    }

    pub fn next_edit_field(&mut self) {
        match self.input_mode {
            InputMode::EditingOperation => {
                self.save_edit();
                self.input_mode = InputMode::EditingSource;
                self.edit_buffer = self.editing_operation.1.clone();
                self.edit_cursor_position = self.edit_buffer.len();
            }
            InputMode::EditingSource => {
                self.save_edit();
                self.input_mode = InputMode::EditingDestination;
                self.edit_buffer = self.editing_operation.2.clone();
                self.edit_cursor_position = self.edit_buffer.len();
            }
            InputMode::EditingDestination => {
                self.save_edit();
                self.input_mode = InputMode::EditingType;
                self.edit_buffer = match self.editing_operation.3 {
                    OperationType::Copy => "copy".to_string(),
                    OperationType::Move => "move".to_string(),
                };
                self.edit_cursor_position = self.edit_buffer.len();
            }
            InputMode::EditingType => {
                self.save_edit();
            }
            InputMode::Normal => {}
        }
    }

    pub fn previous_edit_field(&mut self) {
        match self.input_mode {
            InputMode::EditingType => {
                self.save_edit();
                self.input_mode = InputMode::EditingDestination;
                self.edit_buffer = self.editing_operation.2.clone();
                self.edit_cursor_position = self.edit_buffer.len();
            }
            InputMode::EditingDestination => {
                self.save_edit();
                self.input_mode = InputMode::EditingSource;
                self.edit_buffer = self.editing_operation.1.clone();
                self.edit_cursor_position = self.edit_buffer.len();
            }
            InputMode::EditingSource => {
                self.save_edit();
                self.input_mode = InputMode::EditingOperation;
                self.edit_buffer = self.editing_operation.0.clone();
                self.edit_cursor_position = self.edit_buffer.len();
            }
            InputMode::EditingOperation => {
                self.save_edit();
            }
            InputMode::Normal => {}
        }
    }
}

pub fn run_app(config: Config, report_dir: &str) -> anyhow::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(config, report_dir);
    let res = run_app_internal(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("Error: {:?}", err);
    }

    Ok(())
}

fn run_app_internal<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> io::Result<()> {
    loop {
        terminal.draw(|f| ui(f, app))?;

        if let ratatui::crossterm::event::Event::Key(key) = ratatui::crossterm::event::read()? {
            if key.kind != ratatui::crossterm::event::KeyEventKind::Press {
                continue;
            }

            match app.input_mode {
                InputMode::Normal => match key.code {
                    ratatui::crossterm::event::KeyCode::Char('q') => return Ok(()),
                    ratatui::crossterm::event::KeyCode::Tab => app.next_tab(),
                    ratatui::crossterm::event::KeyCode::BackTab => app.previous_tab(),
                    ratatui::crossterm::event::KeyCode::Char('j')
                    | ratatui::crossterm::event::KeyCode::Down => match app.current_tab {
                        0 => app.next_operation(),
                        2 => app.next_result(),
                        3 => app.scroll_details_down(),
                        _ => {}
                    },
                    ratatui::crossterm::event::KeyCode::Char('k')
                    | ratatui::crossterm::event::KeyCode::Up => match app.current_tab {
                        0 => app.previous_operation(),
                        2 => app.previous_result(),
                        3 => app.scroll_details_up(),
                        _ => {}
                    },
                    ratatui::crossterm::event::KeyCode::Char('e') => {
                        app.start_editing();
                    }
                    ratatui::crossterm::event::KeyCode::Char('r') => {
                        app.execute_operations();
                    }
                    ratatui::crossterm::event::KeyCode::Char('s') => {
                        if let Err(e) = app.config.save_to_file("config.yaml") {
                            app.show_message(format!("Save failed: {}", e));
                        } else {
                            app.show_message("Configuration saved!".to_string());
                        }
                    }
                    ratatui::crossterm::event::KeyCode::Char('d') => {
                        if app.current_tab == 2 && !app.results.is_empty() {
                            app.toggle_details();
                        }
                    }
                    ratatui::crossterm::event::KeyCode::Enter => {
                        if app.current_tab == 2 && !app.results.is_empty() {
                            app.toggle_details();
                        }
                    }
                    ratatui::crossterm::event::KeyCode::Char('p') => {
                        app.show_message(format!("Report directory: {}", app.report_dir.display()));
                    }
                    _ => {}
                },
                InputMode::EditingOperation
                | InputMode::EditingSource
                | InputMode::EditingDestination
                | InputMode::EditingType => match key.code {
                    ratatui::crossterm::event::KeyCode::Esc => {
                        app.input_mode = InputMode::Normal;
                        app.edit_buffer.clear();
                        app.edit_cursor_position = 0;
                        app.show_message("Edit cancelled".to_string());
                    }
                    ratatui::crossterm::event::KeyCode::Enter => {
                        app.save_edit();
                    }
                    ratatui::crossterm::event::KeyCode::Tab => {
                        app.next_edit_field();
                    }
                    ratatui::crossterm::event::KeyCode::BackTab => {
                        app.previous_edit_field();
                    }
                    ratatui::crossterm::event::KeyCode::Left => {
                        app.move_cursor_left();
                    }
                    ratatui::crossterm::event::KeyCode::Right => {
                        app.move_cursor_right();
                    }
                    ratatui::crossterm::event::KeyCode::Home => {
                        app.move_cursor_home();
                    }
                    ratatui::crossterm::event::KeyCode::End => {
                        app.move_cursor_end();
                    }
                    ratatui::crossterm::event::KeyCode::Backspace => {
                        app.handle_backspace();
                    }
                    ratatui::crossterm::event::KeyCode::Delete => {
                        app.handle_delete();
                    }
                    ratatui::crossterm::event::KeyCode::Char(c) => {
                        app.handle_edit_input(c);
                    }
                    _ => {}
                },
            }
        }

        if app.message_timer > 0 {
            app.message_timer -= 1;
            if app.message_timer == 0 {
                app.message.clear();
            }
        }
    }
}

fn ui(f: &mut Frame, app: &mut App) {
    let size = f.area();

    let titles = vec!["Operations", "Configuration", "Results", "Details"];
    let tabs = Tabs::new(titles)
        .select(app.current_tab)
        .block(Block::default().borders(Borders::ALL).title("File Manager"))
        .style(Style::default().fg(Color::White))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .divider(Span::raw("|"));

    f.render_widget(tabs, size);

    let main_chunk = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .split(f.area());

    match app.current_tab {
        0 => render_operations_tab(f, app, main_chunk[1]),
        1 => render_config_tab(f, app, main_chunk[1]),
        2 => render_results_tab(f, app, main_chunk[1]),
        3 => render_details_tab(f, app, main_chunk[1]),
        _ => {}
    }

    if !app.message.is_empty() {
        let message_area = Rect::new(size.width / 4, size.height - 3, size.width / 2, 3);
        let message_widget = Paragraph::new(app.message.clone())
            .style(Style::default().bg(Color::Blue).fg(Color::White))
            .block(Block::default().borders(Borders::ALL))
            .alignment(Alignment::Center);
        f.render_widget(message_widget, message_area);
    }

    let help_text = match app.input_mode {
        InputMode::Normal => match app.current_tab {
            0 => "Help: ↑/↓/j/k=Select, e=Edit, Tab=Switch tabs, r=Run, s=Save, q=Quit",
            1 => "Help: Tab=Switch tabs, r=Run operations, s=Save config, q=Quit",
            2 => {
                "Help: ↑/↓/j/k=Select, Enter/d=Details, Tab=Switch tabs, p=Show report path, q=Quit"
            }
            3 => "Help: ↑/↓=Scroll, Tab=Switch tabs, q=Quit",
            _ => "Help: Tab=Switch tabs, q=Quit",
        },
        InputMode::EditingOperation
        | InputMode::EditingSource
        | InputMode::EditingDestination
        | InputMode::EditingType => {
            "EDIT MODE: ↑/↓/Tab=Navigate fields, Enter=Save, Esc=Cancel, Type to edit"
        }
    };

    let help_widget = Paragraph::new(help_text)
        .style(Style::default().fg(Color::Gray))
        .alignment(Alignment::Center);
    f.render_widget(help_widget, main_chunk[2]);

    // Render edit popup if in edit mode
    if let InputMode::EditingOperation
    | InputMode::EditingSource
    | InputMode::EditingDestination
    | InputMode::EditingType = app.input_mode
    {
        render_edit_popup(f, app, size);
    }
}

fn render_operations_tab(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let operations: Vec<ListItem> = app
        .config
        .operations
        .iter()
        .enumerate()
        .map(|(idx, op)| {
            let is_selected = app.operations_state.selected() == Some(idx);
            let name_style = if is_selected {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Yellow)
            };

            let lines = vec![
                Line::from(vec![
                    Span::raw("Name: "),
                    Span::styled(&op.name, name_style),
                ]),
                Line::from(vec![
                    Span::raw("From: "),
                    Span::styled(
                        op.origin.to_string_lossy(),
                        Style::default().fg(Color::Green),
                    ),
                ]),
                Line::from(vec![
                    Span::raw("To: "),
                    Span::styled(
                        op.destination.to_string_lossy(),
                        Style::default().fg(Color::Cyan),
                    ),
                ]),
                Line::from(vec![
                    Span::raw("Type: "),
                    Span::styled(
                        match op.operation_type {
                            OperationType::Copy => "Copy",
                            OperationType::Move => "Move",
                        },
                        Style::default().fg(Color::Magenta),
                    ),
                ]),
            ];
            ListItem::new(lines)
        })
        .collect();

    let operations_list = List::new(operations)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Operations (e to edit)"),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    f.render_stateful_widget(operations_list, chunks[0], &mut app.operations_state);

    let help_text = vec![
        Line::from("Report Directory:"),
        Line::from(Span::styled(
            app.report_dir.to_string_lossy(),
            Style::default().fg(Color::Yellow),
        )),
        Line::from(""),
        Line::from("Commands:"),
        Line::from("  ↑/↓/j/k - Select operation"),
        Line::from("  e - Edit selected operation"),
        Line::from("  Tab/Shift+Tab - Switch tabs"),
        Line::from("  r - Run operations"),
        Line::from("  s - Save config"),
        Line::from("  p - Show report path"),
        Line::from("  q - Quit"),
    ];

    let help_widget = Paragraph::new(help_text)
        .block(Block::default().borders(Borders::ALL).title("Info"))
        .alignment(Alignment::Left);

    f.render_widget(help_widget, chunks[1]);
}

fn render_config_tab(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let config_text = serde_yaml::to_string(&app.config)
        .unwrap_or_else(|_| "Failed to serialize config".to_string());

    let config_widget = Paragraph::new(config_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Configuration"),
        )
        .scroll((app.current_field as u16, 0));

    f.render_widget(config_widget, chunks[0]);

    let stats_text = vec![
        Line::from("Statistics:"),
        Line::from(format!(
            "  Total operations: {}",
            app.config.operations.len()
        )),
        Line::from(format!(
            "  Copy operations: {}",
            app.config
                .operations
                .iter()
                .filter(|op| op.operation_type == OperationType::Copy)
                .count()
        )),
        Line::from(format!(
            "  Move operations: {}",
            app.config
                .operations
                .iter()
                .filter(|op| op.operation_type == OperationType::Move)
                .count()
        )),
        Line::from(""),
        Line::from("Report Directory:"),
        Line::from(Span::styled(
            app.report_dir.to_string_lossy(),
            Style::default().fg(Color::Yellow),
        )),
    ];

    let stats_widget = Paragraph::new(stats_text)
        .block(Block::default().borders(Borders::ALL).title("Stats"))
        .alignment(Alignment::Left);

    f.render_widget(stats_widget, chunks[1]);
}

fn render_results_tab(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    if app.results.is_empty() {
        let message = Paragraph::new("No results yet. Run operations from the Operations tab.")
            .block(Block::default().borders(Borders::ALL).title("Results"))
            .alignment(Alignment::Center);
        f.render_widget(message, area);
    } else {
        let successful = app.results.iter().filter(|r| r.success).count();
        let total = app.results.len();
        let percentage = (successful as f32 / total as f32) * 100.0;

        let gauge = Gauge::default()
            .block(Block::default().borders(Borders::ALL).title("Success Rate"))
            .gauge_style(Style::default().fg(Color::Green))
            .percent(percentage as u16);

        f.render_widget(gauge, chunks[0]);

        let rows: Vec<Row> = app
            .results
            .iter()
            .enumerate()
            .map(|(idx, r)| {
                let status = if r.success { "✓" } else { "✗" };
                let verified = if r.hash_verified { "✓" } else { "✗" };
                let files_info = if r.files_processed > 0 {
                    format!("{} files", r.files_processed)
                } else {
                    "0 files".to_string()
                };

                let style = if Some(idx) == app.selected_result {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };

                Row::new(vec![
                    status.to_string(),
                    r.operation_name.clone(),
                    files_info,
                    format!("{} bytes", r.total_size),
                    verified.to_string(),
                ])
                .style(style)
            })
            .collect();

        let results_table = Table::new(
            rows,
            vec![
                Constraint::Length(3),
                Constraint::Length(20),
                Constraint::Length(12),
                Constraint::Length(15),
                Constraint::Length(3),
            ],
        )
        .header(Row::new(vec![
            "Status", "Name", "Files", "Size", "Verified",
        ]))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Results (Enter/d for details)"),
        )
        .style(Style::default().fg(Color::White));

        f.render_widget(results_table, chunks[1]);
    }
}

fn render_details_tab(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    if app.results.is_empty() {
        let message = Paragraph::new("No results available. Run operations first.")
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Operation Details"),
            )
            .alignment(Alignment::Center);
        f.render_widget(message, area);
        return;
    }

    if let Some(selected_idx) = app.selected_result {
        if selected_idx < app.results.len() {
            let result = &app.results[selected_idx];

            let mut details_text = Vec::new();

            details_text.push(Line::from(Span::styled(
                format!("Operation: {}", result.operation_name),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )));

            details_text.push(Line::from(""));
            details_text.push(Line::from(vec![
                Span::raw("Status: "),
                Span::styled(
                    if result.success { "SUCCESS" } else { "FAILED" },
                    if result.success {
                        Style::default().fg(Color::Green)
                    } else {
                        Style::default().fg(Color::Red)
                    },
                ),
            ]));

            details_text.push(Line::from(vec![
                Span::raw("Type: "),
                Span::styled(
                    format!("{:?}", result.operation_type),
                    Style::default().fg(Color::Magenta),
                ),
            ]));

            details_text.push(Line::from(format!("Source: {}", result.source)));
            details_text.push(Line::from(format!("Destination: {}", result.destination)));

            if let Some(error) = &result.error_message {
                details_text.push(Line::from(""));
                details_text.push(Line::from(Span::styled(
                    "Error Details:",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                )));

                let max_line_len = 70;
                let mut remaining = error.as_str();
                while !remaining.is_empty() {
                    let end = if remaining.len() > max_line_len {
                        let break_pos =
                            remaining[..max_line_len].rfind(' ').unwrap_or(max_line_len);
                        break_pos
                    } else {
                        remaining.len()
                    };

                    details_text.push(Line::from(format!("  {}", &remaining[..end])));
                    remaining = &remaining[end..].trim_start();
                }
            }

            if !result.file_list.is_empty() {
                details_text.push(Line::from(""));
                details_text.push(Line::from(Span::styled(
                    "File List:",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )));

                for (file_idx, file_entry) in result.file_list.iter().enumerate() {
                    let status = if file_entry.success { "✓" } else { "✗" };

                    details_text.push(Line::from(format!(
                        "  {}. {} {} -> {}",
                        file_idx + 1,
                        status,
                        file_entry.source_path,
                        file_entry.destination_path
                    )));

                    if let Some(err) = &file_entry.error_message {
                        details_text.push(Line::from(format!("     Error: {}", err)));
                    }
                }
            }

            details_text.push(Line::from(""));
            details_text.push(Line::from(Span::styled(
                "Operation Log:",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));

            for detail in result.details.iter().skip(app.details_scroll as usize) {
                details_text.push(Line::from(format!("  {}", detail)));
            }

            if app.details_scroll > 0 {
                details_text.push(Line::from(Span::styled(
                    format!("  ↑ Scrolled {} lines", app.details_scroll),
                    Style::default().fg(Color::Gray),
                )));
            }

            let details_widget = Paragraph::new(details_text)
                .block(Block::default().borders(Borders::ALL).title(format!(
                    "Details: {}/{} (↑/↓ to scroll)",
                    selected_idx + 1,
                    app.results.len()
                )))
                .scroll((0, 0));

            f.render_widget(details_widget, chunks[0]);
        }
    } else {
        let message = Paragraph::new(
            "Select a result in the Results tab and press 'd' or Enter to view details.",
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Operation Details"),
        )
        .alignment(Alignment::Center);
        f.render_widget(message, area);
        return;
    }

    let report_info = vec![
        Line::from("Report Directory:"),
        Line::from(Span::styled(
            app.report_dir.to_string_lossy(),
            Style::default().fg(Color::Yellow),
        )),
        Line::from(""),
        Line::from("Report Files:"),
        Line::from("  • operation_summary.txt"),
        Line::from("  • file_operations_report_*.txt"),
        Line::from("  • file_list_report_*.txt"),
        Line::from("  • file_list_*.txt (in destination folders)"),
        Line::from(""),
        Line::from("Navigation:"),
        Line::from("  ↑/↓ - Scroll details"),
        Line::from("  Tab - Switch tabs"),
        Line::from("  q - Quit"),
    ];

    let info_widget = Paragraph::new(report_info)
        .block(Block::default().borders(Borders::ALL).title("Information"))
        .alignment(Alignment::Left);

    f.render_widget(info_widget, chunks[1]);
}

fn render_edit_popup(f: &mut Frame, app: &mut App, size: Rect) {
    let popup_width = 60;
    let popup_height = 12;
    let popup_x = (size.width - popup_width) / 2;
    let popup_y = (size.height - popup_height) / 2;

    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    let popup_block = Block::default()
        .borders(Borders::ALL)
        .style(Style::default().bg(Color::DarkGray))
        .title("Edit Operation");

    f.render_widget(popup_block, popup_area);

    let inner_area = Rect::new(
        popup_area.x + 2,
        popup_area.y + 2,
        popup_area.width - 4,
        popup_area.height - 4,
    );

    let field_name = match app.input_mode {
        InputMode::EditingOperation => "Operation Name",
        InputMode::EditingSource => "Source Path",
        InputMode::EditingDestination => "Destination Path",
        InputMode::EditingType => "Operation Type (copy/move)",
        InputMode::Normal => "",
    };

    let current_value = match app.input_mode {
        InputMode::EditingOperation => &app.editing_operation.0,
        InputMode::EditingSource => &app.editing_operation.1,
        InputMode::EditingDestination => &app.editing_operation.2,
        InputMode::EditingType => match app.editing_operation.3 {
            OperationType::Copy => "copy",
            OperationType::Move => "move",
        },
        InputMode::Normal => "",
    };

    let field_text = vec![
        Line::from(format!("Editing: {}", field_name)),
        Line::from(""),
        Line::from("Current value:"),
        Line::from(Span::styled(
            current_value,
            Style::default().fg(Color::Yellow),
        )),
        Line::from(""),
        Line::from("New value:"),
    ];

    let field_widget = Paragraph::new(field_text)
        .block(Block::default().borders(Borders::NONE))
        .alignment(Alignment::Left);

    f.render_widget(field_widget, inner_area);

    let input_area = Rect::new(inner_area.x, inner_area.y + 6, inner_area.width, 3);

    let input_block = Block::default()
        .borders(Borders::ALL)
        .style(Style::default().bg(Color::Black));

    f.render_widget(input_block, input_area);

    let input_text = format!("{}", app.edit_buffer);
    let input_widget = Paragraph::new(input_text)
        .block(Block::default().borders(Borders::NONE))
        .style(Style::default().fg(Color::White));

    f.render_widget(input_widget, input_area);

    let cursor_x = input_area.x + 1 + app.edit_cursor_position as u16;
    let cursor_y = input_area.y + 1;

    if cursor_x < input_area.x + input_area.width - 1 {
        f.set_cursor_position((cursor_x, cursor_y)); // CORRECTED: Pass as tuple
    }

    let help_area = Rect::new(inner_area.x, inner_area.y + 9, inner_area.width, 3);

    let help_text = vec![
        Line::from("Tab/Shift+Tab: Next/Prev field | Enter: Save | Esc: Cancel"),
        Line::from("↑/↓/Home/End: Move cursor | Backspace/Delete: Delete"),
    ];

    let help_widget = Paragraph::new(help_text)
        .block(Block::default().borders(Borders::NONE))
        .style(Style::default().fg(Color::Gray))
        .alignment(Alignment::Center);

    f.render_widget(help_widget, help_area);
}
