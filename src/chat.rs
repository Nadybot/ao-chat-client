use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, RwLock,
    },
};

use bimap::BiHashMap;
use nadylib::{
    client_socket::SocketSendHandle,
    models::{Channel, Message},
    packets::{
        ClientLookupPacket, GroupMessagePacket, LoginSelectPacket, MsgPrivatePacket,
        OutPrivgrpInvitePacket, OutPrivgrpKickPacket, PrivgrpMessagePacket, PrivgrpPartPacket,
    },
    AOSocket, ReceivedPacket,
};
use tokio::sync::{
    mpsc::{UnboundedReceiver, UnboundedSender},
    oneshot::Sender,
    Notify,
};
use tui::text::{Span, Spans};

use crate::command;

pub enum StateQuery {
    Channels(Sender<Vec<ResolvedChannel>>),
}

pub enum Command {
    Invite(String),
    Kick(String),
    Leave(String),
    Tell(String, String),
    Message(ResolvedChannel, String),
}

impl From<command::Command> for Command {
    fn from(cmd: command::Command) -> Self {
        match cmd {
            command::Command::Invite(user) => Self::Invite(user),
            command::Command::Kick(user) => Self::Kick(user),
            command::Command::Leave(user) => Self::Leave(user),
            command::Command::Tell(user, message) => Self::Tell(user, message),
        }
    }
}

pub enum UiUpdate {
    Message(ResolvedMessage),
    Invite(ResolvedChannel),
    Kick(ResolvedChannel),
    Leave(String, ResolvedChannel),
}

#[derive(Clone)]
pub enum ChannelType {
    Group,
    PrivateChannel,
    Tell,
    Vicinity,
}

#[derive(Clone)]
pub struct ResolvedMessage {
    pub sender: Option<String>,
    pub channel: ResolvedChannel,
    pub text: String,
}

impl ResolvedMessage {
    fn new(state: &ChatState, message: &Message) -> Self {
        let sender = message.sender.map(|id| {
            state
                .user_lookup
                .read()
                .unwrap()
                .get_by_left(&id)
                .unwrap()
                .to_owned()
        });
        let channel = ResolvedChannel::new(state, &message.channel);

        Self {
            sender,
            channel,
            text: message.text.clone(),
        }
    }

    pub fn render<'a>(&self) -> Vec<Spans<'a>> {
        let channel = self.channel.render();

        let text = if let Some(sender) = &self.sender {
            format!("[{}] {}: {}", channel, sender, self.text)
        } else {
            format!("[{}] {}", channel, self.text)
        };
        let lines = text.split("\n");
        let spans: Vec<Spans> = lines
            .map(|line| Spans::from(Span::raw(line.to_string())))
            .collect();

        spans
    }
}

#[derive(Clone)]
pub struct ResolvedChannel {
    pub id: u32,
    pub name: String,
    pub r#type: ChannelType,
}

