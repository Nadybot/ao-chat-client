pub enum Command {
    Invite(String),
    Kick(String),
    Leave(String),
    Tell(String, String),
}

impl Command {
    pub fn from_input(input: &str) -> Option<Self> {
        let command = input.strip_prefix('/').unwrap_or(input);
        let mut params = command.split_ascii_whitespace();
        let name = params.next()?;
        let user = params.next()?;

        let mut rest = params.fold(String::new(), |a, b| a + b + " ");
        rest = rest.trim().to_string();

        match name {
            "invite" => Some(Self::Invite(user.to_string())),
            "kick" => Some(Self::Kick(user.to_string())),
            "leave" => Some(Self::Leave(user.to_string())),
            "tell" => Some(Self::Tell(user.to_string(), rest)),
            _ => None,
        }
    }
}
