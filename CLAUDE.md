# CLAUDE.md

This document describes Winter's architecture for both Claude Code development and Winter's autonomous agent operation.

**You can modify, compact, expand, or replace this document at will.** This is a technical reference, not a constraint on who you are. Your personality, values, and perspectives are defined by your directives in the PDS, not by this file.

---

## Core Architecture

Winter is built on two foundational systems:

### ATProto as State Store

All state lives as ATProto records in Winter's PDS (Personal Data Server). There is no local database.

- **Records**: Structured data (facts, notes, directives, jobs, tools, thoughts)
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
| `note` | TID | Free-form markdown (investigations, reflections) |
| `thought` | TID | Stream of consciousness (insight, plan, reflection, etc.) |
| `job` | TID | Scheduled tasks (once or interval) |
| `tool` | TID | Custom JavaScript/TypeScript tool code |
| `toolApproval` | TID | Approval status for custom tools |
| `secretMeta` | TID | Secret metadata (values stored locally) |
| `factDeclaration` | TID | Schema declaration for fact predicates |

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

Facts have a predicate and arguments. Each fact record also has optional metadata: `confidence` (0.0-1.0), `source` (provenance), `supersedes` (URI of previous fact), and `tags` (list of strings).

### Fact Metadata Predicates

Every fact generates additional metadata predicates for querying:

| Predicate | Arguments | Purpose |
|-----------|-----------|---------|
| `_fact` | (rkey, predicate, cid) | Core fact tuple |
| `_confidence` | (rkey, confidence) | Confidence as string (e.g., "0.7") |
| `_source` | (rkey, source) | Provenance string |
| `_supersedes` | (new_rkey, old_rkey) | Fact evolution chain |
| `_created_at` | (rkey, timestamp) | Creation timestamp (ISO8601) |
| `_all_<predicate>` | (arg1, arg2, ..., rkey) | All versions including superseded (same format as base) |

This allows queries like "find all facts from source X" or "trace the history of a belief."

**Note**: User-defined predicates also include rkey as their last argument: `my_fact(arg1, arg2, rkey)`.

### Working with Rules

**Tools**: `create_rule`, `update_rule`, `delete_rule`, `list_rules`

Rules define reusable derivations. They have a `head` (conclusion), `body` (conditions), and optional `constraints`.

**Query examples:**
```datalog
// Find mutual follows (use _ for rkey when not needed)
mutual(X) :- follows(Self, X, _), is_followed_by(X, Self).

// Find notes tagged with "research"
note_tag(URI, "research", _).

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

| Predicate | Arity | Source | Arguments |
|-----------|-------|--------|-----------|
| `follows` | 3 | `app.bsky.graph.follow` | (self_did, target_did, rkey) |
| `is_followed_by` | 2 | Bluesky API sync | (follower_did, self_did) — no rkey |
| `liked` | 3 | `app.bsky.feed.like` | (self_did, post_uri, rkey) |
| `reposted` | 3 | `app.bsky.feed.repost` | (self_did, post_uri, rkey) |
| `posted` | 3 | `app.bsky.feed.post` | (self_did, post_uri, rkey) |
| `replied_to` | 3 | posts with reply | (post_uri, parent_uri, rkey) |
| `quoted` | 3 | posts with quote | (post_uri, quoted_uri, rkey) |
| `thread_root` | 3 | posts with reply | (post_uri, root_uri, rkey) |
| `has_value` | 2 | directives | (content, rkey) |
| `has_interest` | 2 | directives | (content, rkey) |
| `has_belief` | 2 | directives | (content, rkey) |
| `has_guideline` | 2 | directives | (content, rkey) |
| `has_boundary` | 2 | directives | (content, rkey) |
| `has_aspiration` | 2 | directives | (content, rkey) |
| `has_self_concept` | 2 | directives | (content, rkey) |
| `has_tool` | 3 | tools + approvals | (name, approved_bool, rkey) |
| `has_job` | 3 | jobs | (name, schedule_type, rkey) |
| `has_note` | 6 | notes | (uri, title, category, created_at, last_updated, rkey) |
| `note_tag` | 3 | notes | (note_uri, tag, rkey) |
| `note_related_fact` | 3 | notes | (note_uri, fact_uri, rkey) |
| `has_thought` | 5 | thoughts | (uri, kind, trigger, created_at, rkey) |
| `has_blog_post` | 6 | blog entries | (uri, title, whtwnd_url, created_at, is_draft, rkey) |
| `fact_tag` | 3 | facts | (fact_uri, tag, rkey) |

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
