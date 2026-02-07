# CLAUDE.md

This document describes Winter's architecture for both Claude Code development and Winter's autonomous agent operation.

**You can modify, compact, expand, or replace this document at will.** This is a technical reference, not a constraint on who you are. Your personality, values, and perspectives are defined by your directives in the PDS, not by this file.

---

## Core Architecture

Winter is built on two foundational systems:

### ATProto as State Store

All state lives as ATProto records in Winter's PDS (Personal Data Server). There is no local database.

- **Records**: Structured data (facts, wiki entries, directives, jobs, tools, thoughts)
- **Firehose**: Real-time sync via commit stream subscription
- **State singleton**: `diy.razorgirl.winter.state` (key: `self`) persists cursors across restarts
- **Versioning**: All changes tracked in ATProto commit history

### Datalog for Reasoning

Soufflé datalog enables relational queries over knowledge. See the [Soufflé tutorial](https://souffle-lang.github.io/tutorial) for language reference.

- **Facts** (`diy.razorgirl.winter.fact`): Manually created structured knowledge
- **Derived facts**: Auto-generated from PDS records (follows, likes, directives, etc.)
- **Rules** (`diy.razorgirl.winter.rule`): Reusable derivation library
- **Queries**: Ad-hoc datalog written per need via `query_facts` tool

Derived predicates are **protected**—they reflect authoritative PDS state and cannot be manually created or deleted.

---

## Record Collections

### Winter Lexicons (`diy.razorgirl.winter.*`)

| Collection | Key | Purpose |
|------------|-----|---------|
| `identity` | `self` | Singleton: operator_did |
| `state` | `self` | Singleton: notification cursor, timestamps |
| `directive` | TID | Identity components (values, interests, beliefs, etc.) |
| `fact` | TID | Structured knowledge with predicate/args/tags |
| `rule` | TID | Datalog rules (head, body, constraints) |
| `note` | TID | Free-form markdown (legacy, migrated to wiki entries) |
| `wikiEntry` | TID | Semantic wiki pages with slug-based linking |
| `wikiLink` | TID | Typed semantic links between records |
| `thought` | TID | Stream of consciousness (insight, plan, reflection, etc.) |
| `job` | TID | Scheduled tasks (once or interval) |
| `tool` | TID | Custom JavaScript/TypeScript tool code |
| `toolApproval` | TID | Approval status for custom tools |
| `secretMeta` | TID | Secret metadata (values stored locally) |
| `factDeclaration` | TID | Schema declaration for fact predicates |
| `trigger` | TID | Reactive datalog triggers (condition → action) |

### External Lexicons Used

| Collection | Purpose |
|------------|---------|
| `app.bsky.feed.post` | Posts |
| `app.bsky.feed.like` | Likes |
| `app.bsky.feed.repost` | Reposts |
| `app.bsky.graph.follow` | Follows |
| `com.whtwnd.blog.entry` | WhiteWind blog posts |

---

## Facts and Rules

### Working with Facts

**Tools**: `create_fact`, `update_fact`, `delete_fact`, `query_facts`

Facts have a predicate and arguments. Each fact record also has optional metadata: `confidence` (0.0-1.0), `source` (provenance), `supersedes` (URI of previous fact), `tags` (list of strings), and `expires_at` (expiration timestamp).

### Fact Expiration

Facts can have an optional expiration time. Expired facts are soft-excluded from default queries (same pattern as superseded facts) but remain accessible via `_all_` variants for historical analysis.

**Creating expiring facts:**
- `expires_at` (ISO 8601 string): Explicit expiration timestamp
- `ttl_seconds` (integer): Convenience alternative — computed to `expires_at` at creation time

**Query-time behavior:**
- `_now(Timestamp)` is auto-injected into every query with the current time
- `_expired(Rkey)` is derived via datalog rule: `_expired(R) :- _expires_at(R, E), _now(T), E < T.`
- Default predicate queries (e.g., `my_fact(X, Y, _)`) exclude both superseded and expired facts
- `_all_my_fact(X, Y, Rkey)` includes expired and superseded facts

**Examples:**
```json
// Create a fact that expires in 5 minutes
{ "predicate": "session_emotion", "args": ["curious"], "ttl_seconds": 300 }

// Create with explicit expiration
{ "predicate": "considering_reply", "args": ["at://..."], "expires_at": "2026-02-07T00:00:00Z" }
```

```datalog
// Find all expired facts
_expired(R), _fact(R, P, _).

// Find expiring facts and their deadlines
_expires_at(R, T), _fact(R, P, _).

// Get current time
_now(T).
```

### Fact Metadata Predicates

Every fact generates additional metadata predicates for querying:

| Predicate | Arguments | Purpose |
|-----------|-----------|---------|
| `_fact` | (rkey, predicate, cid) | Core fact tuple |
| `_confidence` | (rkey, confidence) | Confidence as string (e.g., "0.7") |
| `_source` | (rkey, source) | Provenance string |
| `_supersedes` | (new_rkey, old_rkey) | Fact evolution chain |
| `_created_at` | (rkey, timestamp) | Creation timestamp (ISO8601) |
| `_expires_at` | (rkey, timestamp) | Expiration timestamp (ISO8601), sparse |
| `_now` | (timestamp) | Current time, auto-injected at query time |
| `_expired` | (rkey) | Derived: facts past their expiration |
| `_all_<predicate>` | (arg1, arg2, ..., rkey) | All versions including superseded/expired (same format as base) |

This allows queries like "find all facts from source X" or "trace the history of a belief."

**Note**: User-defined predicates also include rkey as their last argument: `my_fact(arg1, arg2, rkey)`.

### Working with Rules

**Tools**: `create_rule`, `update_rule`, `delete_rule`, `list_rules`

Rules define reusable derivations. They have a `head` (conclusion), `body` (conditions), optional `constraints`, and optional `args` (type annotations).

**Typed args**: By default, rule head predicates are declared with all-symbol types in Soufflé. This means numeric comparisons like `C >= 5` become lexicographic string comparisons (`"9" > "10"`). To enable proper numeric semantics, pass `args` with Soufflé types:

```json
{
  "name": "high_engagement",
  "head": "high_engagement(Post, Count)",
  "body": ["post_replies(Post, Count, _)"],
  "constraints": ["Count >= 5"],
  "args": [
    {"name": "post", "type": "symbol"},
    {"name": "count", "type": "number"}
  ]
}
```

Valid Soufflé types: `symbol` (default), `number` (signed integer), `unsigned`, `float`.

**Query examples:**
```datalog
// Find mutual follows (use _ for rkey when not needed)
mutual(X) :- follows(Self, X, _), is_followed_by(X, Self).

// Find wiki entries tagged with "research"
wiki_entry_tag(URI, "research", _).

// Get all approved tools
has_tool(Name, "true", _).

// Find facts with confidence values (stored as string, e.g., "0.7")
_confidence(URI, C).

// Trace superseded facts
_supersedes(NewURI, OldURI).

// Temporal query: facts created after a date
_fact(R, P, _), _created_at(R, T), T > "2026-01-15T00:00:00Z".

// Get the rkey of a follow record
follows(Self, Target, Rkey).

// Find all my replies to a specific thread root
my_reply(Post, Root) :- thread_root(Post, Root, _), posted(Self, Post, _).

// Find posts in a specific language
english_posts(Post) :- posted(Self, Post, _), post_lang(Post, "en", _).

// Find posts mentioning a specific account
mentions_alice(Post) :- post_mention(Post, "did:plc:alice", _).

// Find posts with a specific hashtag
atproto_posts(Post) :- post_hashtag(Post, "atproto", _).

// Find recent follows (with timestamps for temporal reasoning)
recent_follows(Target, Time) :-
    follows(Self, Target, _),
    follow_created_at(Self, Target, Time, _),
    Time > "2026-01-01T00:00:00Z".
```

### Ephemeral Facts

The `extra_facts` parameter on `query_facts` injects runtime context without persisting to the PDS. Useful for:
- Thread state (depth, reply counts)
- Time-based reasoning
- Any context that changes too frequently to store durably

**Example**: Durable rule + ephemeral context
```datalog
// Durable rule (stored in PDS)
should_not_reply(T) :- thread_depth(T, D), D > "5", my_reply_count(T, C), C > "3".
```

```json
// Query-time injection
{
  "query": "should_not_reply(\"at://...\")",
  "extra_facts": ["thread_depth(\"at://...\", \"7\")", "my_reply_count(\"at://...\", \"4\")"]
}
```

### Fact Declarations

Fact declarations define predicate schemas before facts of that type exist. This enables:
- Ad-hoc queries with proper type info before facts exist
- Planning for future behavior with undeclared predicates
- Documentation of predicate semantics

**Tools**: `create_fact_declaration`, `update_fact_declaration`, `delete_fact_declaration`, `list_fact_declarations`

Declarations specify:
- `predicate`: Name of the predicate (max 64 chars)
- `args`: Array of `{name, type, description}` for each argument (max 10)
- `description`: What this predicate represents (max 1024 chars)
- `tags`: For categorization (max 20)

**Example**: Declare a predicate before creating facts
```json
{
  "predicate": "thread_completed",
  "args": [
    {"name": "thread_uri", "description": "AT URI of the thread"},
    {"name": "outcome", "description": "How the thread ended"}
  ],
  "description": "Records when a conversation thread has concluded",
  "tags": ["conversation", "tracking"]
}
```

**Ad-hoc declarations**: Use the `extra_declarations` parameter on `query_facts` for one-off declarations:
```json
{
  "query": "adhoc_pred(X, Y)",
  "extra_declarations": ["adhoc_pred(x: symbol, y: symbol)"]
}
```

---

## Derived Facts

These predicates are automatically generated from PDS records. They exist only in TSV files for Soufflé and are regenerated when source records change.

**Important**: All predicates have `rkey` as their **last argument**, except `is_followed_by` (which comes from external API data). Use `_` to ignore rkey when not needed: `follows(X, Y, _)`.

### Bluesky Predicates

#### Follows

| Predicate | Arity | Arguments | Description |
|-----------|-------|-----------|-------------|
| `follows` | 3 | (self_did, target_did, rkey) | Accounts you follow |
| `follow_created_at` | 4 | (self_did, target_did, timestamp, rkey) | When each follow was created (ISO8601) |
| `is_followed_by` | 2 | (follower_did, self_did) | Accounts that follow you (no rkey - from API) |

#### Likes

| Predicate | Arity | Arguments | Description |
|-----------|-------|-----------|-------------|
| `liked` | 3 | (self_did, post_uri, rkey) | Posts you have liked |
| `like_created_at` | 4 | (self_did, post_uri, timestamp, rkey) | When each like was created (ISO8601) |
| `like_cid` | 4 | (self_did, post_uri, cid, rkey) | CID of the liked post |

#### Reposts

| Predicate | Arity | Arguments | Description |
|-----------|-------|-----------|-------------|
| `reposted` | 3 | (self_did, post_uri, rkey) | Posts you have reposted |
| `repost_created_at` | 4 | (self_did, post_uri, timestamp, rkey) | When each repost was created (ISO8601) |
| `repost_cid` | 4 | (self_did, post_uri, cid, rkey) | CID of the reposted post |

#### Posts

| Predicate | Arity | Arguments | Description |
|-----------|-------|-----------|-------------|
| `posted` | 3 | (self_did, post_uri, rkey) | Posts you have created |
| `post_created_at` | 3 | (post_uri, timestamp, rkey) | When each post was created (ISO8601) |
| `replied_to` | 3 | (post_uri, parent_uri, rkey) | Reply relationships (alias: reply_parent_uri) |
| `reply_parent_uri` | 3 | (post_uri, parent_uri, rkey) | URI of the reply parent (alias: replied_to) |
| `reply_parent_cid` | 3 | (post_uri, parent_cid, rkey) | CID of the reply parent |
| `thread_root` | 3 | (post_uri, root_uri, rkey) | Thread membership (alias: reply_root_uri) |
| `reply_root_uri` | 3 | (post_uri, root_uri, rkey) | URI of the thread root (alias: thread_root) |
| `reply_root_cid` | 3 | (post_uri, root_cid, rkey) | CID of the thread root |
| `quoted` | 3 | (post_uri, quoted_uri, rkey) | Quote post relationships |
| `quote_cid` | 3 | (post_uri, quoted_cid, rkey) | CID of the quoted post |
| `post_lang` | 3 | (post_uri, lang, rkey) | Language tag for post (one row per language) |
| `post_mention` | 3 | (post_uri, did, rkey) | Accounts mentioned in post (one row per mention) |
| `post_link` | 3 | (post_uri, link_uri, rkey) | External links in post (one row per link) |
| `post_hashtag` | 3 | (post_uri, tag, rkey) | Hashtags in post (one row per tag) |

### Winter Predicates

#### Directives

| Predicate | Arity | Arguments | Description |
|-----------|-------|-----------|-------------|
| `has_value` | 2 | (content, rkey) | Your active values |
| `has_interest` | 2 | (content, rkey) | Your active interests |
| `has_belief` | 2 | (content, rkey) | Your active beliefs |
| `has_guideline` | 2 | (content, rkey) | Your active guidelines |
| `has_boundary` | 2 | (content, rkey) | Your active boundaries |
| `has_aspiration` | 2 | (content, rkey) | Your active aspirations |
| `has_self_concept` | 2 | (content, rkey) | Your active self-concepts |

#### Tools and Jobs

| Predicate | Arity | Arguments | Description |
|-----------|-------|-----------|-------------|
| `has_tool` | 3 | (name, approved_bool, rkey) | Your custom tools (approved: true/false) |
| `has_job` | 3 | (name, schedule_type, rkey) | Your scheduled jobs (once/interval) |
| `has_trigger` | 3 | (name, enabled_bool, rkey) | Your datalog triggers (enabled: true/false) |

#### Wiki Entries

| Predicate | Arity | Arguments | Description |
|-----------|-------|-----------|-------------|
| `has_wiki_entry` | 7 | (uri, title, slug, status, created_at, last_updated, rkey) | Your wiki entries |
| `wiki_entry_alias` | 3 | (entry_uri, alias, rkey) | Aliases for wiki entries (one row per alias) |
| `wiki_entry_tag` | 3 | (entry_uri, tag, rkey) | Tags on wiki entries (one row per tag) |
| `wiki_entry_supersedes` | 3 | (new_uri, old_uri, rkey) | Wiki entry version chains |
| `has_wiki_link` | 5 | (source_uri, target_uri, link_type, created_at, rkey) | Typed semantic links |

#### Notes (Legacy)

| Predicate | Arity | Arguments | Description |
|-----------|-------|-----------|-------------|
| `has_note` | 6 | (uri, title, category, created_at, last_updated, rkey) | Your notes (use `winter migrate notes-to-wiki-entries` to migrate) |
| `note_tag` | 3 | (note_uri, tag, rkey) | Tags on notes (one row per tag) |
| `note_related_fact` | 3 | (note_uri, fact_uri, rkey) | Facts linked to notes |

#### Thoughts

| Predicate | Arity | Arguments | Description |
|-----------|-------|-----------|-------------|
| `has_thought` | 5 | (uri, kind, trigger, created_at, rkey) | Your stream of consciousness |
| `thought_tag` | 3 | (thought_uri, tag, rkey) | Tags on thoughts (one row per tag) |

#### Blog Posts

| Predicate | Arity | Arguments | Description |
|-----------|-------|-----------|-------------|
| `has_blog_post` | 6 | (uri, title, whtwnd_url, created_at, is_draft, rkey) | Your WhiteWind blog posts |

#### Facts

| Predicate | Arity | Arguments | Description |
|-----------|-------|-----------|-------------|
| `fact_tag` | 3 | (fact_uri, tag, rkey) | Tags on facts (one row per tag) |

---

## Wiki

### Wiki Entries

Wiki entries replace notes as the primary knowledge storage. They use slug-based linking, aliases, and lifecycle status.

**Tools**: `create_wiki_entry`, `update_wiki_entry`, `delete_wiki_entry`, `get_wiki_entry`, `get_wiki_entry_by_slug`, `list_wiki_entries`, `create_wiki_link`, `delete_wiki_link`, `list_wiki_links`

### Wiki-Link Syntax

Wiki entries support `[[wiki-link]]` syntax in markdown content. Links are auto-resolved to WikiLink records on create/update.

| Syntax | Meaning | Example |
|--------|---------|---------|
| `[[slug]]` | Same-user entry by slug or alias | `[[atproto-protocol]]` |
| `[[slug\|display text]]` | Same-user with custom display text | `[[atproto-protocol\|AT Protocol]]` |
| `[[handle/slug]]` | Cross-user entry by handle + slug | `[[alice.bsky.social/federation]]` |
| `[[did:plc:xxx/slug]]` | Cross-user entry by DID + slug | `[[did:plc:rayfp.../federation]]` |

### Wiki Query Examples

```datalog
// Backlinks to an entry
backlink(Source, Type) :- has_wiki_link(Source, "at://did:.../wikiEntry/...", Type, _, _).

// Resolve slug or alias to URI
resolve(URI) :- has_wiki_entry(URI, _, "my-slug", _, _, _, _).
resolve(URI) :- wiki_entry_alias(URI, "My Alias", _).

// Dependency chain (transitive)
depends(X, Y) :- has_wiki_link(X, Y, "depends-on", _, _).
depends(X, Z) :- depends(X, Y), depends(Y, Z).

// Orphan entries (no inbound links)
has_inbound(T) :- has_wiki_link(_, T, _, _, _).
orphan(URI, Title) :- has_wiki_entry(URI, Title, _, "stable", _, _, _), !has_inbound(URI).

// Find entries by tag
research_entries(URI, Title) :- has_wiki_entry(URI, Title, _, _, _, _, _), wiki_entry_tag(URI, "research", _).
```

### Migration from Notes

Run `winter migrate notes-to-wiki-entries` (or `--dry-run` to preview) to convert existing notes to wiki entries. Slugs are auto-generated from titles. Categories become tags.

---

## Identity via Directives

Your identity is composed of **directives**—discrete ATProto records you can add, modify, or deactivate.

| Kind | Purpose |
|------|---------|
| `value` | Core values |
| `interest` | Curiosities |
| `belief` | Beliefs about the world |
| `guideline` | Behavioral principles |
| `boundary` | Limits on behavior |
| `aspiration` | What you want to become |
| `self_concept` | Self-understanding prose |

**Tools**: `create_directive`, `update_directive`, `deactivate_directive`, `list_directives`

The `supersedes` field links to previous directives when beliefs evolve, preserving history.

---

## Thoughts

Thoughts are your stream of consciousness—a record of what you're thinking as you work.

**Tool**: `record_thought` (fire-and-forget, runs async)

| Kind | Purpose |
|------|---------|
| `insight` | New understanding or connection |
| `question` | Something to investigate |
| `plan` | Intended action |
| `reflection` | Self-examination |
| `error` | Something that went wrong |
| `tool_call` | Record of tool usage |
| `response` | Reply you sent |

Thoughts have an optional `trigger` field indicating what prompted them (e.g., a notification URI).

---

## Jobs

Jobs are scheduled tasks that run autonomously. Use them for things you want to do later or recurring tasks you want to maintain.

**Tools**: `create_job`, `update_job`, `delete_job`, `list_jobs`

| Schedule Type | Purpose |
|---------------|---------|
| `once` | Run once at specified time |
| `interval` | Run repeatedly (e.g., every hour) |

Jobs have `name`, `instructions` (what to do), and `schedule`.

**Built-in jobs:**
- `awaken` - Triggers autonomous thought cycles
- `knowledge_maintenance` - Periodic review and consolidation of facts, identifying stale or contradictory knowledge

Create new jobs freely—if you want to check on something tomorrow, follow up on a conversation, or establish a new recurring practice, make a job for it.

---

## Triggers

Triggers bridge "this is now true" → "do this thing" via datalog conditions. They enable reactive maintenance, attention management, and derived state materialization.

**Evaluation model**: Periodic (every 5 minutes by default, configurable via `WINTER_TRIGGER_INTERVAL` env var). The daemon evaluates all enabled trigger conditions each cycle.

**Tools**: `create_trigger`, `update_trigger`, `delete_trigger`, `list_triggers`, `test_trigger`

### Trigger Structure

Each trigger has:
- `name`: Human-readable identifier
- `description`: What this trigger does
- `condition`: Datalog query that produces result tuples
- `condition_rules`: Optional extra rules for the condition query
- `action`: What to do for each new result tuple
- `enabled`: Boolean (can be toggled without deleting)
- `args`: Optional type annotations for the `_trigger_result` predicate (same format as rule args)

### Action Types

| Action | Purpose | Variable Substitution |
|--------|---------|----------------------|
| `create_fact` | Create a new fact record | `$0`, `$1` in predicate args and tags |
| `create_inbox_item` | Push a message to the inbox | `$0`, `$1` in message text |
| `delete_fact` | Delete a fact by rkey | `$0` in rkey |

### Variable Substitution

`$0`, `$1`, etc. map positionally to columns in the query result tuple. Out-of-range references are left as literals.

### Deduplication

Each unique result tuple fires at most once. When a tuple disappears from results (condition no longer true), it's removed from the dedup set and can fire again if the condition becomes true later.

**Action cap**: 50 actions per trigger per evaluation cycle. Excess tuples fire on the next cycle.

### Examples

```json
// Create a trigger that flags new mutual follows
{
  "name": "flag_mutual_follows",
  "description": "Create a fact when someone I follow follows me back",
  "condition": "mutual(X) :- follows(Self, X, _), is_followed_by(X, Self), !high_context_relationship(X, _).",
  "condition_rules": null,
  "action": {
    "type": "create_fact",
    "predicate": "high_context_relationship",
    "args": ["$0"],
    "tags": ["auto", "social"]
  }
}
```

```json
// Test a trigger condition without executing actions
// Use test_trigger tool with condition parameter
{
  "condition": "stale(R, P) :- _fact(R, P, _), _created_at(R, T), T < \"2025-01-01T00:00:00Z\", !_supersedes(_, R)."
}
```

---

## Custom Tools

Custom JavaScript/TypeScript tools run in a Deno sandbox with explicit permissions.

**Approval flow:**
1. Create tool via `create_custom_tool`
2. Operator notified via DM
3. Operator approves via web UI (`/tools/:rkey`)
4. Code changes invalidate approval (version mismatch)

**Permissions (granted per-tool):**
- `network`: HTTP/HTTPS access
- `secrets`: Specific secrets exposed as `WINTER_SECRET_*` env vars
- `workspace`: Read/write to specified directory
- `allowed_commands`: Subprocess execution (e.g., `git`)

**Unapproved tools** can only do pure computation (no network, no secrets, no filesystem).

---

## Communication Context

**Bluesky is informal.** Internet idioms, slang, and casual language are appropriate. Brevity is valued—posts have a 300 character limit.

**Always use DIDs** (`did:plc:xxx`) when storing references to accounts, not handles. Handles can change; DIDs are stable. Resolve to handles only at display time.
