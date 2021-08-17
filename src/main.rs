#![deny(clippy::pedantic)]
#![allow(
    clippy::let_underscore_drop,
    clippy::cast_possible_truncation,
    clippy::too_many_lines,
    clippy::module_name_repetitions
)]

use chat::{ChannelType, ResolvedChannel};
use directories::ProjectDirs;
use futures_util::StreamExt;
use nadylib::{AOSocket, SocketConfig};
use tokio::sync::{mpsc::unbounded_channel, oneshot};
use tui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::Text,
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

use std::{
    fs::{create_dir_all, write},
    io,
};

use crate::chat::{Command, StateQuery, UiUpdate};

mod chat;
mod command;
mod config;
mod input;
mod term;
mod util;

const ORANGE: Color = Color::Rgb(232, 149, 6);

#[derive(PartialEq, Eq)]
enum InputMode {
    Command,
    Chat,
    Scroll,
}

struct App<'a> {
    current_mode: InputMode,
    channel_switcher_open: bool,
    channel_switcher_state: ListState,
    channel_switcher_channels: Vec<ResolvedChannel>,
    current_channel: ResolvedChannel,
    input_text: String,
    status_text: String,
    messages: Text<'a>,
    scroll_y: usize,
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let project_dirs = ProjectDirs::from("org", "Nadybot", "ao-chat-client")
        .expect("No valid home directory path provided by OS");
    let mut config_path = project_dirs.config_dir().to_path_buf();

    if !config_path.exists() {
        create_dir_all(&config_path)?;
    }

    config_path.push("config.txt");

    if !config_path.exists() {
        write(&config_path, "USERNAME=\nPASSWORD=\nCHARNAME=\n")?;
        println!(
            "No configuration file found, I created one at {:?}. Please fill it in.",
            config_path
        );
        std::process::exit(1);
    }

    let config = config::load(&config_path)
        .expect("Failed to read config file, please check formatting and permissions");

    let (mut terminal, _cleanup) = term::init_crossterm()?;
    terminal.clear()?;

    let mut input = input::EventStream::new();

    let sock = AOSocket::connect("chat.d1.funcom.com:7105", SocketConfig::default())
        .await
        .unwrap();
    let mut app = App {
        current_mode: InputMode::Command,
        channel_switcher_open: false,
        channel_switcher_state: ListState::default(),
        channel_switcher_channels: Vec::new(),
        current_channel: ResolvedChannel {
            id: 0,
            name: String::from("Vicinity"),
            r#type: ChannelType::Vicinity,
        },
        input_text: String::new(),
        status_text: String::from("Initialized"),
        messages: Text::raw(""),
        scroll_y: 0,
    };

    let (state_query_sender, state_query_receiver) = unbounded_channel();
    let (command_sender, command_receiver) = unbounded_channel();
    let (ui_update_sender, mut ui_update_receiver) = unbounded_channel();
    tokio::spawn(chat::chat_task(
        sock,
        state_query_receiver,
        command_receiver,
        ui_update_sender,
        config.user_name.clone(),
        config.character_name.clone(),
        config.password.clone(),
    ));

    loop {
        terminal.draw(|f| {
            // Split up into chat layer and two bars
            let size = f.size();
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(0)
                .constraints(
                    [
                        Constraint::Min(0),
                        Constraint::Length(1),
                        Constraint::Length(1),
                    ]
                    .as_ref(),
                )
                .split(size);

            // Set background look
            let block = Block::default().style(
                Style::default()
                    .bg(Color::Rgb(51, 51, 51))
                    .fg(Color::LightYellow),
            );
            f.render_widget(block, size);

            let chat_block = Paragraph::new(app.messages.clone())
                .scroll((app.scroll_y as u16, 0))
                .wrap(Wrap { trim: false })
                .block(Block::default());
            f.render_widget(chat_block, chunks[0]);

            // Status bar
            let status_bar = match app.current_mode {
                InputMode::Command => {
                    Paragraph::new(format!("[Mode: Command] {}", app.status_text))
                        .block(Block::default().style(Style::default().bg(ORANGE).fg(Color::Black)))
                        .alignment(Alignment::Left)
                        .wrap(Wrap { trim: true })
                }
                InputMode::Scroll => Paragraph::new(format!("[Mode: Scroll] {}", app.status_text))
                    .block(Block::default().style(Style::default().bg(Color::Red).fg(Color::White)))
                    .alignment(Alignment::Left)
                    .wrap(Wrap { trim: true }),
                InputMode::Chat => Paragraph::new(format!("[Mode: Chat] {}", app.status_text))
                    .block(
                        Block::default().style(Style::default().bg(Color::Blue).fg(Color::White)),
                    )
                    .alignment(Alignment::Left)
                    .wrap(Wrap { trim: true }),
            };
            f.render_widget(status_bar, chunks[1]);

            let input_bar =
                Block::default().style(Style::default().bg(Color::Black).fg(Color::White));
            f.render_widget(input_bar, chunks[2]);

            let input_paragraph = Paragraph::new(app.input_text.as_str());

            if let InputMode::Chat = app.current_mode {
                let channel_text = format!("[{}]", app.current_channel.render());

                let input_bar_layout = Layout::default()
                    .direction(Direction::Horizontal)
                    .margin(0)
                    .constraints(
                        [
                            Constraint::Length(channel_text.len() as u16),
                            Constraint::Length(1),
                            Constraint::Min(0),
                        ]
                        .as_ref(),
                    )
                    .split(chunks[2]);

                let channel_indictator = Paragraph::new(channel_text);

                f.render_widget(channel_indictator, input_bar_layout[0]);
                f.render_widget(input_paragraph, input_bar_layout[2]);

                f.set_cursor(
                    input_bar_layout[2].x + app.input_text.len() as u16,
                    input_bar_layout[2].y,
                );
            } else {
                f.render_widget(input_paragraph, chunks[2]);

                f.set_cursor(chunks[2].x + app.input_text.len() as u16, chunks[2].y);
            }

            if app.channel_switcher_open {
                if !app.channel_switcher_channels.is_empty()
                    && app.channel_switcher_state.selected().is_none()
                {
                    app.channel_switcher_state.select(Some(0));
                }

                let popup = List::new(
                    app.channel_switcher_channels
                        .iter()
                        .map(|c| ListItem::new(c.render()))
                        .collect::<Vec<ListItem>>(),
                )
                .block(
                    Block::default()
                        .title("Channel switcher")
                        .borders(Borders::ALL),
                )
                .highlight_style(Style::default().add_modifier(Modifier::ITALIC))
                .highlight_symbol(">>");
                let area = util::centered_rect(60, 50, size);
                f.render_widget(Clear, area);
                f.render_stateful_widget(popup, area, &mut app.channel_switcher_state);
            }
        })?;

        tokio::select! {
            input = input.next() => {
                if let Some(maybe_event) = input {
                    let event = maybe_event?;
                    if input::should_quit(&event) {
                        break;
                    }

                    if let input::Event::Key(key) = event {
                        match key {
                            input::KeyEvent { code: input::KeyCode::Backspace, .. } => {
                                app.input_text.pop();
                            },
                            input::KeyEvent { code: input::KeyCode::Up, ..} if app.channel_switcher_open => {
                                let i = match app.channel_switcher_state.selected() {
                                    Some(i) => {
                                        if i == 0 {
                                            app.channel_switcher_channels.len() - 1
                                        } else {
                                            i - 1
                                        }
                                    }
                                    None => 0,
                                };
                                app.channel_switcher_state.select(Some(i));
                            }
                            input::KeyEvent { code: input::KeyCode::Down, ..} if app.channel_switcher_open => {
                                let i = match app.channel_switcher_state.selected() {
                                    Some(i) => {
                                        if i >= app.channel_switcher_channels.len() - 1 {
                                            0
                                        } else {
                                            i + 1
                                        }
                                    }
                                    None => 0,
                                };
                                app.channel_switcher_state.select(Some(i));
                            }
                            input::KeyEvent { code: input::KeyCode::Enter, .. } => {
                                if app.channel_switcher_open {
                                    app.current_channel = app.channel_switcher_channels[app.channel_switcher_state.selected().unwrap()].clone();
                                    app.channel_switcher_open = false;
                                    app.current_mode = InputMode::Chat;
                                } else if InputMode::Chat == app.current_mode {
                                    let text = app.input_text.clone();
                                    app.input_text.clear();

                                    let _ = command_sender.send(Command::Message(app.current_channel.clone(), text));
                                } else if InputMode::Command == app.current_mode {
                                    let command = command::Command::from_input(&app.input_text);
                                    app.input_text.clear();

                                    if let Some(cmd) = command {
                                        let cmd = cmd.into();
                                        let _ = command_sender.send(cmd);
                                    } else {
                                        app.status_text = String::from("Error in command syntax");
                                    }
                                }
                            }
                            input::KeyEvent { code: input::KeyCode::Esc, .. } => {
                                app.input_text.clear();
                                if InputMode::Command == app.current_mode {
                                    app.current_mode = InputMode::Chat;
                                } else {
                                    app.current_mode = InputMode::Command;
                                    app.input_text.push('/');
                                }
                            },
                            input::KeyEvent { code: input::KeyCode::Tab, .. } => {
                                app.channel_switcher_open = !app.channel_switcher_open;
                                let (tx, rx) = oneshot::channel();
                                let query = StateQuery::Channels(tx);
                                let _ = state_query_sender.send(query);
                                let channels = rx.await.unwrap();
                                app.channel_switcher_channels = channels;
                            },
                            input::KeyEvent { code: input::KeyCode::Char('k'), modifiers } if modifiers.contains(input::KeyModifiers::CONTROL) => {
                                app.channel_switcher_open = !app.channel_switcher_open;
                                let (tx, rx) = oneshot::channel();
                                let query = StateQuery::Channels(tx);
                                let _ = state_query_sender.send(query);
                                let channels = rx.await.unwrap();
                                app.channel_switcher_channels = channels;
                            },
                            input::KeyEvent { code: input::KeyCode::Char(c), .. } => app.input_text.push(c),
                            _ => {},
                        }
                    }
                } else {
                    break;
                }
            },

            ui_update = ui_update_receiver.recv() => {
                if let Some(update) = ui_update {
                    match update {
                        UiUpdate::Message(msg) => {
                            let rendered = msg.render();
                            app.messages.lines.splice(0..0, rendered);

                            if app.current_mode != InputMode::Scroll {
                                app.scroll_y = 0;
                            }
                        },
                        _ => {},
                    }
                }
            },
        };
    }

    Ok(())
}
