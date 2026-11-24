//! Discord objects Concord reads, plus pure functions that turn them into what
//! the UI shows. Field names follow the official docs
//! (developers/resources/{user,guild,channel}).
//! NOTE: `GET /users/@me/channels` (existing DMs) is NOT documented by Discord.

use serde::{Deserialize, Serialize};

// ------------------------------------------------------------------ raw API

#[derive(Debug, Clone, Deserialize)]
pub struct RawGuild {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawUser {
    pub username: String,
    #[serde(default)]
    pub global_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawChannel {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: u8,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub position: i32,
    #[serde(default)]
    pub last_message_id: Option<String>,
    #[serde(default)]
    pub recipients: Vec<RawUser>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawAuthor {
    pub id: String,
}

/// An attachment; only its presence matters, so every field is ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct RawAttachment {}

/// The few fields of a message Concord needs (official docs: resources/message).
#[derive(Debug, Clone, Deserialize)]
pub struct RawMessage {
    pub id: String,
    pub author: RawAuthor,
    #[serde(rename = "type")]
    pub kind: u8,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub attachments: Vec<RawAttachment>,
}

/// Discord snowflakes start counting at 2015-01-01T00:00:00Z.
pub const DISCORD_EPOCH_MS: i64 = 1_420_070_400_000;

/// When a snowflake was created (ms since the Unix epoch).
pub fn snowflake_time_ms(id: &str) -> Option<i64> {
    id.parse::<u64>().ok().map(|n| (n >> 22) as i64 + DISCORD_EPOCH_MS)
}

/// Message types Concord will delete (all marked deletable in Discord's docs):
/// DEFAULT, CHANNEL_PINNED_MESSAGE, USER_JOIN, THREAD_CREATED, REPLY,
/// CHAT_INPUT_COMMAND, CONTEXT_MENU_COMMAND. Types the docs mark NOT deletable
/// (CALL, THREAD_STARTER_MESSAGE, ...) and everything else are left alone.
pub const DELETABLE_MESSAGE_TYPES: [u8; 7] = [0, 6, 7, 18, 19, 20, 23];

/// Body of `GET /guilds/{id}/threads/active`.
#[derive(Debug, Clone, Deserialize)]
pub struct RawThreadList {
    pub threads: Vec<RawChannel>,
}

// ------------------------------------------------------------- UI-facing types

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GuildInfo {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    Text,
    Announcement,
    Voice,
    Stage,
    Forum,
    Media,
    Thread,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChannelInfo {
    pub id: String,
    pub name: String,
    pub kind: ChannelKind,
}

/// A category (or "No category" / "Active threads") and its channels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChannelGroup {
    pub id: Option<String>,
    pub name: String,
    pub channels: Vec<ChannelInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DmInfo {
    pub id: String,
    pub name: String,
    pub is_group: bool,
}

// ------------------------------------------------------------------- helpers

/// Discord IDs are decimal digits. Anything else must never reach a URL path.
pub fn is_snowflake(s: &str) -> bool {
    !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit())
}

pub fn guild_info(g: RawGuild) -> GuildInfo {
    GuildInfo { id: g.id, name: g.name, icon: g.icon }
}

fn channel_kind(t: u8) -> Option<ChannelKind> {
    // Only channel types that can hold messages. Categories (4), directory (14)
    // and anything unknown are not listed as channels.
    match t {
        0 => Some(ChannelKind::Text),
        2 => Some(ChannelKind::Voice),
        5 => Some(ChannelKind::Announcement),
        13 => Some(ChannelKind::Stage),
        15 => Some(ChannelKind::Forum),
        16 => Some(ChannelKind::Media),
        10..=12 => Some(ChannelKind::Thread),
        _ => None,
    }
}

/// Group a guild's channels like Discord's sidebar: uncategorised first, then
/// categories by position; channels inside by position (id breaks ties).
/// `threads` (active threads) are appended as a final "Active threads" group.
pub fn group_channels(channels: Vec<RawChannel>, threads: Vec<RawChannel>) -> Vec<ChannelGroup> {
    let mut categories: Vec<&RawChannel> = channels.iter().filter(|c| c.kind == 4).collect();
    categories.sort_by_key(|c| (c.position, snowflake_num(&c.id)));

    let in_group = |parent: Option<&str>| -> Vec<ChannelInfo> {
        let mut items: Vec<&RawChannel> = channels
            .iter()
            .filter(|c| c.kind != 4 && c.parent_id.as_deref() == parent && channel_kind(c.kind).is_some())
            .collect();
        items.sort_by_key(|c| (c.position, snowflake_num(&c.id)));
        items.into_iter().map(to_channel_info).collect()
    };

    let mut groups = Vec::new();

    let uncategorised = in_group(None);
    if !uncategorised.is_empty() {
        groups.push(ChannelGroup { id: None, name: "No category".into(), channels: uncategorised });
    }
    for cat in categories {
        let list = in_group(Some(&cat.id));
        if !list.is_empty() {
            groups.push(ChannelGroup {
                id: Some(cat.id.clone()),
                name: cat.name.clone().unwrap_or_else(|| "Category".into()),
                channels: list,
            });
        }
    }

    let mut thread_items: Vec<ChannelInfo> = threads
        .iter()
        .filter(|c| channel_kind(c.kind) == Some(ChannelKind::Thread))
        .map(to_channel_info)
        .collect();
    thread_items.sort_by(|a, b| snowflake_num(&b.id).cmp(&snowflake_num(&a.id))); // newest first
    if !thread_items.is_empty() {
        groups.push(ChannelGroup { id: None, name: "Active threads".into(), channels: thread_items });
    }
    groups
}

fn to_channel_info(c: &RawChannel) -> ChannelInfo {
    ChannelInfo {
        id: c.id.clone(),
        name: c.name.clone().unwrap_or_else(|| "unnamed".into()),
        kind: channel_kind(c.kind).unwrap_or(ChannelKind::Text),
    }
}

fn snowflake_num(id: &str) -> u64 {
    id.parse().unwrap_or(0)
}

fn user_name(u: &RawUser) -> String {
    match u.global_name.as_deref().map(str::trim) {
        Some(g) if !g.is_empty() => g.to_owned(),
        _ => u.username.clone(),
    }
}

/// DM (type 1) and group DM (type 3) channels, most recently active first.
pub fn dm_list(channels: Vec<RawChannel>) -> Vec<DmInfo> {
    let mut dms: Vec<&RawChannel> = channels.iter().filter(|c| c.kind == 1 || c.kind == 3).collect();
    dms.sort_by(|a, b| {
        let key = |c: &RawChannel| snowflake_num(c.last_message_id.as_deref().unwrap_or(&c.id));
        key(b).cmp(&key(a))
    });
    dms.into_iter()
        .map(|c| {
            let is_group = c.kind == 3;
            let name = match c.name.as_deref().map(str::trim) {
                Some(n) if is_group && !n.is_empty() => n.to_owned(),
                _ => {
                    let names: Vec<String> = c.recipients.iter().map(user_name).collect();
                    if names.is_empty() { "Unknown user".into() } else { names.join(", ") }
                }
            };
            DmInfo { id: c.id.clone(), name, is_group }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(id: &str, kind: u8, name: &str, parent: Option<&str>, pos: i32) -> RawChannel {
        RawChannel {
            id: id.into(),
            kind,
            name: Some(name.into()),
            parent_id: parent.map(Into::into),
            position: pos,
            last_message_id: None,
            recipients: vec![],
        }
    }

    fn user(name: &str, global: Option<&str>) -> RawUser {
        RawUser { username: name.into(), global_name: global.map(Into::into) }
    }

    #[test]
    fn snowflake_validation() {
        assert!(is_snowflake("80351110224678912"));
        assert!(!is_snowflake(""));
        assert!(!is_snowflake("12a"));
        assert!(!is_snowflake("../users/@me"));
        assert!(!is_snowflake("123456789012345678901")); // 21 digits
    }

    #[test]
    fn groups_like_the_sidebar() {
        let channels = vec![
            ch("30", 0, "rules", None, 0),
            ch("10", 4, "Text", None, 1),
            ch("11", 4, "Voice", None, 2),
            ch("12", 4, "Empty", None, 3),
            ch("21", 0, "off-topic", Some("10"), 2),
            ch("20", 0, "general", Some("10"), 1),
            ch("22", 2, "lounge", Some("11"), 0),
            ch("23", 14, "directory", Some("11"), 1), // not message-bearing
        ];
        let g = group_channels(channels, vec![]);
        let names: Vec<_> = g.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["No category", "Text", "Voice"]); // empty category dropped
        assert_eq!(g[1].channels.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["general", "off-topic"]);
        assert_eq!(g[2].channels.len(), 1);
        assert_eq!(g[2].channels[0].kind, ChannelKind::Voice);
    }

    #[test]
    fn active_threads_go_last_newest_first() {
        let threads = vec![ch("100", 11, "old", Some("20"), 0), ch("200", 11, "new", Some("20"), 0)];
        let g = group_channels(vec![ch("20", 0, "general", None, 0)], threads);
        let last = g.last().unwrap();
        assert_eq!(last.name, "Active threads");
        assert_eq!(last.channels.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["new", "old"]);
    }

    #[test]
    fn dms_sorted_by_recent_activity_and_named() {
        let mut a = ch("1", 1, "", None, 0);
        a.name = None;
        a.recipients = vec![user("alice", Some("Alice W"))];
        a.last_message_id = Some("500".into());
        let mut b = ch("2", 3, "", None, 0);
        b.name = None;
        b.recipients = vec![user("bob", None), user("carol", None)];
        b.last_message_id = Some("900".into());
        let mut c = ch("3", 3, "Raid team", None, 0);
        c.last_message_id = Some("700".into());
        let server_channel = ch("4", 0, "general", None, 0);

        let dms = dm_list(vec![a, b, c, server_channel]);
        let names: Vec<_> = dms.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["bob, carol", "Raid team", "Alice W"]);
        assert!(dms[0].is_group && dms[1].is_group && !dms[2].is_group);
    }

    #[test]
    fn parses_real_shaped_json() {
        let json = r#"[{"id":"9","type":1,"last_message_id":null,"recipients":[{"id":"5","username":"zed","global_name":null,"avatar":"x"}],"flags":0}]"#;
        let raw: Vec<RawChannel> = serde_json::from_str(json).unwrap();
        assert_eq!(dm_list(raw)[0].name, "zed");
    }

    #[test]
    fn snowflake_timestamps() {
        // The example id from Discord's docs.
        assert_eq!(snowflake_time_ms("175928847299117063"), Some(1_462_015_105_796));
        assert_eq!(snowflake_time_ms("not-a-number"), None);
        assert_eq!(snowflake_time_ms("0"), Some(DISCORD_EPOCH_MS));
    }

    #[test]
    fn message_content_and_attachments_parse() {
        let json = r#"{"id":"5","type":0,"pinned":false,"content":"hi https://x.y",
            "author":{"id":"1","username":"u"},"attachments":[{"id":"9","filename":"a.png","size":3}],"embeds":[]}"#;
        let m: RawMessage = serde_json::from_str(json).unwrap();
        assert_eq!((m.content.as_str(), m.attachments.len()), ("hi https://x.y", 1));
        // Missing optional fields default.
        let bare: RawMessage = serde_json::from_str(r#"{"id":"5","type":0,"author":{"id":"1"}}"#).unwrap();
        assert_eq!((bare.content.as_str(), bare.attachments.len(), bare.pinned), ("", 0, false));
    }
}
