//! Job scheduling tools for MCP.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::protocol::{CallToolResult, ToolDefinition};
use winter_atproto::{Job, JobSchedule, JobStatus, Tid};

use super::{ToolMeta, ToolState};

/// Collection name for jobs.
const JOB_COLLECTION: &str = "diy.razorgirl.winter.job";

pub fn definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "schedule_job".to_string(),
            description: "Schedule a one-time job to run at a specific time.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name for the job"
                    },
                    "instructions": {
                        "type": "string",
                        "description": "Instructions for what to do when the job runs"
                    },
                    "run_at": {
                        "type": "string",
                        "description": "ISO 8601 timestamp for when to run (e.g., '2026-01-30T14:00:00Z')"
                    }
                },
                "required": ["name", "instructions", "run_at"]
            }),
        },
        ToolDefinition {
            name: "schedule_recurring".to_string(),
            description: "Schedule a recurring job that runs at regular intervals.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name for the job"
                    },
                    "instructions": {
                        "type": "string",
                        "description": "Instructions for what to do when the job runs"
                    },
                    "interval_seconds": {
                        "type": "integer",
                        "description": "How often to run (in seconds)"
                    }
                },
                "required": ["name", "instructions", "interval_seconds"]
            }),
        },
        ToolDefinition {
            name: "list_jobs".to_string(),
            description: "List all scheduled jobs.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "status": {
                        "type": "string",
                        "description": "Filter by status (pending, running, completed, failed)"
                    },
                    "name": {
                        "type": "string",
                        "description": "Filter by job name (case-insensitive substring)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of jobs to return"
                    }
                }
            }),
        },
        ToolDefinition {
            name: "cancel_job".to_string(),
            description: "Cancel a scheduled job.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "rkey": {
                        "type": "string",
                        "description": "Record key of the job to cancel"
                    }
                },
                "required": ["rkey"]
            }),
        },
        ToolDefinition {
            name: "get_job".to_string(),
            description: "Get a job by its record key, including full instructions.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "rkey": {
                        "type": "string",
                        "description": "Record key of the job"
                    }
                },
                "required": ["rkey"]
            }),
        },
    ]
}

/// Get all job tools with their permission metadata.
/// All job tools are allowed for the autonomous agent.
pub fn tools() -> Vec<ToolMeta> {
    definitions().into_iter().map(ToolMeta::allowed).collect()
}

