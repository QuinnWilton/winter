#!/bin/bash
# Simple coverage measurement script for claude-ai-interactive

echo "=== Claude AI Interactive Test Coverage Report ==="
echo "Generated at: $(date)"
echo

# Run tests and collect basic metrics
echo "Running tests..."
cargo test --lib --quiet 2>&1 | grep -E "(test result:|passed:|failed:)" || true

echo
echo "=== Module Coverage Summary ==="

# Count test functions per module
for module in cost history session cli analytics execution profiling output error; do
    test_count=$(grep -r "^\s*#\[test\]" claude-ai-interactive/src/$module* 2>/dev/null | wc -l | xargs)
    fn_count=$(grep -r "^\s*pub fn\|^\s*fn" claude-ai-interactive/src/$module* 2>/dev/null | grep -v test | wc -l | xargs)
    echo "Module: $module"
    echo "  Test functions: $test_count"
    echo "  Total functions: ~$fn_count"
    echo "  Coverage estimate: $((test_count * 100 / (fn_count + 1)))%"
    echo
done

echo "=== Property-Based Testing Coverage ==="
property_tests=$(grep -r "proptest!" claude-ai-interactive/src --include="*_test.rs" | wc -l | xargs)
echo "Property tests found: $property_tests"

echo
echo "=== Test Infrastructure ==="
test_files=$(find claude-ai-interactive/src -name "*_test.rs" | wc -l | xargs)
test_modules=$(grep -r "mod.*test" claude-ai-interactive/src | wc -l | xargs)
echo "Test files: $test_files"
echo "Test modules: $test_modules"

echo
echo "=== Coverage Gaps Identified ==="
echo "Modules with low test coverage:"
for module in analytics execution profiling; do
    test_count=$(grep -r "^\s*#\[test\]" claude-ai-interactive/src/$module* 2>/dev/null | wc -l | xargs)
    if [ "$test_count" -lt "5" ]; then
        echo "  - $module: only $test_count tests"
    fi
done

echo
echo "=== Recommendations ==="
echo "1. Add more tests for analytics, execution, and profiling modules"
echo "2. Increase property-based testing coverage"
echo "3. Add integration tests between modules"
echo "4. Target 95% line coverage across all modules"

echo
echo "Note: For detailed coverage metrics, use: cargo tarpaulin --lib --out Html"