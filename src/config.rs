use std::{fs::read_to_string, path::Path};

pub struct Config {
    pub character_name: String,
    pub user_name: String,
    pub password: String,
}

pub fn load(path: &Path) -> Option<Config> {
    let contents = read_to_string(path).ok()?;
    let character_name = contents
        .lines()
        .find(|line| line.starts_with("CHARNAME"))?
        .split('=')
        .nth(1)?;
    let user_name = contents
        .lines()
        .find(|line| line.starts_with("USERNAME"))?
        .split('=')
        .nth(1)?;
    let password = contents
        .lines()
        .find(|line| line.starts_with("PASSWORD"))?
        .split('=')
        .nth(1)?;

    if character_name.is_empty() || user_name.is_empty() || password.is_empty() {
        return None;
    }

    Some(Config {
        user_name: user_name.to_string(),
        character_name: character_name.to_string(),
        password: password.to_string(),
    })
}
