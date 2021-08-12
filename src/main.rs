#![deny(clippy::pedantic)]
#![allow(
    clippy::let_underscore_drop,
    clippy::cast_possible_truncation,
    clippy::too_many_lines,
    clippy::module_name_repetitions,
)]

use chat::ChatState;
use directories::ProjectDirs;
use futures_util::StreamExt;
use nadylib::{
    models::{Channel, Message},
    packets::{GroupMessagePacket, MsgPrivatePacket, PrivgrpMessagePacket},
    AOSocket, SocketConfig,
};
use tokio::sync::mpsc::unbounded_channel;
use tui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

use std::{
    fs::{create_dir_all, write},
    io,
};

mod chat;
mod command;
mod config;
mod input;
mod term;
mod util;

const ORANGE: Color = Color::Rgb(232, 149, 6);

enum InputMode {
    Command,
    Chat,
}

struct App {
    current_mode: InputMode,
    channel_switcher_open: bool,
    channel_switcher_state: ListState,
    current_channel: Channel,
    input_text: String,
    status_text: String,
    chat_state: ChatState,
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
    let sender = sock.get_sender();
    let mut app = App {
        current_mode: InputMode::Command,
        channel_switcher_open: true,
        channel_switcher_state: ListState::default(),
        current_channel: Channel::Vicinity,
        input_text: String::new(),
        status_text: String::from("Initialized"),
        chat_state: ChatState::with_sender(sender),
    };

    let (state_sender, mut receiver) = unbounded_channel();
    tokio::spawn(chat::chat_task(
        sock,
        state_sender,
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

            // Chat block is empty
            let chat_block = List::new(
                app.chat_state
                    .messages
                    .iter()
                    .map(|m| ListItem::new(app.chat_state.render_message(m)))
                    .collect::<Vec<ListItem>>(),
            )
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
                let channel_text =
                    format!("[{}]", app.chat_state.render_channel(&app.current_channel));

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
                if !app.chat_state.channels.is_empty()
                    && app.channel_switcher_state.selected().is_none()
                {
                    app.channel_switcher_state.select(Some(0));
                }

                let popup = List::new(
                    app.chat_state
                        .channels
                        .iter()
                        .map(|c| ListItem::new(app.chat_state.render_channel(c)))
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
                                            app.chat_state.channels.len() - 1
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
                                        if i >= app.chat_state.channels.len() - 1 {
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
                                    app.current_channel = app.chat_state.channels[app.channel_switcher_state.selected().unwrap()].clone();
                                    app.channel_switcher_open = false;
                                } else if let InputMode::Chat = app.current_mode {
                                    let text = app.input_text.clone();
                                    app.input_text.clear();

                                    let message = Message {
                                        sender: Some(app.chat_state.current_user),
                                        channel: app.current_channel.clone(),
                                        text,
                                        send_tag: String::from("\u{0}"),
                                    };

                                    match app.current_channel {
                                        Channel::Group(_) => { let _ = app.chat_state.sender.send(GroupMessagePacket { message }).await; },
                                        Channel::Tell(_) => { let _ = app.chat_state.sender.send(MsgPrivatePacket { message }).await; },
                                        Channel::PrivateChannel(_) => { let _ = app.chat_state.sender.send(PrivgrpMessagePacket { message }).await; },
                                        Channel::Vicinity => panic!("Shouldn't happen"),
                                    };
                                } else if let InputMode::Command = app.current_mode {
                                    let command = command::Command::from_input(&app.input_text);
                                    app.input_text.clear();

                                    if let Some(cmd) = command {
                                        match cmd {
                                            command::Command::Kick(user) => {},
                                            command::Command::Invite(user) => {},
                                            command::Command::Leave(user) => {},
                                            command::Command::Tell(user, maybe_text) => {
                                                if let Some(text) = maybe_text {
                                                    app.chat_state.send_tell(&user, text).await;
                                                } else {
                                                    app.chat_state.lookup_user(&user).await;
                                                }
                                            }
                                        }
                                    } else {
                                        app.status_text = String::from("Error in command syntax");
                                    }
                                }
                            }
                            input::KeyEvent { code: input::KeyCode::Esc, .. } => {
                                app.input_text.clear();
                                if let InputMode::Command = app.current_mode {
                                    app.current_mode = InputMode::Chat;
                                } else {
                                    app.current_mode = InputMode::Command;
                                    app.input_text.push('/');
                                }
                            },
                            input::KeyEvent { code: input::KeyCode::Tab, .. } => app.channel_switcher_open = !app.channel_switcher_open,
                            input::KeyEvent { code: input::KeyCode::Char('k'), modifiers } if modifiers.contains(input::KeyModifiers::CONTROL) => app.channel_switcher_open = !app.channel_switcher_open,
                            input::KeyEvent { code: input::KeyCode::Char(c), .. } => app.input_text.push(c),
                            _ => {},
                        }
                    }
                } else {
                    break;
                }
            }

            state_update = receiver.recv() => {
                if let Some(update) = state_update {
                    app.chat_state.handle_update(update);
                }
            }
        };
    }

    Ok(())
}
