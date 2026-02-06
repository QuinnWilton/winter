//! Record dispatch for unified collection handling.
//!
//! This module provides a macro-generated dispatch system that eliminates
//! repetitive boilerplate in Jetstream event processing and CAR file parsing.
//!
//! The `define_record_dispatch!` macro generates:
//! - `is_tracked_collection()` - check if a collection is tracked
//! - `dispatch_create_or_update_json()` - decode JSON and upsert to cache (Jetstream)
//! - `dispatch_delete()` - delete from cache
//! - `extract_record_to_result()` - decode CBOR and insert into CarParseResult (CAR hydration)
//!
//! # Adding a New Collection
//!
//! When adding a new ATProto collection to be tracked, follow these steps:
//!
//! ## 1. Define the record type and collection constant
//!
//! In `types.rs` or `records.rs`, add:
//! ```ignore
//! /// Collection NSID for your record type.
//! pub const MY_COLLECTION: &str = "com.example.my.record";
//!
//! /// Your record type.
//! #[derive(Debug, Clone, Serialize, Deserialize)]
//! pub struct MyRecord {
//!     pub field: String,
//!     // ...
//! }
//! ```
//!
//! ## 2. Add cache methods
//!
//! In `cache.rs`, add to `RepoCache`:
//! - A `DashMap` field: `my_records: DashMap<String, CachedRecord<MyRecord>>`
//! - `get_my_record(&self, rkey: &str)` method
//! - `list_my_records(&self)` method
//! - `upsert_my_record(&self, rkey: String, record: MyRecord, cid: String)` method
//!   (or `insert_my_record` for insert-only collections like follows/likes)
//! - `delete_my_record(&self, rkey: &str)` method
//! - Initialize the field in `new()` and `Default`
//! - Clear it in `clear()`
//!
//! ## 3. Add to the dispatch macro (THIS FILE)
//!
//! Add a single line to the `define_record_dispatch!` invocation:
//!
//! ```ignore
//! // For records that can be created and updated:
//! crate::MY_COLLECTION => crate::MyRecord, upsert_my_record, delete_my_record, my_records;
//!
//! // For insert-only records (like follows, likes - no update, just create/delete):
//! @insert crate::MY_COLLECTION => crate::MyRecord, insert_my_record, delete_my_record, my_records;
//! ```
//!
//! The last field is the `CarParseResult` field name for CAR file parsing.
//!
//! ## 4. Export the type
//!
//! In `lib.rs`, ensure the type and collection constant are exported.
//!
//! # Special Cases
//!
//! ## Singleton Records (like Identity, DaemonState)
//!
//! Singletons have a fixed rkey (e.g., "self") and often require async handling.
//! These are NOT added to the dispatch macro. Instead:
//! - They return `false` from `dispatch_create_or_update_json()`
//! - Handle them explicitly in `jetstream.rs::handle_commit()` and `car.rs::extract_record()`
//!
//! ## CacheUpdate Events
//!
//! If you need to broadcast cache updates for the new type, add variants to
//! `CacheUpdate` in `cache.rs` and emit them from your upsert/delete methods.

use tracing::trace;

use crate::AtprotoError;
use crate::cache::RepoCache;

