use std::{collections::HashMap, sync::Arc};

use bimap::BiHashMap;
use nadylib::{
    client_socket::SocketSendHandle,
    models::{Channel, Message},
    packets::{ClientLookupPacket, LoginSelectPacket, MsgPrivatePacket},
    AOSocket, ReceivedPacket,
};
use tokio::sync::{mpsc::UnboundedSender, Notify};

pub struct ChatState {
    pub channels: Vec<Channel>,
    pub messages: Vec<Message>,
    pub pending_invites: Vec<Channel>,
    pub user_lookup: BiHashMap<u32, String>,
    pub current_user: u32,
    pub pending_lookups: HashMap<String, Arc<Notify>>,
    pub sender: SocketSendHandle,
}

pub enum StateUpdate {
    Message(Message),
    Channel(Channel),
    Invite(Channel),
    User(u32, String),
    CurrentUser(u32),
    UserLookupFinished(String, u32, bool),
}

impl ChatState {
    pub fn with_sender(sender: SocketSendHandle) -> Self {
        Self {
            channels: Vec::new(),
            messages: Vec::new(),
            pending_invites: Vec::new(),
            user_lookup: BiHashMap::new(),
            current_user: 0,
            pending_lookups: HashMap::new(),
            sender,
        }
    }

    pub async fn lookup_user(&mut self, user: &str) {
        if self.user_lookup.get_by_right(user).is_none() {
            let notify = if let Some(notifier) = self.pending_lookups.get(user) {
                notifier.clone()
            } else {
                let notify = Arc::new(Notify::new());
                self.pending_lookups
                    .insert(user.to_string(), notify.clone());
                let pack = ClientLookupPacket {
                    character_name: user.to_string(),
                };
                let _ = self.sender.send(pack).await;
                notify
            };

            notify.notified().await;
        }
    }

    pub async fn send_tell(&mut self, user: &str, text: String) {
        self.lookup_user(user).await;

        if let Some(id) = self.user_lookup.get_by_right(user) {
            let message = Message {
                sender: Some(self.current_user),
                channel: Channel::Tell(*id),
                text,
                send_tag: String::from("\u{0}"),
            };
            let packet = MsgPrivatePacket { message };
            let _ = self.sender.send(packet).await;
        }
    }

    pub fn handle_update(&mut self, update: StateUpdate) {
        match update {
            StateUpdate::Message(msg) => self.messages.push(msg),
            StateUpdate::Channel(channel) => self.channels.push(channel),
            StateUpdate::Invite(channel) => self.pending_invites.push(channel),
            StateUpdate::User(id, name) => {
                self.user_lookup.insert(id, name);
            }
            StateUpdate::CurrentUser(id) => self.current_user = id,
            StateUpdate::UserLookupFinished(name, id, exists) => {
                if exists {
                    self.user_lookup.insert(id, name.clone());
                    self.channels.push(Channel::Tell(id));
                }

                if let Some(notify) = self.pending_lookups.remove(&name) {
                    notify.notify_waiters();
                }
            }
        }
    }

    pub fn render_channel(&self, channel: &Channel) -> String {
        match channel {
            Channel::Group(group) => {
                let name = group.name.as_ref().map_or_else(
                    || {
                        self.channels
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
                            .as_str()
                    },
                    |n| n.as_str(),
                );
                format!("*{}", name)
            }
            Channel::PrivateChannel(user) => {
                format!("#{}", self.user_lookup.get_by_left(user).unwrap())
            }
            Channel::Tell(user) => {
                format!("@{}", self.user_lookup.get_by_left(user).unwrap())
            }
            Channel::Vicinity => String::from("."),
        }
    }

    pub fn render_message(&self, message: &Message) -> String {
        let channel = self.render_channel(&message.channel);

        if let Some(sender) = message.sender {
            let name = self.user_lookup.get_by_left(&sender).unwrap();

            format!("[{}] {}: {}", channel, name, message.text)
        } else {
            format!("[{}] {}", channel, message.text)
        }
    }
}

pub async fn chat_task(
    mut sock: AOSocket,
    sender: UnboundedSender<StateUpdate>,
    username: String,
    char_name: String,
    password: String,
) -> nadylib::Result<()> {
    while let Ok(packet) = sock.read_packet().await {
        match packet {
            ReceivedPacket::LoginSeed(s) => {
                sock.login(&username, &password, &s.login_seed).await?;
            }
            ReceivedPacket::LoginCharlist(c) => {
                let character = c.characters.iter().find(|i| i.name == char_name).unwrap();
                let pack = LoginSelectPacket {
                    character_id: character.id,
                };
                let _ = sender.send(StateUpdate::CurrentUser(character.id));
                sock.send(pack).await?;
            }
            ReceivedPacket::LoginError(e) => panic!("{}", e.message),
            ReceivedPacket::ClientName(c) => {
                let _ = sender.send(StateUpdate::User(c.character_id, c.character_name));
            }
            ReceivedPacket::MsgVicinity(m) => {
                let _ = sender.send(StateUpdate::Message(m.message));
            }
            ReceivedPacket::MsgVicinitya(m) => {
                let _ = sender.send(StateUpdate::Message(m.message));
            }
            ReceivedPacket::GroupAnnounce(g) => {
                let _ = sender.send(StateUpdate::Channel(g.channel));
            }
            ReceivedPacket::GroupMessage(m) => {
                let _ = sender.send(StateUpdate::Message(m.message));
            }
            ReceivedPacket::MsgPrivate(m) => {
                let _ = sender.send(StateUpdate::Message(m.message));
            }
            ReceivedPacket::PrivgrpInvite(p) => {
                let _ = sender.send(StateUpdate::Invite(p.channel));
            }
            ReceivedPacket::PrivgrpMessage(m) => {
                let _ = sender.send(StateUpdate::Message(m.message));
            }
            ReceivedPacket::ClientLookup(c) => {
                let _ = sender.send(StateUpdate::UserLookupFinished(
                    c.character_name,
                    c.character_id,
                    c.exists,
                ));
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
    }

    Ok(())
}
