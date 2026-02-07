//! Detection utilities for Deno custom tool code analysis.

/// Detect if tool code needs network access based on common patterns.
/// Catches remote imports, fetch calls, and other network APIs that
/// would fail without `--allow-net`.
pub fn code_needs_network(code: &str) -> bool {
    // Remote ES module imports — both static and dynamic
    // Static:  import ... from "https://...", import "https://..."
    // Dynamic: await import("https://..."), import("npm:...")
    let remote_prefixes = [
        "\"https://", "'https://",
        "\"http://",  "'http://",
        "\"npm:",     "'npm:",
        "\"jsr:",     "'jsr:",
    ];

    for prefix in &remote_prefixes {
        // Static: from "https://..." or import "https://..."
        if code.contains(&format!("from {}", prefix))
            || code.contains(&format!("import {}", prefix))
        {
            return true;
        }
        // Dynamic: import("https://...") — with optional whitespace
        if code.contains(&format!("import({}", prefix))
            || code.contains(&format!("import ({}", prefix))
        {
            return true;
        }
    }

    // Explicit network APIs
    code.contains("fetch(")
        || code.contains("Deno.connect")
        || code.contains("new WebSocket")
        || code.contains("new EventSource")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_remote_imports() {
        assert!(code_needs_network(
            r#"import { serve } from "https://deno.land/std/http/server.ts";"#
        ));
        assert!(code_needs_network(r#"import chalk from "npm:chalk";"#));
        assert!(code_needs_network(
            r#"const mod = await import("https://example.com/mod.ts");"#
        ));
    }

    #[test]
    fn detects_fetch() {
        assert!(code_needs_network("const res = await fetch(url);"));
    }

    #[test]
    fn allows_local_code() {
        assert!(!code_needs_network(
            r#"const x = "hello"; console.log(x);"#
        ));
        assert!(!code_needs_network(
            r#"import { foo } from "./local.ts";"#
        ));
    }
}