pub async fn schedule_job(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let name = match arguments.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => return CallToolResult::error("Missing required parameter: name"),
    };

    let instructions = match arguments.get("instructions").and_then(|v| v.as_str()) {
        Some(i) => i,
        None => return CallToolResult::error("Missing required parameter: instructions"),
    };

    let run_at_str = match arguments.get("run_at").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return CallToolResult::error("Missing required parameter: run_at"),
    };

    let run_at: DateTime<Utc> = match run_at_str.parse() {
        Ok(dt) => dt,
        Err(e) => return CallToolResult::error(format!("Invalid run_at timestamp: {}", e)),
    };

    let job = Job {
        name: name.to_string(),
        instructions: instructions.to_string(),
        schedule: JobSchedule::Once { at: run_at },
        status: JobStatus::Pending,
        last_run: None,
        next_run: Some(run_at),
        failure_count: 0,
        created_at: Utc::now(),
    };

    let rkey = Tid::now().to_string();

    match state
        .atproto
        .create_record(JOB_COLLECTION, Some(&rkey), &job)
        .await
    {
        Ok(response) => {
            // Update cache so subsequent queries see the change immediately
            if let Some(cache) = &state.cache {
                cache.upsert_job(rkey.clone(), job.clone(), response.cid.clone());
            }
            CallToolResult::success(
                json!({
                    "rkey": rkey,
                    "uri": response.uri,
                    "cid": response.cid,
                    "name": name,
                    "run_at": run_at.to_rfc3339()
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to schedule job: {}", e)),
    }
}

pub async fn schedule_recurring(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let name = match arguments.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => return CallToolResult::error("Missing required parameter: name"),
    };

    let instructions = match arguments.get("instructions").and_then(|v| v.as_str()) {
        Some(i) => i,
        None => return CallToolResult::error("Missing required parameter: instructions"),
    };

    let interval = match arguments.get("interval_seconds").and_then(|v| v.as_u64()) {
        Some(0) => return CallToolResult::error("interval_seconds must be greater than 0"),
        Some(i) => i,
        None => return CallToolResult::error("Missing required parameter: interval_seconds"),
    };

    let now = Utc::now();
    let next_run = now + chrono::Duration::seconds(interval as i64);

    let job = Job {
        name: name.to_string(),
        instructions: instructions.to_string(),
        schedule: JobSchedule::Interval { seconds: interval },
        status: JobStatus::Pending,
        last_run: None,
        next_run: Some(next_run),
        failure_count: 0,
        created_at: now,
    };

    let rkey = Tid::now().to_string();

    match state
        .atproto
        .create_record(JOB_COLLECTION, Some(&rkey), &job)
        .await
    {
        Ok(response) => {
            // Update cache so subsequent queries see the change immediately
            if let Some(cache) = &state.cache {
                cache.upsert_job(rkey.clone(), job.clone(), response.cid.clone());
            }
            CallToolResult::success(
                json!({
                    "rkey": rkey,
                    "uri": response.uri,
                    "cid": response.cid,
                    "name": name,
                    "interval_seconds": interval,
                    "next_run": next_run.to_rfc3339()
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to schedule recurring job: {}", e)),
    }
}

pub async fn list_jobs(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let status_filter = arguments.get("status").and_then(|v| v.as_str());
    let name_filter = arguments.get("name").and_then(|v| v.as_str());
    let limit = arguments.get("limit").and_then(|v| v.as_u64());

    // Try cache first, fall back to HTTP
    let jobs = if let Some(ref cache) = state.cache {
        if cache.state() == winter_atproto::SyncState::Live {
            tracing::debug!("using cache for list_jobs");
            cache
                .list_jobs()
                .into_iter()
                .map(|(rkey, cached)| winter_atproto::ListRecordItem {
                    uri: format!("at://did/{}:{}", JOB_COLLECTION, rkey),
                    cid: cached.cid,
                    value: cached.value,
                })
                .collect()
        } else {
            match state.atproto.list_all_records::<Job>(JOB_COLLECTION).await {
                Ok(records) => records,
                Err(e) => return CallToolResult::error(format!("Failed to list jobs: {}", e)),
            }
        }
    } else {
        match state.atproto.list_all_records::<Job>(JOB_COLLECTION).await {
            Ok(records) => records,
            Err(e) => return CallToolResult::error(format!("Failed to list jobs: {}", e)),
        }
    };

    let formatted: Vec<Value> = jobs
        .into_iter()
        .filter(|item| {
            // Filter by status
            if let Some(filter) = status_filter {
                let status_str = match &item.value.status {
                    JobStatus::Pending => "pending",
                    JobStatus::Running => "running",
                    JobStatus::Completed => "completed",
                    JobStatus::Failed { .. } => "failed",
                };
                if status_str != filter {
                    return false;
                }
            }
            // Filter by name (case-insensitive substring)
            if let Some(name) = name_filter
                && !item
                    .value
                    .name
                    .to_lowercase()
                    .contains(&name.to_lowercase())
            {
                return false;
            }
            true
        })
        .take(limit.unwrap_or(usize::MAX as u64) as usize)
        .map(|item| {
            let rkey = item.uri.split('/').next_back().unwrap_or("");
            let schedule_desc = match &item.value.schedule {
                JobSchedule::Once { at } => format!("once at {}", at.to_rfc3339()),
                JobSchedule::Interval { seconds } => format!("every {} seconds", seconds),
            };
            json!({
                "rkey": rkey,
                "name": item.value.name,
                "instructions": item.value.instructions,
                "schedule": schedule_desc,
                "status": format!("{:?}", item.value.status).to_lowercase(),
                "next_run": item.value.next_run.map(|dt| dt.to_rfc3339()),
                "last_run": item.value.last_run.map(|dt| dt.to_rfc3339()),
                "failure_count": item.value.failure_count
            })
        })
        .collect();

    CallToolResult::success(
        json!({
            "count": formatted.len(),
            "jobs": formatted
        })
        .to_string(),
    )
}

pub async fn cancel_job(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let rkey = match arguments.get("rkey").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return CallToolResult::error("Missing required parameter: rkey"),
    };

    match state.atproto.delete_record(JOB_COLLECTION, rkey).await {
        Ok(()) => {
            // Remove from cache
            if let Some(cache) = &state.cache {
                cache.delete_job(rkey);
            }
            CallToolResult::success(
                json!({
                    "cancelled": true,
                    "rkey": rkey
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to cancel job: {}", e)),
    }
}

pub async fn get_job(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let rkey = match arguments.get("rkey").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return CallToolResult::error("Missing required parameter: rkey"),
    };

    match state.atproto.get_record::<Job>(JOB_COLLECTION, rkey).await {
        Ok(record) => {
            let job = record.value;
            let schedule_desc = match &job.schedule {
                JobSchedule::Once { at } => format!("once at {}", at.to_rfc3339()),
                JobSchedule::Interval { seconds } => format!("every {} seconds", seconds),
            };

            CallToolResult::success(
                json!({
                    "rkey": rkey,
                    "name": job.name,
                    "instructions": job.instructions,
                    "schedule": schedule_desc,
                    "status": format!("{:?}", job.status).to_lowercase(),
                    "next_run": job.next_run.map(|dt| dt.to_rfc3339()),
                    "last_run": job.last_run.map(|dt| dt.to_rfc3339()),
                    "failure_count": job.failure_count,
                    "created_at": job.created_at.to_rfc3339()
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to get job: {}", e)),
    }
}
