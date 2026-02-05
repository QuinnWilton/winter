//! Bluesky client implementation.

use atrium_api::agent::bluesky::{AtprotoServiceType, BSKY_CHAT_DID};
use atrium_api::app::bsky::feed::like::RecordData as LikeRecordData;
use atrium_api::app::bsky::feed::post::RecordData as PostRecordData;
use atrium_api::com::atproto::repo::strong_ref::MainData as StrongRefData;
use atrium_api::types::string::{Cid, Datetime};
use bsky_sdk::BskyAgent;
use bsky_sdk::agent::config::Config as BskyConfig;
use bsky_sdk::rich_text::RichText;
use thiserror::Error;
use tracing::{debug, info};

use super::types::{
    BlueskyNotification, Conversation, ConvoMember, DirectMessage, FeedPost, FollowInfo,
    ImageInput, NotificationReason, PostRef, Profile, SearchPost, SearchUser, ThreadContext,
    ThreadPost, TimelinePost,
};

/// Errors that can occur when interacting with Bluesky.
#[derive(Debug, Error)]
pub enum BlueskyError {
    #[error("authentication failed: {0}")]
    Auth(String),

    #[error("API error: {0}")]
    Api(String),

    #[error("rate limited{}", endpoint.as_ref().map(|e| format!(" on {}", e)).unwrap_or_default())]
    RateLimited {
        /// The endpoint that was rate limited (optional).
        endpoint: Option<String>,
    },

    #[error("not configured")]
    NotConfigured,

    #[error("too many images: {count} exceeds maximum of {max}")]
    TooManyImages { count: usize, max: usize },

    #[error("image too large: {size} bytes exceeds maximum of {max} bytes")]
    ImageTooLarge { size: usize, max: usize },

    #[error("invalid image MIME type: {0}")]
    InvalidMimeType(String),
}

/// Client for interacting with Bluesky.
pub struct BlueskyClient {
    agent: BskyAgent,
    handle: String,
    /// Cursor for notification pagination (tracks last seen notification).
    last_seen_at: Option<String>,
    /// Cursor for DM pagination (tracks last seen DM timestamp).
    last_dm_cursor: Option<String>,
}

impl BlueskyClient {
    /// Create a new Bluesky client and authenticate.
    pub async fn new(
        pds_url: &str,
        handle: &str,
        app_password: &str,
    ) -> Result<Self, BlueskyError> {
        // Build agent with custom PDS URL if provided
        let agent = if pds_url == "https://bsky.social" {
            BskyAgent::builder()
                .build()
                .await
                .map_err(|e| BlueskyError::Auth(e.to_string()))?
        } else {
            let config = BskyConfig {
                endpoint: pds_url.to_string(),
                ..Default::default()
            };
            BskyAgent::builder()
                .config(config)
                .build()
                .await
                .map_err(|e| BlueskyError::Auth(e.to_string()))?
        };

        // Login with app password
        agent
            .login(handle, app_password)
            .await
            .map_err(|e| BlueskyError::Auth(e.to_string()))?;

        info!(handle = %handle, pds = %pds_url, "authenticated with bluesky");

        Ok(Self {
            agent,
            handle: handle.to_string(),
            last_seen_at: None,
            last_dm_cursor: None,
        })
    }

    /// Get the current user's DID.
    pub async fn did(&self) -> Option<String> {
        self.agent.get_session().await.map(|s| s.did.to_string())
    }

    /// Get the current user's handle.
    pub fn handle(&self) -> &str {
        &self.handle
    }

    /// Get the current notification cursor.
    pub fn last_seen_at(&self) -> Option<&str> {
        self.last_seen_at.as_deref()
    }

    /// Set the notification cursor (used to restore state on startup).
    pub fn set_last_seen_at(&mut self, cursor: Option<String>) {
        self.last_seen_at = cursor;
    }

    /// Get the current DM cursor.
    pub fn last_dm_cursor(&self) -> Option<&str> {
        self.last_dm_cursor.as_deref()
    }

    /// Set the DM cursor (used to restore state on startup).
    pub fn set_last_dm_cursor(&mut self, cursor: Option<String>) {
        self.last_dm_cursor = cursor;
    }

    /// Create a new post on Bluesky.
    ///
    /// If `facets` is provided, those facets are used directly.
    /// Otherwise, mentions (@handle) and URLs are automatically detected and linked.
    pub async fn post(
        &self,
        text: &str,
        facets: Option<Vec<winter_atproto::Facet>>,
    ) -> Result<PostRef, BlueskyError> {
        // If explicit facets provided, use them; otherwise auto-detect
        let (final_text, final_facets) = if let Some(explicit_facets) = facets {
            let atrium_facets = convert_winter_facets(&explicit_facets);
            (
                text.to_string(),
                if atrium_facets.is_empty() {
                    None
                } else {
                    Some(atrium_facets)
                },
            )
        } else {
            let rt = RichText::new_with_detect_facets(text)
                .await
                .map_err(|e| BlueskyError::Api(e.to_string()))?;
            (rt.text, rt.facets)
        };

        let record_data = PostRecordData {
            created_at: Datetime::now(),
            embed: None,
            entities: None,
            facets: final_facets,
            labels: None,
            langs: None,
            reply: None,
            tags: None,
            text: final_text,
        };

        let output = self
            .agent
            .create_record(record_data)
            .await
            .map_err(|e| BlueskyError::Api(e.to_string()))?;

        debug!(uri = %output.uri, "created bluesky post");

        Ok(PostRef {
            uri: output.uri.to_string(),
            cid: output.cid.as_ref().to_string(),
        })
    }

    /// Reply to an existing post.
    ///
    /// `parent` is the post being directly replied to.
    /// `root` is the root of the thread (same as parent for direct replies to root posts).
    /// If `facets` is provided, those facets are used directly.
    /// Otherwise, mentions (@handle) and URLs are automatically detected and linked.
    pub async fn reply(
        &self,
        text: &str,
        parent: &PostRef,
        root: &PostRef,
        facets: Option<Vec<winter_atproto::Facet>>,
    ) -> Result<PostRef, BlueskyError> {
        // If explicit facets provided, use them; otherwise auto-detect
        let (final_text, final_facets) = if let Some(explicit_facets) = facets {
            let atrium_facets = convert_winter_facets(&explicit_facets);
            (
                text.to_string(),
                if atrium_facets.is_empty() {
                    None
                } else {
                    Some(atrium_facets)
                },
            )
        } else {
            let rt = RichText::new_with_detect_facets(text)
                .await
                .map_err(|e| BlueskyError::Api(e.to_string()))?;
            (rt.text, rt.facets)
        };

        // Parse CIDs (URIs are just strings)
        let parent_cid: Cid = parent
            .cid
            .parse()
            .map_err(|e| BlueskyError::Api(format!("invalid parent CID: {}", e)))?;
        let root_cid: Cid = root
            .cid
            .parse()
            .map_err(|e| BlueskyError::Api(format!("invalid root CID: {}", e)))?;

        let reply_ref = atrium_api::app::bsky::feed::post::ReplyRefData {
            parent: StrongRefData {
                cid: parent_cid,
                uri: parent.uri.clone(),
            }
            .into(),
            root: StrongRefData {
                cid: root_cid,
                uri: root.uri.clone(),
            }
            .into(),
        };

        let record_data = PostRecordData {
            created_at: Datetime::now(),
            embed: None,
            entities: None,
            facets: final_facets,
            labels: None,
            langs: None,
            reply: Some(reply_ref.into()),
            tags: None,
            text: final_text,
        };

        let output = self
            .agent
            .create_record(record_data)
            .await
            .map_err(|e| BlueskyError::Api(e.to_string()))?;

        debug!(uri = %output.uri, parent = %parent.uri, "created bluesky reply");

        Ok(PostRef {
            uri: output.uri.to_string(),
            cid: output.cid.as_ref().to_string(),
        })
    }

