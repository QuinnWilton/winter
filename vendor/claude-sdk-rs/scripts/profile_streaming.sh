#!/bin/bash

# Script to profile streaming performance and generate reports
set -e

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

echo -e "${GREEN}Streaming Performance Profiling${NC}"
echo "================================="

# Check if we're in the right directory
if [ ! -f "Cargo.toml" ]; then
    echo -e "${RED}Error: Must be run from the project root directory${NC}"
    exit 1
fi

# Create results directory
RESULTS_DIR="target/profiling-results"
mkdir -p "$RESULTS_DIR"

# Function to run benchmarks with different configurations
run_benchmark_config() {
    local name=$1
    local env_vars=$2
    
    echo -e "\n${YELLOW}Running benchmark: $name${NC}"
    echo "Configuration: $env_vars"
    
    # Run the benchmark
    eval "$env_vars cargo bench --package claude-ai-runtime --bench streaming_bench -- --save-baseline $name"
    
    # Copy results
    if [ -d "target/criterion" ]; then
        cp -r target/criterion "$RESULTS_DIR/criterion-$name"
    fi
}

# 1. Baseline benchmark
echo -e "\n${GREEN}1. Running baseline benchmark${NC}"
run_benchmark_config "baseline" ""

# 2. Benchmark with different channel sizes
echo -e "\n${GREEN}2. Testing different channel buffer sizes${NC}"
# This would require modifying the code to accept env vars, so we'll note it for optimization

# 3. Run streaming benchmark specifically
echo -e "\n${GREEN}3. Detailed streaming benchmark${NC}"
cargo bench --package claude-ai-runtime --bench streaming_bench -- --verbose

# 4. Profile memory usage during streaming
echo -e "\n${GREEN}4. Memory profiling (requires valgrind)${NC}"
if command -v valgrind &> /dev/null; then
    echo "Running memory profiling..."
    cargo build --release --package claude-ai-runtime --bench streaming_bench
    valgrind --tool=massif --massif-out-file="$RESULTS_DIR/massif.out" \
        target/release/deps/streaming_bench-* --bench --profile-time 10 || true
    
    if command -v ms_print &> /dev/null; then
        ms_print "$RESULTS_DIR/massif.out" > "$RESULTS_DIR/memory-profile.txt"
    fi
else
    echo -e "${YELLOW}Valgrind not found. Skipping memory profiling.${NC}"
fi

# 5. Generate flame graphs (requires flamegraph tool)
echo -e "\n${GREEN}5. Generating flame graphs${NC}"
if command -v cargo-flamegraph &> /dev/null; then
    echo "Generating flame graph..."
    cd claude-ai-runtime
    cargo flamegraph --bench streaming_bench -o "../$RESULTS_DIR/flamegraph.svg" -- --bench || true
    cd ..
else
    echo -e "${YELLOW}cargo-flamegraph not found. Install with: cargo install flamegraph${NC}"
fi

# 6. Run client benchmarks for comparison
echo -e "\n${GREEN}6. Running client benchmarks for comparison${NC}"
cargo bench --package claude-ai --bench client_bench

# 7. Generate performance report
echo -e "\n${GREEN}7. Generating performance report${NC}"
REPORT_FILE="$RESULTS_DIR/performance-report.md"

cat > "$REPORT_FILE" << EOF
# Streaming Performance Report

Generated on: $(date)

## Benchmark Results

### Streaming Throughput
Check \`$RESULTS_DIR/criterion-baseline\` for detailed HTML reports.

### Key Metrics
- Message parsing performance
- Streaming throughput for different message sizes
- Buffer size impact on performance
- JSON parsing performance
- Backpressure handling

## Memory Profile
$(if [ -f "$RESULTS_DIR/memory-profile.txt" ]; then
    echo "Memory profiling results available in memory-profile.txt"
else
    echo "Memory profiling not available (install valgrind)"
fi)

## Flame Graph
$(if [ -f "$RESULTS_DIR/flamegraph.svg" ]; then
    echo "CPU flame graph available in flamegraph.svg"
else
    echo "Flame graph not available (install cargo-flamegraph)"
fi)

## Optimization Opportunities

Based on the profiling results, consider:

1. **Buffer Size Optimization**: Current buffer size is hardcoded to 100 in MessageStream::from_line_stream
2. **String Allocation**: Multiple string allocations in parsing could be reduced
3. **JSON Parsing**: Consider using simd-json for faster parsing
4. **Backpressure**: Implement adaptive buffer sizing based on consumer speed

## Next Steps

1. Run \`make bench-compare\` after optimizations to measure improvements
2. Focus on the bottlenecks identified in the flame graph
3. Consider implementing zero-copy parsing where possible
EOF

echo -e "\n${GREEN}Performance profiling complete!${NC}"
echo -e "Results saved to: ${YELLOW}$RESULTS_DIR${NC}"
echo -e "View the report: ${YELLOW}$REPORT_FILE${NC}"
echo -e "View HTML reports: ${YELLOW}$RESULTS_DIR/criterion-baseline/report/index.html${NC}"

# Make the script executable
chmod +x "$0"