impl ResolvedChannel {
    fn new(state: &ChatState, channel: &Channel) -> Self {
        let (name, id, r#type) = match channel {
            Channel::Group(group) => (
                group.name.clone().unwrap_or_else(|| {
                    state
                        .channels
                        .read()
                        .unwrap()
                        .iter()
                        .find_map(|c| {
                            if let Channel::Group(g) = c {
                                if group.id == g.id {
                                    Some(g.name.as_ref().unwrap())
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        })
                        .unwrap()
                        .to_string()
                }),
                group.id,
                ChannelType::Group,
            ),
            Channel::PrivateChannel(id) => (
                state
                    .user_lookup
                    .read()
                    .unwrap()
                    .get_by_left(&id)
                    .unwrap()
                    .to_string(),
                *id,
                ChannelType::PrivateChannel,
            ),
            Channel::Tell(id) => (
                state
                    .user_lookup
                    .read()
                    .unwrap()
                    .get_by_left(&id)
                    .unwrap()
                    .to_string(),
                *id,
                ChannelType::Tell,
            ),
            Channel::Vicinity => (String::from("Vicinity"), 0, ChannelType::Vicinity),
        };

        Self { name, id, r#type }
    }

    pub fn render(&self) -> String {
        match self.r#type {
            ChannelType::Group => {
                format!("*{}", self.name)
            }
            ChannelType::PrivateChannel => {
                format!("#{}", self.name)
            }
            ChannelType::Tell => {
                format!("@{}", self.name)
            }
            ChannelType::Vicinity => String::from("."),
        }
    }
}

pub struct ChatState {
    pub channels: RwLock<Vec<Channel>>,
    pub past_invites: RwLock<Vec<Channel>>,
    pub user_lookup: RwLock<BiHashMap<u32, String>>,
    pub current_user: AtomicU32,
    pub pending_lookups: RwLock<HashMap<String, Arc<Notify>>>,
    pub ui_update_sender: UnboundedSender<UiUpdate>,
    pub sender: SocketSendHandle,
}

impl ChatState {
    pub fn new(sender: SocketSendHandle, ui_update_sender: UnboundedSender<UiUpdate>) -> Self {
        Self {
            channels: RwLock::new(Vec::new()),
            past_invites: RwLock::new(Vec::new()),
            user_lookup: RwLock::new(BiHashMap::new()),
            current_user: AtomicU32::new(0),
            pending_lookups: RwLock::new(HashMap::new()),
            sender,
            ui_update_sender,
        }
    }

    pub async fn lookup_user(&self, user: String) -> Option<u32> {
        let maybe_user = self
            .user_lookup
            .read()
            .unwrap()
            .get_by_right(&user)
            .map(|i| *i);
        if let Some(id) = maybe_user {
            Some(id)
        } else {
            let maybe_notify = self
                .pending_lookups
                .read()
                .unwrap()
                .get(&user)
                .map(|n| n.clone());
            let notify = if let Some(notifier) = maybe_notify {
                notifier
            } else {
                let notify = Arc::new(Notify::new());
                self.pending_lookups
                    .write()
                    .unwrap()
                    .insert(user.clone(), notify.clone());
                let pack = ClientLookupPacket {
                    character_name: user.clone(),
                };
                let _ = self.sender.send(pack).await;
                notify
            };

            notify.notified().await;

            self.user_lookup
                .read()
                .unwrap()
                .get_by_right(&user)
                .map(|v| *v)
        }
    }

    pub async fn invite(&self, user: String) {
        let user_id = self.lookup_user(user).await;

        if let Some(id) = user_id {
            let packet = OutPrivgrpInvitePacket { character_id: id };
            let _ = self.sender.send(packet).await;
        }
    }

    pub async fn kick(&self, user: String) {
        let user_id = self.lookup_user(user).await;

        if let Some(id) = user_id {
            let packet = OutPrivgrpKickPacket { character_id: id };
            let _ = self.sender.send(packet).await;
        }
    }

    pub async fn leave(&self, user: String) {
        let user_id = self.lookup_user(user).await;

        if let Some(id) = user_id {
            let packet = PrivgrpPartPacket {
                channel: Channel::PrivateChannel(id),
            };
            let _ = self.sender.send(packet).await;
        }
    }

    pub async fn send_tell(&self, user: String, text: String) {
        let user_id = self.lookup_user(user).await;

        if let Some(id) = user_id {
            let message = Message {
                sender: Some(self.current_user.load(Ordering::Relaxed)),
                channel: Channel::Tell(id),
                text,
                send_tag: String::from("\u{0}"),
            };
            let resolved = ResolvedMessage::new(self, &message);
            let _ = self.ui_update_sender.send(UiUpdate::Message(resolved));
            if !self.channels.read().unwrap().iter().any(|channel| {
                if let Channel::Tell(user) = channel {
                    *user == id
                } else {
                    false
                }
            }) {
                self.channels.write().unwrap().push(message.channel.clone());
            }
            let packet = MsgPrivatePacket { message };
            let _ = self.sender.send(packet).await;
        }
    }

    pub async fn send_message(&self, resolved_channel: ResolvedChannel, text: String) {
        let channel = match resolved_channel.r#type {
            ChannelType::Vicinity => Channel::Vicinity,
            ChannelType::Tell => Channel::Tell(resolved_channel.id),
            ChannelType::PrivateChannel => Channel::PrivateChannel(resolved_channel.id),
            ChannelType::Group => self
                .channels
                .read()
                .unwrap()
                .iter()
                .find(|c| {
                    if let Channel::Group(g) = c {
                        resolved_channel.id == g.id
                    } else {
                        false
                    }
                })
                .unwrap()
                .clone(),
        };

        let message = Message {
            sender: Some(self.current_user.load(Ordering::Relaxed)),
            channel,
            text,
            send_tag: String::from("\u{0}"),
        };

        match message.channel {
            Channel::Group(_) => self
                .sender
                .send(GroupMessagePacket { message })
                .await
                .unwrap(),
            Channel::Tell(_) => {
                let resolved = ResolvedMessage::new(self, &message);
                let _ = self.ui_update_sender.send(UiUpdate::Message(resolved));
                self.sender
                    .send(MsgPrivatePacket { message })
                    .await
                    .unwrap()
            }
            Channel::PrivateChannel(_) => self
                .sender
                .send(PrivgrpMessagePacket { message })
                .await
                .unwrap(),
            Channel::Vicinity => panic!("impossible"),
        }
    }
}

pub async fn chat_task(
    mut sock: AOSocket,
    mut state_query_receiver: UnboundedReceiver<StateQuery>,
    mut command_receiver: UnboundedReceiver<Command>,
    ui_update_sender: UnboundedSender<UiUpdate>,
    username: String,
    char_name: String,
    password: String,
) -> nadylib::Result<()> {
    let chat_state = Arc::new(ChatState::new(sock.get_sender(), ui_update_sender.clone()));

    loop {
        tokio::select! {
            packet = sock.read_packet() => {
                if let Ok(packet) = packet {
                    match packet {
                        ReceivedPacket::LoginSeed(s) => {
                            sock.login(&username, &password, &s.login_seed).await?;
                        }
                        ReceivedPacket::LoginCharlist(c) => {
                            let character = c.characters.iter().find(|i| i.name == char_name).unwrap();
                            let pack = LoginSelectPacket {
                                character_id: character.id,
                            };
                            chat_state.current_user.store(character.id, Ordering::Relaxed);
                            sock.send(pack).await?;
                        }
                        ReceivedPacket::LoginError(e) => panic!("{}", e.message),
                        ReceivedPacket::ClientName(c) => {
                            chat_state
                                .user_lookup
                                .write()
                                .unwrap()
                                .insert(c.character_id, c.character_name);
                        }
                        ReceivedPacket::MsgVicinity(m) => {
                            let resolved = ResolvedMessage::new(&chat_state, &m.message);
                            let _ = ui_update_sender.send(UiUpdate::Message(resolved));
                        }
                        ReceivedPacket::MsgVicinitya(m) => {
                            let resolved = ResolvedMessage::new(&chat_state, &m.message);
                            let _ = ui_update_sender.send(UiUpdate::Message(resolved));
                        }
                        ReceivedPacket::GroupAnnounce(g) => {
                            chat_state.channels.write().unwrap().push(g.channel);
                        }
                        ReceivedPacket::GroupMessage(m) => {
                            let resolved = ResolvedMessage::new(&chat_state, &m.message);
                            let _ = ui_update_sender.send(UiUpdate::Message(resolved));
                        }
                        ReceivedPacket::MsgPrivate(m) => {
                            let resolved = ResolvedMessage::new(&chat_state, &m.message);
                            let _ = ui_update_sender.send(UiUpdate::Message(resolved));
                            if !chat_state.channels.read().unwrap().iter().any(|channel| {
                                if let Channel::Tell(user) = channel {
                                    if let Channel::Tell(other_user) = m.message.channel {
                                        *user == other_user
                                    } else {
                                        false
                                    }
                                } else {
                                    false
                                }
                            }) {
                                chat_state.channels.write().unwrap().push(m.message.channel);
                            }
                        }
                        ReceivedPacket::PrivgrpInvite(p) => {
                            chat_state.past_invites.write().unwrap().push(p.channel);
                        }
                        ReceivedPacket::PrivgrpMessage(m) => {
                            let resolved = ResolvedMessage::new(&chat_state, &m.message);
                            let _ = ui_update_sender.send(UiUpdate::Message(resolved));
                        }
                        ReceivedPacket::ClientLookup(c) => {
                            if c.exists {
                                chat_state
                                    .user_lookup
                                    .write()
                                    .unwrap()
                                    .insert(c.character_id, c.character_name.clone());
                                chat_state.channels.write().unwrap().push(Channel::Tell(c.character_id));
                            }

                            if let Some(notify) = chat_state.pending_lookups.write().unwrap().remove(&c.character_name) {
                                notify.notify_waiters();
                            }
                        }
                        ReceivedPacket::LoginOk
                        | ReceivedPacket::BuddyRemove(_)
                        | ReceivedPacket::BuddyStatus(_)
                        | ReceivedPacket::ChatNotice(_)
                        | ReceivedPacket::PrivgrpClijoin(_)
                        | ReceivedPacket::PrivgrpClipart(_)
                        | ReceivedPacket::PrivgrpKick(_)
                        | ReceivedPacket::MsgSystem(_)
                        | ReceivedPacket::Ping(_) => {}
                    }
                } else {
                    break;
                }
            },
            command = command_receiver.recv() => {
                if let Some(cmd) = command {
                    match cmd {
                        Command::Invite(user_name) => {
                            let chat_state = chat_state.clone();
                            tokio::spawn(async move { chat_state.invite(user_name).await });
                        }
                        Command::Kick(user_name) => {
                            let chat_state = chat_state.clone();
                            tokio::spawn(async move { chat_state.kick(user_name).await });
                        }
                        Command::Leave(user_name) => {
                            let chat_state = chat_state.clone();
                            tokio::spawn(async move { chat_state.leave(user_name).await });
                        }
                        Command::Tell(user_name, text) => {
                            let chat_state = chat_state.clone();
                            tokio::spawn(async move { chat_state.send_tell(user_name, text).await });
                        }
                        Command::Message(channel, text) => {
                            let chat_state = chat_state.clone();
                            tokio::spawn(async move { chat_state.send_message(channel, text).await });
                        }
                    }
                }
            },
            query = state_query_receiver.recv() => {
                if let Some(query) = query {
                    match query {
                        StateQuery::Channels(sender) => {
                            let channels: Vec<ResolvedChannel> = chat_state.channels.read().unwrap().iter().map(|channel| ResolvedChannel::new(&chat_state, channel)).collect();
                            let _ = sender.send(channels);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