    /// Upload a blob to the PDS.
    ///
    /// Returns the blob reference for use in embeds.
    async fn upload_blob(
        &self,
        data: &[u8],
        mime_type: &str,
    ) -> Result<atrium_api::types::BlobRef, BlueskyError> {
        // Validate MIME type
        const ALLOWED_MIME_TYPES: &[&str] = &["image/jpeg", "image/png", "image/webp", "image/gif"];
        if !ALLOWED_MIME_TYPES.contains(&mime_type) {
            return Err(BlueskyError::InvalidMimeType(mime_type.to_string()));
        }

        // Validate size (max 1MB)
        const MAX_IMAGE_SIZE: usize = 1_000_000;
        if data.len() > MAX_IMAGE_SIZE {
            return Err(BlueskyError::ImageTooLarge {
                size: data.len(),
                max: MAX_IMAGE_SIZE,
            });
        }

        let output = self
            .agent
            .api
            .com
            .atproto
            .repo
            .upload_blob(data.to_vec())
            .await
            .map_err(|e| BlueskyError::Api(format!("failed to upload blob: {}", e)))?;

        debug!(size = data.len(), mime_type = %mime_type, "uploaded blob");

        Ok(output.blob.clone())
    }

    /// Create a new post with images.
    ///
    /// If `facets` is provided, those facets are used directly.
    /// Otherwise, mentions (@handle) and URLs are automatically detected and linked.
    pub async fn post_with_images(
        &self,
        text: &str,
        images: Vec<ImageInput>,
        facets: Option<Vec<winter_atproto::Facet>>,
    ) -> Result<PostRef, BlueskyError> {
        // Validate image count
        const MAX_IMAGES: usize = 4;
        if images.len() > MAX_IMAGES {
            return Err(BlueskyError::TooManyImages {
                count: images.len(),
                max: MAX_IMAGES,
            });
        }

        // Upload all images
        let mut image_refs = Vec::new();
        for image in &images {
            let blob_ref = self.upload_blob(&image.data, &image.mime_type).await?;
            image_refs.push((blob_ref, image.alt.clone()));
        }

        // Build the images embed
        let embed = if !image_refs.is_empty() {
            use atrium_api::app::bsky::embed::images::{ImageData, MainData as ImagesMainData};
            use atrium_api::app::bsky::feed::post::RecordEmbedRefs;

            let embed_images: Vec<_> = image_refs
                .into_iter()
                .map(|(blob, alt)| {
                    ImageData {
                        alt,
                        aspect_ratio: None,
                        image: blob,
                    }
                    .into()
                })
                .collect();

            Some(atrium_api::types::Union::Refs(
                RecordEmbedRefs::AppBskyEmbedImagesMain(Box::new(
                    ImagesMainData {
                        images: embed_images,
                    }
                    .into(),
                )),
            ))
        } else {
            None
        };

        // If explicit facets provided, use them; otherwise auto-detect
        let (final_text, final_facets) = if let Some(explicit_facets) = facets {
            let atrium_facets = convert_winter_facets(&explicit_facets);
            (
                text.to_string(),
                if atrium_facets.is_empty() {
                    None
                } else {
                    Some(atrium_facets)
                },
            )
        } else {
            let rt = RichText::new_with_detect_facets(text)
                .await
                .map_err(|e| BlueskyError::Api(e.to_string()))?;
            (rt.text, rt.facets)
        };

        let record_data = PostRecordData {
            created_at: Datetime::now(),
            embed,
            entities: None,
            facets: final_facets,
            labels: None,
            langs: None,
            reply: None,
            tags: None,
            text: final_text,
        };

        let output = self
            .agent
            .create_record(record_data)
            .await
            .map_err(|e| BlueskyError::Api(e.to_string()))?;

        debug!(uri = %output.uri, image_count = images.len(), "created bluesky post with images");

        Ok(PostRef {
            uri: output.uri.to_string(),
            cid: output.cid.as_ref().to_string(),
        })
    }

    /// Reply to an existing post with images.
    ///
    /// `parent` is the post being directly replied to.
    /// `root` is the root of the thread (same as parent for direct replies to root posts).
    /// If `facets` is provided, those facets are used directly.
    /// Otherwise, mentions (@handle) and URLs are automatically detected and linked.
    pub async fn reply_with_images(
        &self,
        text: &str,
        parent: &PostRef,
        root: &PostRef,
        images: Vec<ImageInput>,
        facets: Option<Vec<winter_atproto::Facet>>,
    ) -> Result<PostRef, BlueskyError> {
        // Validate image count
        const MAX_IMAGES: usize = 4;
        if images.len() > MAX_IMAGES {
            return Err(BlueskyError::TooManyImages {
                count: images.len(),
                max: MAX_IMAGES,
            });
        }

        // Upload all images
        let mut image_refs = Vec::new();
        for image in &images {
            let blob_ref = self.upload_blob(&image.data, &image.mime_type).await?;
            image_refs.push((blob_ref, image.alt.clone()));
        }

        // Build the images embed
        let embed = if !image_refs.is_empty() {
            use atrium_api::app::bsky::embed::images::{ImageData, MainData as ImagesMainData};
            use atrium_api::app::bsky::feed::post::RecordEmbedRefs;

            let embed_images: Vec<_> = image_refs
                .into_iter()
                .map(|(blob, alt)| {
                    ImageData {
                        alt,
                        aspect_ratio: None,
                        image: blob,
                    }
                    .into()
                })
                .collect();

            Some(atrium_api::types::Union::Refs(
                RecordEmbedRefs::AppBskyEmbedImagesMain(Box::new(
                    ImagesMainData {
                        images: embed_images,
                    }
                    .into(),
                )),
            ))
        } else {
            None
        };

        // If explicit facets provided, use them; otherwise auto-detect
        let (final_text, final_facets) = if let Some(explicit_facets) = facets {
            let atrium_facets = convert_winter_facets(&explicit_facets);
            (
                text.to_string(),
                if atrium_facets.is_empty() {
                    None
                } else {
                    Some(atrium_facets)
                },
            )
        } else {
            let rt = RichText::new_with_detect_facets(text)
                .await
                .map_err(|e| BlueskyError::Api(e.to_string()))?;
            (rt.text, rt.facets)
        };

        // Parse CIDs (URIs are just strings)
        let parent_cid: Cid = parent
            .cid
            .parse()
            .map_err(|e| BlueskyError::Api(format!("invalid parent CID: {}", e)))?;
        let root_cid: Cid = root
            .cid
            .parse()
            .map_err(|e| BlueskyError::Api(format!("invalid root CID: {}", e)))?;

        let reply_ref = atrium_api::app::bsky::feed::post::ReplyRefData {
            parent: StrongRefData {
                cid: parent_cid,
                uri: parent.uri.clone(),
            }
            .into(),
            root: StrongRefData {
                cid: root_cid,
                uri: root.uri.clone(),
            }
            .into(),
        };

        let record_data = PostRecordData {
            created_at: Datetime::now(),
            embed,
            entities: None,
            facets: final_facets,
            labels: None,
            langs: None,
            reply: Some(reply_ref.into()),
            tags: None,
            text: final_text,
        };

        let output = self
            .agent
            .create_record(record_data)
            .await
            .map_err(|e| BlueskyError::Api(e.to_string()))?;

        debug!(uri = %output.uri, parent = %parent.uri, image_count = images.len(), "created bluesky reply with images");

        Ok(PostRef {
            uri: output.uri.to_string(),
            cid: output.cid.as_ref().to_string(),
        })
    }

