use std::{
    process::{Command, Stdio},
    thread,
};

use system_tray::menu::{ToggleState, ToggleType};

use crate::{config::PowerConfig, tray::TrayMenuEntry};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Shutdown,
    Restart,
    Suspend,
    Hibernate,
}

impl Action {
    pub fn from_id(id: i32) -> Option<Self> {
        match id {
            0 => Some(Self::Shutdown),
            1 => Some(Self::Restart),
            2 => Some(Self::Suspend),
            3 => Some(Self::Hibernate),
            _ => None,
        }
    }

    pub fn command(self, config: &PowerConfig) -> &str {
        match self {
            Self::Shutdown => &config.shutdown,
            Self::Restart => &config.restart,
            Self::Suspend => &config.suspend,
            Self::Hibernate => &config.hibernate,
        }
    }
}

pub fn menu_entries(config: &PowerConfig) -> [TrayMenuEntry; 4] {
    let labels = ["Shutdown", "Restart", "Suspend", "Hibernate"];
    std::array::from_fn(|id| {
        let action = Action::from_id(id as i32).expect("power action index");
        TrayMenuEntry {
            id: id as i32,
            label: labels[id].into(),
            enabled: !action.command(config).trim().is_empty(),
            separator: false,
            submenu: Vec::new(),
            toggle_type: ToggleType::CannotBeToggled,
            toggle_state: ToggleState::Off,
        }
    })
}

pub fn execute(action: Action, command: String) {
    if command.trim().is_empty() {
        return;
    }
    if let Err(error) = thread::Builder::new()
        .name(format!("power-{action:?}"))
        .spawn(move || {
            match Command::new("sh")
                .arg("-c")
                .arg(&command)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
            {
                Ok(status) if status.success() => {}
                Ok(status) => log::warn!("{action:?} command exited with {status}"),
                Err(error) => log::warn!("failed to run {action:?} command: {error}"),
            }
        })
    {
        log::warn!("failed to start {action:?} command worker: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_disables_actions_without_commands_and_keeps_order() {
        let config = PowerConfig {
            restart: "   ".into(),
            ..PowerConfig::default()
        };
        let entries = menu_entries(&config);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.label.as_str())
                .collect::<Vec<_>>(),
            ["Shutdown", "Restart", "Suspend", "Hibernate"]
        );
        assert!(!entries[1].enabled);
        assert!(entries[0].enabled && entries[2].enabled && entries[3].enabled);
        assert_eq!(Action::from_id(4), None);
    }

    #[test]
    fn each_menu_id_uses_its_configured_command() {
        let config = PowerConfig {
            shutdown: "first".into(),
            restart: "second".into(),
            suspend: "third".into(),
            hibernate: "fourth".into(),
            ..PowerConfig::default()
        };
        let commands: Vec<_> = (0..4)
            .map(|id| Action::from_id(id).unwrap().command(&config))
            .collect();
        assert_eq!(commands, ["first", "second", "third", "fourth"]);
    }
}
