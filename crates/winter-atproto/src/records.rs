//! Record collection constants and helpers.

/// Lexicon NSID for Winter identity records.
pub const IDENTITY_COLLECTION: &str = "diy.razorgirl.winter.identity";

/// Lexicon NSID for Winter fact records.
pub const FACT_COLLECTION: &str = "diy.razorgirl.winter.fact";

/// Lexicon NSID for Winter rule records.
pub const RULE_COLLECTION: &str = "diy.razorgirl.winter.rule";

/// Lexicon NSID for Winter note records.
pub const NOTE_COLLECTION: &str = "diy.razorgirl.winter.note";

/// Lexicon NSID for Winter job records.
pub const JOB_COLLECTION: &str = "diy.razorgirl.winter.job";

/// Lexicon NSID for Winter thought records.
pub const THOUGHT_COLLECTION: &str = "diy.razorgirl.winter.thought";

/// Lexicon NSID for Winter daemon state records.
pub const STATE_COLLECTION: &str = "diy.razorgirl.winter.state";

/// Lexicon NSID for Winter custom tool records.
pub const TOOL_COLLECTION: &str = "diy.razorgirl.winter.tool";

/// Lexicon NSID for Winter tool approval records.
pub const TOOL_APPROVAL_COLLECTION: &str = "diy.razorgirl.winter.toolApproval";

/// Lexicon NSID for Winter secret metadata records.
pub const SECRET_META_COLLECTION: &str = "diy.razorgirl.winter.secretMeta";

/// Lexicon NSID for Winter directive records.
pub const DIRECTIVE_COLLECTION: &str = "diy.razorgirl.winter.directive";

/// Lexicon NSID for Winter fact declaration records.
pub const FACT_DECLARATION_COLLECTION: &str = "diy.razorgirl.winter.factDeclaration";

/// Lexicon NSID for Winter wiki entry records.
pub const WIKI_ENTRY_COLLECTION: &str = "diy.razorgirl.winter.wikiEntry";

/// Lexicon NSID for Winter wiki link records.
pub const WIKI_LINK_COLLECTION: &str = "diy.razorgirl.winter.wikiLink";

/// Lexicon NSID for WhiteWind blog entry records.
pub const BLOG_COLLECTION: &str = "com.whtwnd.blog.entry";

/// The singleton key for the identity record.
pub const IDENTITY_KEY: &str = "self";

/// The singleton key for the daemon state record.
pub const STATE_KEY: &str = "self";

/// The singleton key for the secret metadata record.
pub const SECRET_META_KEY: &str = "self";

// =============================================================================
// Bluesky Collection Constants
// =============================================================================

/// Bluesky follow records.
pub const FOLLOW_COLLECTION: &str = "app.bsky.graph.follow";

/// Bluesky like records.
pub const LIKE_COLLECTION: &str = "app.bsky.feed.like";

/// Bluesky repost records.
pub const REPOST_COLLECTION: &str = "app.bsky.feed.repost";

/// Bluesky post records.
pub const POST_COLLECTION: &str = "app.bsky.feed.post";