    /// Send a direct message to a user.
    ///
    /// If `facets` is provided, those facets are used directly.
    /// Otherwise, mentions (@handle) and URLs are automatically detected and linked.
    /// Note: DMs on Bluesky use the chat.bsky lexicon and require proxying to the chat service.
    #[tracing::instrument(skip(self, text, facets), fields(recipient = %recipient_did))]
    pub async fn send_dm(
        &self,
        recipient_did: &str,
        text: &str,
        facets: Option<Vec<winter_atproto::Facet>>,
    ) -> Result<String, BlueskyError> {
        debug!("sending DM, getting chat API proxy");

        // Get chat API with proxy to the Bluesky chat service
        let chat_did = BSKY_CHAT_DID
            .parse()
            .map_err(|e| BlueskyError::Api(format!("invalid chat DID: {}", e)))?;
        let chat_api = self
            .agent
            .api_with_proxy(chat_did, AtprotoServiceType::BskyChat);

        debug!("getting or creating conversation");

        // Get or create a conversation with the recipient
        let convo =
            chat_api
                .chat
                .bsky
                .convo
                .get_convo_for_members(
                    atrium_api::chat::bsky::convo::get_convo_for_members::ParametersData {
                        members: vec![recipient_did.parse().map_err(|e| {
                            BlueskyError::Api(format!("invalid recipient DID: {}", e))
                        })?],
                    }
                    .into(),
                )
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "failed to get/create conversation");
                    BlueskyError::Api(format!("failed to get/create conversation: {}", e))
                })?;

        let convo_id = convo.convo.id.clone();
        debug!(convo_id = %convo_id, "got conversation, sending message");

        // If explicit facets provided, use them; otherwise auto-detect
        let (final_text, final_facets) = if let Some(explicit_facets) = facets {
            let atrium_facets = convert_winter_facets(&explicit_facets);
            (
                text.to_string(),
                if atrium_facets.is_empty() {
                    None
                } else {
                    Some(atrium_facets)
                },
            )
        } else {
            let rt = RichText::new_with_detect_facets(text)
                .await
                .map_err(|e| BlueskyError::Api(e.to_string()))?;
            (rt.text, rt.facets)
        };

        // Send message to the conversation
        let message = chat_api
            .chat
            .bsky
            .convo
            .send_message(
                atrium_api::chat::bsky::convo::send_message::InputData {
                    convo_id: convo_id.clone(),
                    message: atrium_api::chat::bsky::convo::defs::MessageInputData {
                        embed: None,
                        facets: final_facets,
                        text: final_text,
                    }
                    .into(),
                }
                .into(),
            )
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "failed to send message");
                BlueskyError::Api(format!("failed to send message: {}", e))
            })?;

        debug!(convo_id = %convo_id, message_id = %message.id, "sent bluesky dm");

        Ok(message.id.clone())
    }

    /// Send a direct message to an existing conversation.
    ///
    /// This is more reliable than `send_dm` when replying to a conversation
    /// where the convo_id is already known, as it skips the get_convo_for_members lookup.
    /// If `facets` is provided, those facets are used directly.
    /// Otherwise, mentions (@handle) and URLs are automatically detected and linked.
    #[tracing::instrument(skip(self, text, facets), fields(convo_id = %convo_id))]
    pub async fn send_dm_to_convo(
        &self,
        convo_id: &str,
        text: &str,
        facets: Option<Vec<winter_atproto::Facet>>,
    ) -> Result<String, BlueskyError> {
        debug!("sending DM to existing conversation");

        // Get chat API with proxy to the Bluesky chat service
        let chat_did = BSKY_CHAT_DID
            .parse()
            .map_err(|e| BlueskyError::Api(format!("invalid chat DID: {}", e)))?;
        let chat_api = self
            .agent
            .api_with_proxy(chat_did, AtprotoServiceType::BskyChat);

        // If explicit facets provided, use them; otherwise auto-detect
        let (final_text, final_facets) = if let Some(explicit_facets) = facets {
            let atrium_facets = convert_winter_facets(&explicit_facets);
            (
                text.to_string(),
                if atrium_facets.is_empty() {
                    None
                } else {
                    Some(atrium_facets)
                },
            )
        } else {
            let rt = RichText::new_with_detect_facets(text)
                .await
                .map_err(|e| BlueskyError::Api(e.to_string()))?;
            (rt.text, rt.facets)
        };

        // Send message directly to the conversation
        let message = chat_api
            .chat
            .bsky
            .convo
            .send_message(
                atrium_api::chat::bsky::convo::send_message::InputData {
                    convo_id: convo_id.to_string(),
                    message: atrium_api::chat::bsky::convo::defs::MessageInputData {
                        embed: None,
                        facets: final_facets,
                        text: final_text,
                    }
                    .into(),
                }
                .into(),
            )
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "failed to send message to conversation");
                BlueskyError::Api(format!("failed to send message: {}", e))
            })?;

        debug!(convo_id = %convo_id, message_id = %message.id, "sent DM to conversation");

        Ok(message.id.clone())
    }

    /// Like a post.
    pub async fn like(&self, uri: &str, cid: &str) -> Result<String, BlueskyError> {
        let cid: Cid = cid
            .parse()
            .map_err(|e| BlueskyError::Api(format!("invalid CID: {}", e)))?;

        let record_data = LikeRecordData {
            created_at: Datetime::now(),
            subject: StrongRefData {
                cid,
                uri: uri.to_string(),
            }
            .into(),
            via: None,
        };

        let output = self
            .agent
            .create_record(record_data)
            .await
            .map_err(|e| BlueskyError::Api(e.to_string()))?;

        debug!(like_uri = %output.uri, "liked bluesky post");

        Ok(output.uri.to_string())
    }

    /// Follow a user by their DID or handle.
    pub async fn follow(&self, subject: &str) -> Result<String, BlueskyError> {
        // Parse as DID (subject can be a DID like "did:plc:xxx" or we need to resolve handle)
        let did: atrium_api::types::string::Did = if subject.starts_with("did:") {
            subject
                .parse()
                .map_err(|e| BlueskyError::Api(format!("invalid DID: {}", e)))?
        } else {
            // Resolve handle to DID
            let resolved = self
                .agent
                .api
                .com
                .atproto
                .identity
                .resolve_handle(
                    atrium_api::com::atproto::identity::resolve_handle::ParametersData {
                        handle: subject
                            .parse()
                            .map_err(|e| BlueskyError::Api(format!("invalid handle: {}", e)))?,
                    }
                    .into(),
                )
                .await
                .map_err(|e| BlueskyError::Api(format!("failed to resolve handle: {}", e)))?;
            resolved.did.clone()
        };

        let record_data = atrium_api::app::bsky::graph::follow::RecordData {
            created_at: Datetime::now(),
            subject: did.clone(),
        };

        let output = self
            .agent
            .create_record(record_data)
            .await
            .map_err(|e| BlueskyError::Api(e.to_string()))?;

        debug!(follow_uri = %output.uri, subject = %did.as_str(), "followed user");

        Ok(output.uri.to_string())
    }

    /// Get the home timeline (posts from followed accounts).
    ///
    /// Returns a list of posts with author info and text.
    pub async fn get_timeline(&self, limit: Option<u8>) -> Result<Vec<TimelinePost>, BlueskyError> {
        let params = atrium_api::app::bsky::feed::get_timeline::ParametersData {
            algorithm: None,
            cursor: None,
            limit: limit.map(|l| l.clamp(1, 100).try_into().unwrap()),
        };

        let output = self
            .agent
            .api
            .app
            .bsky
            .feed
            .get_timeline(params.into())
            .await
            .map_err(|e| BlueskyError::Api(e.to_string()))?;

        let posts: Vec<TimelinePost> = output
            .feed
            .iter()
            .map(|item| {
                // Extract text from the post record
                let text = self.extract_post_text(&item.post.record);

                TimelinePost {
                    uri: item.post.uri.clone(),
                    cid: item.post.cid.as_ref().to_string(),
                    author_did: item.post.author.did.to_string(),
                    author_handle: item.post.author.handle.to_string(),
                    author_name: item.post.author.display_name.clone(),
                    text,
                    created_at: self.extract_post_created_at(&item.post.record),
                    like_count: item.post.like_count,
                    repost_count: item.post.repost_count,
                    reply_count: item.post.reply_count,
                }
            })
            .collect();

        debug!(count = posts.len(), "fetched timeline");

        Ok(posts)
    }

    /// Get a post thread with full context.
    ///
    /// Returns the thread structure with all posts flattened, participants listed,
    /// and participation metrics for the current user.
    pub async fn get_post_thread(
        &self,
        uri: &str,
        depth: Option<u16>,
    ) -> Result<ThreadContext, BlueskyError> {
        use atrium_api::app::bsky::feed::get_post_thread::OutputThreadRefs;
        use std::collections::HashSet;

        // Limit parent_height to avoid stack overflow during deserialization.
        // The atrium_api uses recursive serde deserialization for nested ThreadViewPost,
        // which can exhaust the stack for deep thread hierarchies.
        let params = atrium_api::app::bsky::feed::get_post_thread::ParametersData {
            uri: uri.to_string(),
            depth: depth.map(|d| d.clamp(0, 100).try_into().unwrap()),
            parent_height: Some(50.try_into().unwrap()),
        };

        let output = self
            .agent
            .api
            .app
            .bsky
            .feed
            .get_post_thread(params.into())
            .await
            .map_err(|e| {
                let error_str = e.to_string();
                if error_str.contains("RateLimitExceeded") || error_str.contains("429") {
                    BlueskyError::RateLimited {
                        endpoint: Some("getPostThread".to_string()),
                    }
                } else {
                    BlueskyError::Api(error_str)
                }
            })?;

        // Parse the thread view into our flattened structure
        let mut posts = Vec::new();
        let mut participants = HashSet::new();
        let my_did = self.did().await;

        // Convert a thread view to a ThreadPost
        fn thread_view_to_post(
            thread_view: &atrium_api::app::bsky::feed::defs::ThreadViewPost,
            depth: u32,
            parent_uri: Option<&str>,
            participants: &mut HashSet<String>,
            client: &BlueskyClient,
        ) -> ThreadPost {
            participants.insert(thread_view.post.author.did.to_string());
            ThreadPost {
                uri: thread_view.post.uri.clone(),
                cid: thread_view.post.cid.as_ref().to_string(),
                author_did: thread_view.post.author.did.to_string(),
                author_handle: thread_view.post.author.handle.to_string(),
                text: client.extract_post_text(&thread_view.post.record),
                created_at: client.extract_post_created_at(&thread_view.post.record),
                reply_count: thread_view.post.reply_count,
                parent_uri: parent_uri.map(|s| s.to_string()),
                depth,
            }
        }

        // Extract root post by traversing up the parent chain
        let root_post = match &output.thread {
            atrium_api::types::Union::Refs(OutputThreadRefs::AppBskyFeedDefsThreadViewPost(
                thread_view,
            )) => {
                use atrium_api::app::bsky::feed::defs::ThreadViewPostParentRefs;

                // Collect ancestor chain by traversing up parents
                let mut ancestors: Vec<&atrium_api::app::bsky::feed::defs::ThreadViewPost> =
                    Vec::new();
                let mut current = thread_view.as_ref();

                // Walk up the parent chain
                while let Some(ref parent) = current.parent {
                    if let atrium_api::types::Union::Refs(
                        ThreadViewPostParentRefs::ThreadViewPost(parent_view),
                    ) = parent
                    {
                        ancestors.push(parent_view.as_ref());
                        current = parent_view.as_ref();
                    } else {
                        // Parent is blocked or not found, stop traversal
                        break;
                    }
                }

                // Reverse ancestors so they're in root -> ... -> parent order
                ancestors.reverse();

                // Process ancestors first (root is at ancestors[0] if any)
                // Note: ancestors don't have their replies populated, only the path to root
                let mut depth = 0u32;
                let mut last_uri: Option<String> = None;

                for ancestor in &ancestors {
                    let post = thread_view_to_post(
                        ancestor,
                        depth,
                        last_uri.as_deref(),
                        &mut participants,
                        self,
                    );
                    last_uri = Some(ancestor.post.uri.clone());
                    posts.push(post);
                    depth += 1;
                }

                // Process the originally requested post
                let post = thread_view_to_post(
                    thread_view.as_ref(),
                    depth,
                    last_uri.as_deref(),
                    &mut participants,
                    self,
                );
                posts.push(post);

                // Process replies iteratively using an explicit stack to avoid stack overflow
                // Stack entries: (replies, parent_uri, depth)
                use atrium_api::app::bsky::feed::defs::ThreadViewPostRepliesItem;
                type RepliesRef<'a> =
                    &'a Option<Vec<atrium_api::types::Union<ThreadViewPostRepliesItem>>>;
                let mut stack: Vec<(RepliesRef<'_>, String, u32)> = vec![(
                    &thread_view.replies,
                    thread_view.post.uri.clone(),
                    depth + 1,
                )];

                while let Some((replies, parent_uri, current_depth)) = stack.pop() {
                    if let Some(replies) = replies {
                        for reply in replies {
                            if let atrium_api::types::Union::Refs(
                                ThreadViewPostRepliesItem::ThreadViewPost(reply_view),
                            ) = reply
                            {
                                participants.insert(reply_view.post.author.did.to_string());
                                let post = ThreadPost {
                                    uri: reply_view.post.uri.clone(),
                                    cid: reply_view.post.cid.as_ref().to_string(),
                                    author_did: reply_view.post.author.did.to_string(),
                                    author_handle: reply_view.post.author.handle.to_string(),
                                    text: self.extract_post_text(&reply_view.post.record),
                                    created_at: self
                                        .extract_post_created_at(&reply_view.post.record),
                                    reply_count: reply_view.post.reply_count,
                                    parent_uri: Some(parent_uri.clone()),
                                    depth: current_depth,
                                };
                                posts.push(post);

                                // Push nested replies onto the stack for later processing
                                stack.push((
                                    &reply_view.replies,
                                    reply_view.post.uri.clone(),
                                    current_depth + 1,
                                ));
                            }
                        }
                    }
                }

                // The root post is the first one in the list
                posts
                    .first()
                    .cloned()
                    .ok_or_else(|| BlueskyError::Api("Empty thread".to_string()))?
            }
            atrium_api::types::Union::Refs(OutputThreadRefs::AppBskyFeedDefsBlockedPost(_)) => {
                return Err(BlueskyError::Api("Thread is blocked".to_string()));
            }
            atrium_api::types::Union::Refs(OutputThreadRefs::AppBskyFeedDefsNotFoundPost(_)) => {
                return Err(BlueskyError::Api("Thread not found".to_string()));
            }
            atrium_api::types::Union::Unknown(_) => {
                return Err(BlueskyError::Api("Unknown thread type".to_string()));
            }
        };

        // Calculate participation metrics
        let total_replies = posts.len().saturating_sub(1); // Exclude root
        let (my_reply_count, my_last_reply_at, posts_since_my_last_reply) =
            if let Some(ref my_did) = my_did {
                let my_posts: Vec<_> = posts.iter().filter(|p| &p.author_did == my_did).collect();
                let my_count = my_posts.len();

                let my_last = my_posts
                    .iter()
                    .filter_map(|p| p.created_at.as_ref())
                    .max()
                    .cloned();

                let since_count = if let Some(ref last_time) = my_last {
                    posts
                        .iter()
                        .filter(|p| {
                            p.created_at
                                .as_ref()
                                .map(|t| t > last_time)
                                .unwrap_or(false)
                        })
                        .count()
                } else {
                    0
                };

                (my_count, my_last, since_count)
            } else {
                (0, None, 0)
            };

        let context = ThreadContext {
            root: root_post,
            posts,
            participants: participants.into_iter().collect(),
            total_replies,
            my_reply_count,
            my_last_reply_at,
            posts_since_my_last_reply,
        };

        debug!(
            uri = %uri,
            total_replies = context.total_replies,
            participants = context.participants.len(),
            "fetched thread context"
        );

        Ok(context)
    }

    /// Get recent notifications.
    pub async fn get_notifications(
        &mut self,
        limit: Option<u8>,
    ) -> Result<Vec<BlueskyNotification>, BlueskyError> {
        let params = atrium_api::app::bsky::notification::list_notifications::ParametersData {
            cursor: None,
            limit: Some(limit.unwrap_or(50).clamp(1, 100).try_into().unwrap()),
            priority: None,
            reasons: None,
            seen_at: None,
        };

        let output = self
            .agent
            .api
            .app
            .bsky
            .notification
            .list_notifications(params.into())
            .await
            .map_err(|e| {
                let error_str = e.to_string();
                if error_str.contains("RateLimitExceeded") || error_str.contains("429") {
                    BlueskyError::RateLimited {
                        endpoint: Some("listNotifications".to_string()),
                    }
                } else {
                    BlueskyError::Api(error_str)
                }
            })?;

        // Get the newest timestamp from this batch
        let newest_timestamp = output
            .notifications
            .first()
            .map(|n| n.indexed_at.as_str().to_string());

        // Filter out already-seen notifications
        let mut notifications = Vec::new();

        // Parse last_seen timestamp for proper datetime comparison
        let last_seen_dt = self
            .last_seen_at
            .as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());

        for notif in &output.notifications {
            let indexed_at = notif.indexed_at.as_str();

            // Skip notifications we've already seen (use datetime comparison, not string)
            if let Some(ref last_seen_dt) = last_seen_dt
                && let Ok(indexed_dt) = chrono::DateTime::parse_from_rfc3339(indexed_at)
                && indexed_dt <= *last_seen_dt
            {
                continue;
            }

            let reason = match NotificationReason::parse(&notif.reason) {
                Some(r) => r,
                None => {
                    debug!(reason = %notif.reason, "unknown notification reason, skipping");
                    continue;
                }
            };

            // Extract text, reply refs, and facets from the record (if it's a post)
            let (text, parent, root, facets) = self.extract_post_data(&notif.record);

            notifications.push(BlueskyNotification {
                reason,
                author_did: notif.author.did.to_string(),
                author_handle: notif.author.handle.to_string(),
                text,
                uri: notif.uri.clone(),
                cid: notif.cid.as_ref().to_string(),
                parent,
                root,
                facets,
            });
        }

        // Update last_seen_at to the newest timestamp
        if let Some(ts) = newest_timestamp {
            self.last_seen_at = Some(ts);
        }

        Ok(notifications)
    }

    /// Extract post text from a record.
    fn extract_post_text(&self, record: &atrium_api::types::Unknown) -> Option<String> {
        serde_json::from_value::<DeserPostRecord>(
            serde_json::to_value(record).unwrap_or(serde_json::Value::Null),
        )
        .ok()
        .map(|p| p.text)
    }

    /// Extract created_at from a record.
    fn extract_post_created_at(&self, record: &atrium_api::types::Unknown) -> Option<String> {
        #[derive(serde::Deserialize)]
        struct PostWithTime {
            #[serde(rename = "createdAt")]
            created_at: Option<String>,
        }
        serde_json::from_value::<PostWithTime>(
            serde_json::to_value(record).unwrap_or(serde_json::Value::Null),
        )
        .ok()
        .and_then(|p| p.created_at)
    }

    /// Extract post text, reply references, and facets from a record.
    fn extract_post_data(
        &self,
        record: &atrium_api::types::Unknown,
    ) -> (
        Option<String>,
        Option<PostRef>,
        Option<PostRef>,
        Vec<winter_atproto::Facet>,
    ) {
        // Try to deserialize as a post record
        if let Ok(post) = serde_json::from_value::<DeserPostRecord>(
            serde_json::to_value(record).unwrap_or(serde_json::Value::Null),
        ) {
            let text = Some(post.text);
            let (parent, root) = if let Some(reply) = post.reply {
                (
                    Some(PostRef {
                        uri: reply.parent.uri,
                        cid: reply.parent.cid,
                    }),
                    Some(PostRef {
                        uri: reply.root.uri,
                        cid: reply.root.cid,
                    }),
                )
            } else {
                (None, None)
            };
            (text, parent, root, post.facets)
        } else {
            (None, None, None, Vec::new())
        }
    }

    /// List conversations with unread messages or recent activity.
    pub async fn list_conversations(&self) -> Result<Vec<Conversation>, BlueskyError> {
        // Get chat API with proxy to the Bluesky chat service
        let chat_did = BSKY_CHAT_DID
            .parse()
            .map_err(|e| BlueskyError::Api(format!("invalid chat DID: {}", e)))?;
        let chat_api = self
            .agent
            .api_with_proxy(chat_did, AtprotoServiceType::BskyChat);

        let output = chat_api
            .chat
            .bsky
            .convo
            .list_convos(
                atrium_api::chat::bsky::convo::list_convos::ParametersData {
                    cursor: None,
                    limit: Some(50.try_into().unwrap()),
                    read_state: None,
                    status: None,
                }
                .into(),
            )
            .await
            .map_err(|e| BlueskyError::Api(format!("failed to list conversations: {}", e)))?;

        let convos_len = output.convos.len();
        let conversations = output
            .convos
            .iter()
            .map(|convo| Conversation {
                id: convo.id.clone(),
                members: convo
                    .members
                    .iter()
                    .map(|m| ConvoMember {
                        did: m.did.to_string(),
                        handle: m.handle.to_string(),
                        display_name: m.display_name.clone(),
                    })
                    .collect(),
                unread_count: convo.unread_count,
            })
            .collect();

        debug!(count = convos_len, "listed conversations");

        Ok(conversations)
    }

    /// Get messages from a conversation.
    pub async fn get_messages(
        &self,
        convo_id: &str,
        cursor: Option<&str>,
    ) -> Result<Vec<DirectMessage>, BlueskyError> {
        // Get chat API with proxy to the Bluesky chat service
        let chat_did = BSKY_CHAT_DID
            .parse()
            .map_err(|e| BlueskyError::Api(format!("invalid chat DID: {}", e)))?;
        let chat_api = self
            .agent
            .api_with_proxy(chat_did, AtprotoServiceType::BskyChat);

        let output = chat_api
            .chat
            .bsky
            .convo
            .get_messages(
                atrium_api::chat::bsky::convo::get_messages::ParametersData {
                    convo_id: convo_id.to_string(),
                    cursor: cursor.map(|s| s.to_string()),
                    limit: Some(50.try_into().unwrap()),
                }
                .into(),
            )
            .await
            .map_err(|e| BlueskyError::Api(format!("failed to get messages: {}", e)))?;

        use atrium_api::chat::bsky::convo::get_messages::OutputMessagesItem;

        let messages_len = output.messages.len();

        // First pass: extract basic message data
        let mut messages = Vec::new();
        for msg in output.messages.iter() {
            // Skip deleted messages or unknown types
            let atrium_api::types::Union::Refs(OutputMessagesItem::ChatBskyConvoDefsMessageView(
                view,
            )) = msg
            else {
                continue;
            };

            let Some(sent_at) = chrono::DateTime::parse_from_rfc3339(view.sent_at.as_str())
                .ok()
                .map(|dt| dt.with_timezone(&chrono::Utc))
            else {
                continue;
            };

            // Convert atrium facets to our Facet type
            let api_facets = view
                .facets
                .as_ref()
                .map(|facets| convert_atrium_facets(facets))
                .unwrap_or_default();

            // If API returned no facets, try to detect them from text
            let facets = if api_facets.is_empty() {
                match RichText::new_with_detect_facets(&view.text).await {
                    Ok(rt) => rt
                        .facets
                        .map(|f| convert_atrium_facets(&f))
                        .unwrap_or_default(),
                    Err(e) => {
                        debug!(error = %e, "failed to detect facets in DM text");
                        api_facets
                    }
                }
            } else {
                api_facets
            };

            messages.push(DirectMessage {
                id: view.id.clone(),
                convo_id: convo_id.to_string(),
                sender_did: view.sender.did.to_string(),
                text: view.text.clone(),
                sent_at,
                facets,
            });
        }

        debug!(convo_id, count = messages_len, "fetched messages");

        Ok(messages)
    }

    /// Get unread direct messages, filtering by cursor.
    ///
    /// Returns messages newer than the current DM cursor and updates the cursor.
    /// Automatically filters out messages sent by the current user (to avoid echo loops).
    pub async fn get_unread_dms(&mut self) -> Result<Vec<DirectMessage>, BlueskyError> {
        // Get our own DID to filter out self-sent messages
        let own_did = self.did().await;

        // List conversations with unread messages
        let convos = self.list_conversations().await?;
        let unread_convos: Vec<_> = convos.into_iter().filter(|c| c.unread_count > 0).collect();

        if unread_convos.is_empty() {
            return Ok(Vec::new());
        }

        let mut all_messages = Vec::new();
        let mut newest_dt: Option<chrono::DateTime<chrono::Utc>> = None;

        // Parse cursor as datetime for proper comparison
        let cursor_dt = self
            .last_dm_cursor
            .as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));

        for convo in unread_convos {
            let messages = self.get_messages(&convo.id, None).await?;

            for msg in messages {
                // Skip messages we've already seen (use datetime comparison)
                if let Some(ref cursor_dt) = cursor_dt
                    && msg.sent_at <= *cursor_dt
                {
                    continue;
                }

                // Skip messages from ourselves (avoid echo loops)
                if let Some(ref own_did) = own_did
                    && msg.sender_did == *own_did
                {
                    debug!(message_id = %msg.id, "skipping self-sent DM");
                    // Still track timestamp to advance cursor past our own messages
                    if newest_dt.is_none() || msg.sent_at > newest_dt.unwrap() {
                        newest_dt = Some(msg.sent_at);
                    }
                    continue;
                }

                // Track newest timestamp
                if newest_dt.is_none() || msg.sent_at > newest_dt.unwrap() {
                    newest_dt = Some(msg.sent_at);
                }

                all_messages.push(msg);
            }
        }

        // Update cursor to newest message timestamp
        if let Some(dt) = newest_dt {
            self.last_dm_cursor = Some(dt.to_rfc3339());
        }

        // Sort by sent_at ascending (oldest first)
        all_messages.sort_by(|a, b| a.sent_at.cmp(&b.sent_at));

        debug!(count = all_messages.len(), "fetched unread DMs");

        Ok(all_messages)
    }

    /// Search for posts across Bluesky.
    ///
    /// Returns posts matching the query with optional filters.
    #[allow(clippy::too_many_arguments)]
    pub async fn search_posts(
        &self,
        query: &str,
        author: Option<&str>,
        since: Option<&str>,
        until: Option<&str>,
        lang: Option<&str>,
        tag: Option<Vec<String>>,
        sort: Option<&str>,
        limit: Option<u8>,
        cursor: Option<&str>,
    ) -> Result<(Vec<SearchPost>, Option<String>), BlueskyError> {
        // Parse author as AtIdentifier if provided
        let author_id = if let Some(a) = author {
            Some(
                a.parse()
                    .map_err(|e| BlueskyError::Api(format!("invalid author identifier: {}", e)))?,
            )
        } else {
            None
        };

        // Parse lang as Language if provided
        let lang_parsed = if let Some(l) = lang {
            Some(
                l.parse()
                    .map_err(|e| BlueskyError::Api(format!("invalid language code: {}", e)))?,
            )
        } else {
            None
        };

        let params = atrium_api::app::bsky::feed::search_posts::ParametersData {
            author: author_id,
            cursor: cursor.map(|s| s.to_string()),
            domain: None,
            lang: lang_parsed,
            limit: limit.map(|l| l.clamp(1, 100).try_into().unwrap()),
            mentions: None,
            q: query.to_string(),
            since: since.map(|s| s.to_string()),
            sort: sort.map(|s| s.to_string()),
            tag,
            until: until.map(|s| s.to_string()),
            url: None,
        };

        let output = self
            .agent
            .api
            .app
            .bsky
            .feed
            .search_posts(params.into())
            .await
            .map_err(|e| {
                let error_str = e.to_string();
                if error_str.contains("RateLimitExceeded") || error_str.contains("429") {
                    BlueskyError::RateLimited {
                        endpoint: Some("searchPosts".to_string()),
                    }
                } else {
                    BlueskyError::Api(error_str)
                }
            })?;

        let posts: Vec<SearchPost> = output
            .posts
            .iter()
            .map(|post| {
                let text = self.extract_post_text(&post.record);

                SearchPost {
                    uri: post.uri.clone(),
                    cid: post.cid.as_ref().to_string(),
                    author_did: post.author.did.to_string(),
                    author_handle: post.author.handle.to_string(),
                    author_name: post.author.display_name.clone(),
                    text,
                    created_at: self.extract_post_created_at(&post.record),
                    like_count: post.like_count,
                    repost_count: post.repost_count,
                    reply_count: post.reply_count,
                }
            })
            .collect();

        debug!(query = %query, count = posts.len(), "searched posts");

        Ok((posts, output.cursor.clone()))
    }

    /// Get all followers of the current user.
    ///
    /// Handles pagination internally to fetch the complete list.
    /// Returns a list of DIDs for all followers.
    pub async fn get_all_followers(&self) -> Result<Vec<String>, BlueskyError> {
        let did = self.did().await.ok_or(BlueskyError::NotConfigured)?;
        let mut all_dids = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let params = atrium_api::app::bsky::graph::get_followers::ParametersData {
                actor: did
                    .parse()
                    .map_err(|e| BlueskyError::Api(format!("invalid DID: {}", e)))?,
                cursor: cursor.clone(),
                limit: Some(100.try_into().unwrap()),
            };

            let output = self
                .agent
                .api
                .app
                .bsky
                .graph
                .get_followers(params.into())
                .await
                .map_err(|e| {
                    let error_str = e.to_string();
                    if error_str.contains("RateLimitExceeded") || error_str.contains("429") {
                        BlueskyError::RateLimited {
                            endpoint: Some("getFollowers".to_string()),
                        }
                    } else {
                        BlueskyError::Api(error_str)
                    }
                })?;

            all_dids.extend(output.followers.iter().map(|f| f.did.to_string()));

            cursor = output.cursor.clone();
            if cursor.is_none() {
                break;
            }
        }

        debug!(count = all_dids.len(), "fetched all followers");

        Ok(all_dids)
    }

    /// Search for users across Bluesky.
    ///
    /// Returns users matching the query (by name, handle, or bio).
    pub async fn search_users(
        &self,
        query: &str,
        limit: Option<u8>,
        cursor: Option<&str>,
    ) -> Result<(Vec<SearchUser>, Option<String>), BlueskyError> {
        let params = atrium_api::app::bsky::actor::search_actors::ParametersData {
            cursor: cursor.map(|s| s.to_string()),
            limit: limit.map(|l| l.clamp(1, 100).try_into().unwrap()),
            q: Some(query.to_string()),
            term: None,
        };

        let output = self
            .agent
            .api
            .app
            .bsky
            .actor
            .search_actors(params.into())
            .await
            .map_err(|e| {
                let error_str = e.to_string();
                if error_str.contains("RateLimitExceeded") || error_str.contains("429") {
                    BlueskyError::RateLimited {
                        endpoint: Some("searchActors".to_string()),
                    }
                } else {
                    BlueskyError::Api(error_str)
                }
            })?;

        let users: Vec<SearchUser> = output
            .actors
            .iter()
            .map(|actor| SearchUser {
                did: actor.did.to_string(),
                handle: actor.handle.to_string(),
                display_name: actor.display_name.clone(),
                description: actor.description.clone(),
                avatar: actor.avatar.clone(),
            })
            .collect();

        debug!(query = %query, count = users.len(), "searched users");

        Ok((users, output.cursor.clone()))
    }

    /// Mute a user by their DID.
    ///
    /// Muted users won't appear in your timeline or notifications.
    pub async fn mute(&self, did: &str) -> Result<(), BlueskyError> {
        let parsed_did: atrium_api::types::string::Did = did
            .parse()
            .map_err(|e| BlueskyError::Api(format!("invalid DID: {}", e)))?;

        self.agent
            .api
            .app
            .bsky
            .graph
            .mute_actor(
                atrium_api::app::bsky::graph::mute_actor::InputData {
                    actor: parsed_did.into(),
                }
                .into(),
            )
            .await
            .map_err(|e| BlueskyError::Api(format!("failed to mute user: {}", e)))?;

        debug!(did = %did, "muted user");

        Ok(())
    }

    /// Unmute a previously muted user.
    pub async fn unmute(&self, did: &str) -> Result<(), BlueskyError> {
        let parsed_did: atrium_api::types::string::Did = did
            .parse()
            .map_err(|e| BlueskyError::Api(format!("invalid DID: {}", e)))?;

        self.agent
            .api
            .app
            .bsky
            .graph
            .unmute_actor(
                atrium_api::app::bsky::graph::unmute_actor::InputData {
                    actor: parsed_did.into(),
                }
                .into(),
            )
            .await
            .map_err(|e| BlueskyError::Api(format!("failed to unmute user: {}", e)))?;

        debug!(did = %did, "unmuted user");

        Ok(())
    }

    /// Block a user by their DID.
    ///
    /// Blocked users can't see your posts or interact with you.
    /// Returns the URI of the block record.
    pub async fn block(&self, did: &str) -> Result<String, BlueskyError> {
        let did: atrium_api::types::string::Did = did
            .parse()
            .map_err(|e| BlueskyError::Api(format!("invalid DID: {}", e)))?;

        let record_data = atrium_api::app::bsky::graph::block::RecordData {
            created_at: Datetime::now(),
            subject: did.clone(),
        };

        let output = self
            .agent
            .create_record(record_data)
            .await
            .map_err(|e| BlueskyError::Api(format!("failed to block user: {}", e)))?;

        debug!(block_uri = %output.uri, subject = %did.as_str(), "blocked user");

        Ok(output.uri.to_string())
    }

    /// Unblock a previously blocked user.
    ///
    /// Takes the block record URI returned from `block()`.
    pub async fn unblock(&self, block_uri: &str) -> Result<(), BlueskyError> {
        // Parse the AT URI to extract repo and rkey
        // Format: at://did:plc:xxx/app.bsky.graph.block/rkey
        let parts: Vec<&str> = block_uri.split('/').collect();
        if parts.len() < 5 {
            return Err(BlueskyError::Api(format!(
                "invalid block URI: {}",
                block_uri
            )));
        }

        let repo = parts[2]; // did:plc:xxx
        let collection = parts[3]; // app.bsky.graph.block
        let rkey = parts[4]; // rkey

        self.agent
            .api
            .com
            .atproto
            .repo
            .delete_record(
                atrium_api::com::atproto::repo::delete_record::InputData {
                    collection: collection
                        .parse()
                        .map_err(|e| BlueskyError::Api(format!("invalid collection: {}", e)))?,
                    repo: repo
                        .parse()
                        .map_err(|e| BlueskyError::Api(format!("invalid repo: {}", e)))?,
                    rkey: rkey
                        .parse()
                        .map_err(|e| BlueskyError::Api(format!("invalid rkey: {}", e)))?,
                    swap_commit: None,
                    swap_record: None,
                }
                .into(),
            )
            .await
            .map_err(|e| BlueskyError::Api(format!("failed to unblock user: {}", e)))?;

        debug!(block_uri = %block_uri, "unblocked user");

        Ok(())
    }

    /// Mute a thread by its root post URI.
    ///
    /// Muted threads won't generate notifications for new replies.
    pub async fn mute_thread(&self, root_uri: &str) -> Result<(), BlueskyError> {
        self.agent
            .api
            .app
            .bsky
            .graph
            .mute_thread(
                atrium_api::app::bsky::graph::mute_thread::InputData {
                    root: root_uri.to_string(),
                }
                .into(),
            )
            .await
            .map_err(|e| BlueskyError::Api(format!("failed to mute thread: {}", e)))?;

        debug!(root = %root_uri, "muted thread");
        Ok(())
    }

    /// Unmute a previously muted thread.
    pub async fn unmute_thread(&self, root_uri: &str) -> Result<(), BlueskyError> {
        self.agent
            .api
            .app
            .bsky
            .graph
            .unmute_thread(
                atrium_api::app::bsky::graph::unmute_thread::InputData {
                    root: root_uri.to_string(),
                }
                .into(),
            )
            .await
            .map_err(|e| BlueskyError::Api(format!("failed to unmute thread: {}", e)))?;

        debug!(root = %root_uri, "unmuted thread");
        Ok(())
    }

    /// Delete a post by its AT URI.
    ///
    /// This is irreversible. The post will be permanently removed.
    pub async fn delete_post(&self, post_uri: &str) -> Result<(), BlueskyError> {
        // Parse the AT URI to extract repo and rkey
        // Format: at://did:plc:xxx/app.bsky.feed.post/rkey
        let parts: Vec<&str> = post_uri.split('/').collect();
        if parts.len() < 5 {
            return Err(BlueskyError::Api(format!("invalid post URI: {}", post_uri)));
        }

        let repo = parts[2]; // did:plc:xxx
        let collection = parts[3]; // app.bsky.feed.post
        let rkey = parts[4]; // rkey

        self.agent
            .api
            .com
            .atproto
            .repo
            .delete_record(
                atrium_api::com::atproto::repo::delete_record::InputData {
                    collection: collection
                        .parse()
                        .map_err(|e| BlueskyError::Api(format!("invalid collection: {}", e)))?,
                    repo: repo
                        .parse()
                        .map_err(|e| BlueskyError::Api(format!("invalid repo: {}", e)))?,
                    rkey: rkey
                        .parse()
                        .map_err(|e| BlueskyError::Api(format!("invalid rkey: {}", e)))?,
                    swap_commit: None,
                    swap_record: None,
                }
                .into(),
            )
            .await
            .map_err(|e| BlueskyError::Api(format!("failed to delete post: {}", e)))?;

        debug!(post_uri = %post_uri, "deleted post");

        Ok(())
    }

    /// Get a user's profile by DID.
    pub async fn get_profile(&self, did: &str) -> Result<Profile, BlueskyError> {
        let params = atrium_api::app::bsky::actor::get_profile::ParametersData {
            actor: did
                .parse()
                .map_err(|e| BlueskyError::Api(format!("invalid actor: {}", e)))?,
        };

        let output = self
            .agent
            .api
            .app
            .bsky
            .actor
            .get_profile(params.into())
            .await
            .map_err(|e| {
                let error_str = e.to_string();
                if error_str.contains("RateLimitExceeded") || error_str.contains("429") {
                    BlueskyError::RateLimited {
                        endpoint: Some("getProfile".to_string()),
                    }
                } else {
                    BlueskyError::Api(error_str)
                }
            })?;

        debug!(did = %did, handle = %output.handle.as_str(), "fetched profile");

        Ok(Profile {
            did: output.did.to_string(),
            handle: output.handle.to_string(),
            display_name: output.display_name.clone(),
            description: output.description.clone(),
            avatar: output.avatar.clone(),
            banner: output.banner.clone(),
            followers_count: output.followers_count,
            follows_count: output.follows_count,
            posts_count: output.posts_count,
            indexed_at: output.indexed_at.as_ref().map(|t| t.as_str().to_string()),
        })
    }

    /// Get an author's feed (their posts).
    pub async fn get_author_feed(
        &self,
        did: &str,
        limit: Option<u8>,
    ) -> Result<Vec<FeedPost>, BlueskyError> {
        let params = atrium_api::app::bsky::feed::get_author_feed::ParametersData {
            actor: did
                .parse()
                .map_err(|e| BlueskyError::Api(format!("invalid actor: {}", e)))?,
            cursor: None,
            filter: None,
            include_pins: None,
            limit: limit.map(|l| l.clamp(1, 100).try_into().unwrap()),
        };

        let output = self
            .agent
            .api
            .app
            .bsky
            .feed
            .get_author_feed(params.into())
            .await
            .map_err(|e| {
                let error_str = e.to_string();
                if error_str.contains("RateLimitExceeded") || error_str.contains("429") {
                    BlueskyError::RateLimited {
                        endpoint: Some("getAuthorFeed".to_string()),
                    }
                } else {
                    BlueskyError::Api(error_str)
                }
            })?;

        let posts: Vec<FeedPost> = output
            .feed
            .iter()
            .map(|item| {
                let text = self.extract_post_text(&item.post.record);
                let is_reply = item.reply.is_some();
                let is_repost = item.reason.is_some();

                FeedPost {
                    uri: item.post.uri.clone(),
                    cid: item.post.cid.as_ref().to_string(),
                    text,
                    created_at: self.extract_post_created_at(&item.post.record),
                    like_count: item.post.like_count,
                    repost_count: item.post.repost_count,
                    reply_count: item.post.reply_count,
                    is_reply,
                    is_repost,
                }
            })
            .collect();

        debug!(did = %did, count = posts.len(), "fetched author feed");

        Ok(posts)
    }

    /// Get accounts that a user follows.
    pub async fn get_follows(
        &self,
        did: &str,
        limit: Option<u8>,
    ) -> Result<Vec<FollowInfo>, BlueskyError> {
        let params = atrium_api::app::bsky::graph::get_follows::ParametersData {
            actor: did
                .parse()
                .map_err(|e| BlueskyError::Api(format!("invalid actor: {}", e)))?,
            cursor: None,
            limit: limit.map(|l| l.clamp(1, 100).try_into().unwrap()),
        };

        let output = self
            .agent
            .api
            .app
            .bsky
            .graph
            .get_follows(params.into())
            .await
            .map_err(|e| {
                let error_str = e.to_string();
                if error_str.contains("RateLimitExceeded") || error_str.contains("429") {
                    BlueskyError::RateLimited {
                        endpoint: Some("getFollows".to_string()),
                    }
                } else {
                    BlueskyError::Api(error_str)
                }
            })?;

        let follows: Vec<FollowInfo> = output
            .follows
            .iter()
            .map(|f| FollowInfo {
                did: f.did.to_string(),
                handle: f.handle.to_string(),
                display_name: f.display_name.clone(),
                description: f.description.clone(),
                avatar: f.avatar.clone(),
            })
            .collect();

        debug!(did = %did, count = follows.len(), "fetched follows");

        Ok(follows)
    }

    /// Get followers of a user by DID.
    pub async fn get_followers(
        &self,
        did: &str,
        limit: Option<u8>,
    ) -> Result<Vec<FollowInfo>, BlueskyError> {
        let params = atrium_api::app::bsky::graph::get_followers::ParametersData {
            actor: did
                .parse()
                .map_err(|e| BlueskyError::Api(format!("invalid actor: {}", e)))?,
            cursor: None,
            limit: limit.map(|l| l.clamp(1, 100).try_into().unwrap()),
        };

        let output = self
            .agent
            .api
            .app
            .bsky
            .graph
            .get_followers(params.into())
            .await
            .map_err(|e| {
                let error_str = e.to_string();
                if error_str.contains("RateLimitExceeded") || error_str.contains("429") {
                    BlueskyError::RateLimited {
                        endpoint: Some("getFollowers".to_string()),
                    }
                } else {
                    BlueskyError::Api(error_str)
                }
            })?;

        let followers: Vec<FollowInfo> = output
            .followers
            .iter()
            .map(|f| FollowInfo {
                did: f.did.to_string(),
                handle: f.handle.to_string(),
                display_name: f.display_name.clone(),
                description: f.description.clone(),
                avatar: f.avatar.clone(),
            })
            .collect();

        debug!(did = %did, count = followers.len(), "fetched followers");

        Ok(followers)
    }
}

