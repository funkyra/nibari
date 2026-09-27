use std::{collections::HashMap, process::Stdio, time::Duration};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::mpsc,
    time::timeout,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub description: String,
    pub available: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Codec {
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub name: String,
    path: String,
    address: String,
    pub profiles: Vec<Profile>,
    pub active_profile: String,
    pub codecs: Vec<Codec>,
    pub active_codec: Option<String>,
    pub codec_error: Option<String>,
}

impl Card {
    pub fn matches(&self, path: &str, address: &str) -> bool {
        if !self.path.is_empty() {
            self.path == path
        } else {
            !self.address.is_empty() && self.address.eq_ignore_ascii_case(address)
        }
    }

    pub fn message_path(&self) -> String {
        format!("/card/{}/bluez", self.name)
    }
}

#[derive(Deserialize)]
struct RawCard {
    name: String,
    #[serde(default)]
    properties: HashMap<String, Value>,
    #[serde(default)]
    profiles: HashMap<String, Value>,
    #[serde(default)]
    active_profile: Value,
}

pub fn parse_cards(json: &str) -> Result<Vec<Card>> {
    let raw: Vec<RawCard> = serde_json::from_str(json)?;
    Ok(raw
        .into_iter()
        .filter_map(|c| {
            let prop = |key: &str| c.properties.get(key).and_then(Value::as_str).unwrap_or("");
            let path = prop("api.bluez5.path").to_owned();
            if path.is_empty() && !c.name.starts_with("bluez_card.") {
                return None;
            }
            let address = if prop("device.string").is_empty() {
                c.name
                    .strip_prefix("bluez_card.")
                    .unwrap_or("")
                    .replace('_', ":")
            } else {
                prop("device.string").to_owned()
            };
            let mut profiles: Vec<_> = c
                .profiles
                .into_iter()
                .map(|(name, p)| Profile {
                    description: p["description"].as_str().unwrap_or(&name).to_owned(),
                    available: p["available"] != false && p["available"] != "no",
                    name,
                })
                .collect();
            profiles.sort_unstable_by(|a, b| a.name.cmp(&b.name));
            let active_profile = c
                .active_profile
                .as_str()
                .or_else(|| c.active_profile.get("name").and_then(Value::as_str))
                .unwrap_or("")
                .to_owned();
            Some(Card {
                name: c.name,
                path,
                address,
                profiles,
                active_profile,
                codecs: Vec::new(),
                active_codec: None,
                codec_error: None,
            })
        })
        .collect())
}

pub fn parse_codecs(json: &str) -> Result<Vec<Codec>> {
    Ok(serde_json::from_str(json)?)
}

pub async fn pactl(args: &[&str]) -> Result<String> {
    let output = timeout(
        Duration::from_secs(4),
        Command::new("pactl")
            .args(args)
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .context("PipeWire command timed out")?
    .context("cannot run pactl")?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8(output.stdout)?)
}

pub async fn cards() -> Result<Vec<Card>> {
    parse_cards(&pactl(&["--format=json", "list", "cards"]).await?)
}

pub async fn load_codecs(card: &mut Card) {
    let path = card.message_path();
    let result = async {
        card.codecs = parse_codecs(&pactl(&["send-message", &path, "list-codecs"]).await?)?;
        card.active_codec =
            serde_json::from_str(&pactl(&["send-message", &path, "get-codec"]).await?)?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    if let Err(error) = result {
        card.codec_error = Some(format!("Codec selection unavailable: {error}"));
    }
}

// The child exists only while the popup is open. Dropping this future kills it.
pub async fn watch(changed: mpsc::Sender<()>) {
    loop {
        let result = async {
            let mut child = Command::new("pactl")
                .arg("subscribe")
                .env("LC_ALL", "C")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()?;
            let mut lines = BufReader::new(child.stdout.take().context("pactl stdout")?).lines();
            while let Some(line) = lines.next_line().await? {
                if line.contains(" on card #")
                    && changed
                        .try_send(())
                        .is_err_and(|e| matches!(e, mpsc::error::TrySendError::Closed(_)))
                {
                    return Ok::<_, anyhow::Error>(());
                }
            }
            child.wait().await?;
            Ok(())
        }
        .await;
        if changed.is_closed() {
            return;
        }
        if let Err(error) = result {
            log::debug!("Bluetooth audio subscription: {error}");
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
        let _ = changed.try_send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cards_match_bluez_paths_and_keep_unavailable_profiles_disabled() {
        let cards = parse_cards(r#"[
          {"name":"alsa_card.usb","properties":{"device.string":"0"},"profiles":{}},
          {"name":"bluez_card.AA_BB_CC_DD_EE_FF","properties":{"api.bluez5.path":"/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF"},
           "active_profile":"a2dp-sink","profiles":{
             "a2dp-sink":{"description":"High Fidelity Playback (A2DP Sink)","available":"yes"},
             "headset-head-unit":{"description":"Headset (HFP)","available":"no"}}}
        ]"#).unwrap();
        assert_eq!(cards.len(), 1);
        let card = &cards[0];
        assert!(card.matches("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF", "AA:BB:CC:DD:EE:FF"));
        assert!(!card.matches("/org/bluez/hci1/dev_11_22_33_44_55_66", "11:22:33:44:55:66"));
        assert_eq!(card.active_profile, "a2dp-sink");
        assert!(
            card.profiles
                .iter()
                .find(|p| p.name == "a2dp-sink")
                .unwrap()
                .available
        );
        assert!(
            !card
                .profiles
                .iter()
                .find(|p| p.name == "headset-head-unit")
                .unwrap()
                .available
        );
    }

    #[test]
    fn codec_ids_are_independent_of_profile_names() {
        let codecs =
            parse_codecs(r#"[{"name":"1","description":"SBC"},{"name":"6","description":"LDAC"}]"#)
                .unwrap();
        assert_eq!(codecs[1].name, "6");
        assert_eq!(codecs[1].description, "LDAC");
        assert!(parse_codecs("not json").is_err());
    }
}