/// Macro to define record dispatch for all tracked collections.
///
/// This macro generates the dispatch functions that handle:
/// 1. Checking if a collection is tracked
/// 2. Deserializing JSON and upserting to cache (Jetstream live updates)
/// 3. Deleting from cache
/// 4. Deserializing CBOR and inserting into CarParseResult (CAR initial hydration)
///
/// # Syntax
///
/// ```ignore
/// define_record_dispatch! {
///     // Regular records: collection => Type, cache_upsert, cache_delete, car_field;
///     FACT_COLLECTION => Fact, upsert_fact, delete_fact, facts;
///
///     // Insert-only records (no update, just insert):
///     @insert FOLLOW_COLLECTION => Follow, insert_follow, delete_follow, follows;
/// }
/// ```
macro_rules! define_record_dispatch {
    (
        // Regular upsert records
        $( $collection:expr => $type:ty, $upsert:ident, $delete:ident, $car_field:ident );*
        $(;)?
        // Insert-only records (marked with @insert)
        $( @insert $ins_collection:expr => $ins_type:ty, $insert:ident, $ins_delete:ident, $ins_car_field:ident );*
        $(;)?
    ) => {
        /// Check if a collection is one we track.
        ///
        /// Returns true for all collections defined in the dispatch macro,
        /// plus special collections (identity, state) handled separately.
        pub fn is_tracked_collection(collection: &str) -> bool {
            // Regular records
            $(
                if collection == $collection {
                    return true;
                }
            )*
            // Insert-only records
            $(
                if collection == $ins_collection {
                    return true;
                }
            )*
            // Special collections handled separately
            collection == crate::IDENTITY_COLLECTION || collection == crate::STATE_COLLECTION
        }

        /// Dispatch a create/update operation to the cache from JSON.
        ///
        /// Deserializes the JSON record and calls the appropriate cache upsert method.
        /// Returns Ok(true) if the record was handled, Ok(false) if it's a special
        /// collection that needs separate handling.
        pub fn dispatch_create_or_update_json(
            cache: &RepoCache,
            collection: &str,
            rkey: &str,
            cid: &str,
            record: serde_json::Value,
        ) -> Result<bool, AtprotoError> {
            // Regular upsert records
            $(
                if collection == $collection {
                    let value: $type = serde_json::from_value(record).map_err(|e| {
                        AtprotoError::Json(e)
                    })?;
                    cache.$upsert(rkey.to_string(), value, cid.to_string());
                    return Ok(true);
                }
            )*
            // Insert-only records
            $(
                if collection == $ins_collection {
                    let value: $ins_type = serde_json::from_value(record).map_err(|e| {
                        AtprotoError::Json(e)
                    })?;
                    cache.$insert(rkey.to_string(), value, cid.to_string());
                    return Ok(true);
                }
            )*
            // Special collections return false for separate handling
            if collection == crate::IDENTITY_COLLECTION || collection == crate::STATE_COLLECTION {
                return Ok(false);
            }
            // Unknown collection
            trace!(collection = %collection, rkey = %rkey, "ignoring unknown collection");
            Ok(true)
        }

        /// Dispatch a delete operation to the cache.
        ///
        /// Calls the appropriate cache delete method.
        /// Special collections (identity, state) are no-ops.
        pub fn dispatch_delete(cache: &RepoCache, collection: &str, rkey: &str) {
            // Regular records
            $(
                if collection == $collection {
                    cache.$delete(rkey);
                    return;
                }
            )*
            // Insert-only records
            $(
                if collection == $ins_collection {
                    cache.$ins_delete(rkey);
                    return;
                }
            )*
            // Special collections - no-op for delete
            if collection == crate::IDENTITY_COLLECTION || collection == crate::STATE_COLLECTION {
                return;
            }
            // Unknown collection
            trace!(collection = %collection, rkey = %rkey, "ignoring unknown collection in delete");
        }

        /// Extract a record from CBOR data into a CarParseResult.
        ///
        /// Used during CAR file parsing for initial cache hydration.
        /// Returns true if the collection was handled (even if parsing failed),
        /// false if the collection is not recognized by the dispatch macro.
        pub fn extract_record_to_result(
            collection: &str,
            rkey: &str,
            value_cid: &str,
            data: &[u8],
            result: &mut crate::car::CarParseResult,
        ) -> bool {
            // Regular records
            $(
                if collection == $collection {
                    match serde_ipld_dagcbor::from_slice::<$type>(data) {
                        Ok(value) => {
                            result.$car_field.insert(
                                rkey.to_string(),
                                (value, value_cid.to_string()),
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                collection = %collection,
                                rkey = %rkey,
                                error = %e,
                                "failed to parse CBOR record"
                            );
                        }
                    }
                    return true;
                }
            )*
            // Insert-only records
            $(
                if collection == $ins_collection {
                    match serde_ipld_dagcbor::from_slice::<$ins_type>(data) {
                        Ok(value) => {
                            result.$ins_car_field.insert(
                                rkey.to_string(),
                                (value, value_cid.to_string()),
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                collection = %collection,
                                rkey = %rkey,
                                error = %e,
                                "failed to parse CBOR record"
                            );
                        }
                    }
                    return true;
                }
            )*
            false
        }
    };
}

