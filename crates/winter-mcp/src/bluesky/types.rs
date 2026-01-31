//! Bluesky API types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Reference to a Bluesky post (needed for replies and threading).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostRef {
    /// AT URI (e.g., at://did:plc:xxx/app.bsky.feed.post/xxx)
    pub uri: String,
    /// Content hash
    pub cid: String,
}

/// A notification received from Bluesky.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlueskyNotification {
    /// Type of notification
    pub reason: NotificationReason,
    /// DID of the notification author
    pub author_did: String,
    /// Handle of the notification author
    pub author_handle: String,
    /// Text content (for mentions, replies, quotes)
    pub text: Option<String>,
    /// AT URI of the notification subject
    pub uri: String,
    /// Content hash
    pub cid: String,
    /// Parent post reference (for threading replies)
    pub parent: Option<PostRef>,
    /// Root post reference (for threading replies)
    pub root: Option<PostRef>,
}

/// Reason for a Bluesky notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotificationReason {
    /// Someone mentioned this account
    Mention,
    /// Someone replied to this account's post
    Reply,
    /// Someone followed this account
    Follow,
    /// Someone liked this account's post
    Like,
    /// Someone reposted this account's post
    Repost,
    /// Someone quoted this account's post
    Quote,
}

impl NotificationReason {
    /// Parse from the Bluesky API reason string.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "mention" => Some(Self::Mention),
            "reply" => Some(Self::Reply),
            "follow" => Some(Self::Follow),
            "like" => Some(Self::Like),
            "repost" => Some(Self::Repost),
            "quote" => Some(Self::Quote),
            _ => None,
        }
    }

    /// Returns true if this notification type should trigger an agent wakeup.
    pub fn triggers_wakeup(&self) -> bool {
        matches!(self, Self::Mention | Self::Reply | Self::Quote)
    }
}

/// A conversation (DM thread).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    /// Conversation ID.
    pub id: String,
    /// Members of the conversation.
    pub members: Vec<ConvoMember>,
    /// Number of unread messages.
    pub unread_count: i64,
}

/// A member of a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConvoMember {
    /// DID of the member.
    pub did: String,
    /// Handle of the member.
    pub handle: String,
    /// Display name (optional).
    pub display_name: Option<String>,
}

/// A direct message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectMessage {
    /// Message ID.
    pub id: String,
    /// Conversation ID this message belongs to.
    pub convo_id: String,
    /// DID of the sender.
    pub sender_did: String,
    /// Text content.
    pub text: String,
    /// When the message was sent.
    pub sent_at: DateTime<Utc>,
}

/// A post from the timeline (following feed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelinePost {
    /// AT URI of the post
    pub uri: String,
    /// Content hash
    pub cid: String,
    /// DID of the author
    pub author_did: String,
    /// Handle of the author
    pub author_handle: String,
    /// Display name of the author
    pub author_name: Option<String>,
    /// Post text content
    pub text: Option<String>,
    /// When the post was created
    pub created_at: Option<String>,
    /// Number of likes
    pub like_count: Option<i64>,
    /// Number of reposts
    pub repost_count: Option<i64>,
    /// Number of replies
    pub reply_count: Option<i64>,
}

/// A post from Bluesky search results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchPost {
    /// AT URI of the post
    pub uri: String,
    /// Content hash
    pub cid: String,
    /// DID of the author
    pub author_did: String,
    /// Handle of the author
    pub author_handle: String,
    /// Display name of the author
    pub author_name: Option<String>,
    /// Post text content
    pub text: Option<String>,
    /// When the post was created
    pub created_at: Option<String>,
    /// Number of likes
    pub like_count: Option<i64>,
    /// Number of reposts
    pub repost_count: Option<i64>,
    /// Number of replies
    pub reply_count: Option<i64>,
}

/// A user from Bluesky search results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchUser {
    /// DID of the user
    pub did: String,
    /// Handle of the user
    pub handle: String,
    /// Display name
    pub display_name: Option<String>,
    /// User bio/description
    pub description: Option<String>,
    /// Avatar URL
    pub avatar: Option<String>,
}