/// Convert atrium facets to our Facet type.
fn convert_atrium_facets(
    facets: &[atrium_api::app::bsky::richtext::facet::Main],
) -> Vec<winter_atproto::Facet> {
    use atrium_api::app::bsky::richtext::facet::MainFeaturesItem;
    use winter_atproto::{ByteSlice, Facet, FacetFeature};

    facets
        .iter()
        .map(|f| {
            let features = f
                .features
                .iter()
                .filter_map(|feature| match feature {
                    atrium_api::types::Union::Refs(MainFeaturesItem::Mention(m)) => {
                        Some(FacetFeature::Mention {
                            did: m.did.to_string(),
                        })
                    }
                    atrium_api::types::Union::Refs(MainFeaturesItem::Link(l)) => {
                        Some(FacetFeature::Link { uri: l.uri.clone() })
                    }
                    atrium_api::types::Union::Refs(MainFeaturesItem::Tag(t)) => {
                        Some(FacetFeature::Tag { tag: t.tag.clone() })
                    }
                    _ => None,
                })
                .collect();

            Facet {
                index: ByteSlice {
                    byte_start: f.index.byte_start as u64,
                    byte_end: f.index.byte_end as u64,
                },
                features,
            }
        })
        .collect()
}

/// Convert our Facet type to atrium facets.
fn convert_winter_facets(
    facets: &[winter_atproto::Facet],
) -> Vec<atrium_api::app::bsky::richtext::facet::Main> {
    use atrium_api::app::bsky::richtext::facet::{
        ByteSliceData, LinkData, MainData, MainFeaturesItem, MentionData, TagData,
    };
    use winter_atproto::FacetFeature;

    facets
        .iter()
        .map(|f| {
            let features: Vec<atrium_api::types::Union<MainFeaturesItem>> = f
                .features
                .iter()
                .map(|feature| match feature {
                    FacetFeature::Mention { did } => {
                        atrium_api::types::Union::Refs(MainFeaturesItem::Mention(Box::new(
                            MentionData {
                                did: did.parse().unwrap_or_else(|_| {
                                    // Fallback to a placeholder DID if parsing fails
                                    "did:plc:invalid".parse().unwrap()
                                }),
                            }
                            .into(),
                        )))
                    }
                    FacetFeature::Link { uri } => {
                        tracing::debug!(link_uri = %uri, "Converting link facet");
                        atrium_api::types::Union::Refs(MainFeaturesItem::Link(Box::new(
                            LinkData { uri: uri.clone() }.into(),
                        )))
                    }
                    FacetFeature::Tag { tag } => atrium_api::types::Union::Refs(
                        MainFeaturesItem::Tag(Box::new(TagData { tag: tag.clone() }.into())),
                    ),
                })
                .collect();

            MainData {
                index: ByteSliceData {
                    byte_start: f.index.byte_start as usize,
                    byte_end: f.index.byte_end as usize,
                }
                .into(),
                features,
            }
            .into()
        })
        .collect()
}

/// Helper struct for deserializing post records from notifications.
#[derive(Debug, serde::Deserialize)]
struct DeserPostRecord {
    text: String,
    reply: Option<DeserReplyRef>,
    #[serde(default)]
    facets: Vec<winter_atproto::Facet>,
}

#[derive(Debug, serde::Deserialize)]
struct DeserReplyRef {
    parent: DeserStrongRef,
    root: DeserStrongRef,
}

#[derive(Debug, serde::Deserialize)]
struct DeserStrongRef {
    uri: String,
    cid: String,
}