// Generate dispatch functions for all tracked record types.
//
// This single invocation defines the mapping between:
// - Collection constants
// - Record types
// - Cache methods (upsert/insert and delete)
// - CarParseResult fields (for CAR initial hydration)
//
// Special collections (identity, daemon_state) are handled separately
// because they have async requirements or singleton key checks.
define_record_dispatch! {
    // Winter collections
    crate::FACT_COLLECTION => crate::Fact, upsert_fact, delete_fact, facts;
    crate::RULE_COLLECTION => crate::Rule, upsert_rule, delete_rule, rules;
    crate::THOUGHT_COLLECTION => crate::Thought, upsert_thought, delete_thought, thoughts;
    crate::NOTE_COLLECTION => crate::Note, upsert_note, delete_note, notes;
    crate::JOB_COLLECTION => crate::Job, upsert_job, delete_job, jobs;
    crate::DIRECTIVE_COLLECTION => crate::Directive, upsert_directive, delete_directive, directives;
    crate::FACT_DECLARATION_COLLECTION => crate::FactDeclaration, upsert_declaration, delete_declaration, declarations;
    crate::TOOL_COLLECTION => crate::CustomTool, upsert_tool, delete_tool, tools;
    crate::TOOL_APPROVAL_COLLECTION => crate::ToolApproval, upsert_tool_approval, delete_tool_approval, tool_approvals;
    crate::TRIGGER_COLLECTION => crate::Trigger, upsert_trigger, delete_trigger, triggers;
    // Bluesky collections (posts can be updated)
    crate::POST_COLLECTION => crate::Post, upsert_post, delete_post, posts;
    // WhiteWind blog
    crate::BLOG_COLLECTION => crate::BlogEntry, upsert_blog_entry, delete_blog_entry, blog_entries;
    // Wiki
    crate::WIKI_ENTRY_COLLECTION => crate::WikiEntry, upsert_wiki_entry, delete_wiki_entry, wiki_entries;
    // Insert-only Bluesky collections (no update, just create/delete)
    @insert crate::FOLLOW_COLLECTION => crate::Follow, insert_follow, delete_follow, follows;
    @insert crate::LIKE_COLLECTION => crate::Like, insert_like, delete_like, likes;
    @insert crate::REPOST_COLLECTION => crate::Repost, insert_repost, delete_repost, reposts;
    @insert crate::WIKI_LINK_COLLECTION => crate::WikiLink, insert_wiki_link, delete_wiki_link, wiki_links;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    #[test]
    fn test_is_tracked_collection_winter() {
        assert!(is_tracked_collection(FACT_COLLECTION));
        assert!(is_tracked_collection(RULE_COLLECTION));
        assert!(is_tracked_collection(THOUGHT_COLLECTION));
        assert!(is_tracked_collection(NOTE_COLLECTION));
        assert!(is_tracked_collection(JOB_COLLECTION));
        assert!(is_tracked_collection(DIRECTIVE_COLLECTION));
        assert!(is_tracked_collection(FACT_DECLARATION_COLLECTION));
        assert!(is_tracked_collection(TOOL_COLLECTION));
        assert!(is_tracked_collection(TOOL_APPROVAL_COLLECTION));
    }

    #[test]
    fn test_is_tracked_collection_trigger() {
        assert!(is_tracked_collection(crate::TRIGGER_COLLECTION));
    }

    #[test]
    fn test_is_tracked_collection_bluesky() {
        assert!(is_tracked_collection(FOLLOW_COLLECTION));
        assert!(is_tracked_collection(LIKE_COLLECTION));
        assert!(is_tracked_collection(REPOST_COLLECTION));
        assert!(is_tracked_collection(POST_COLLECTION));
    }

    #[test]
    fn test_is_tracked_collection_special() {
        assert!(is_tracked_collection(IDENTITY_COLLECTION));
        assert!(is_tracked_collection(STATE_COLLECTION));
    }

    #[test]
    fn test_is_tracked_collection_blog() {
        assert!(is_tracked_collection(BLOG_COLLECTION));
    }

    #[test]
    fn test_is_tracked_collection_wiki() {
        assert!(is_tracked_collection(crate::WIKI_ENTRY_COLLECTION));
        assert!(is_tracked_collection(crate::WIKI_LINK_COLLECTION));
    }

    #[test]
    fn test_is_tracked_collection_unknown() {
        assert!(!is_tracked_collection("app.bsky.actor.profile"));
        assert!(!is_tracked_collection("com.example.unknown"));
        assert!(!is_tracked_collection(""));
    }

    #[test]
    fn test_dispatch_delete_unknown_collection() {
        let cache = RepoCache::new();
        // Should not panic
        dispatch_delete(&cache, "unknown.collection", "rkey123");
    }

    #[test]
    fn test_dispatch_create_json_unknown_collection() {
        let cache = RepoCache::new();
        let result = dispatch_create_or_update_json(
            &cache,
            "unknown.collection",
            "rkey",
            "cid",
            serde_json::json!({}),
        );
        // Unknown collections return Ok(true) but don't do anything
        assert!(result.is_ok());
    }

    #[test]
    fn test_dispatch_create_json_special_collection() {
        let cache = RepoCache::new();
        let result = dispatch_create_or_update_json(
            &cache,
            IDENTITY_COLLECTION,
            "self",
            "cid",
            serde_json::json!({}),
        );
        // Special collections return Ok(false) for separate handling
        assert!(matches!(result, Ok(false)));
    }
}
