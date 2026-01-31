#!/bin/bash

# Multi-Agent Execution Monitor
# Updates execution status based on task completion

PROJECT_DIR="/Users/brandon/Documents/Projects/claude-ai/claude-interactive"
TASKS_DIR="$PROJECT_DIR/tasks"

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Function to count completed tasks
count_tasks() {
    local file=$1
    local total=$(grep -c "^- \[ \]" "$file" 2>/dev/null || echo 0)
    local completed=$(grep -c "^- \[x\]" "$file" 2>/dev/null || echo 0)
    echo "$completed/$total"
}

# Function to calculate percentage
calc_percentage() {
    local completed=$1
    local total=$2
    if [ $total -eq 0 ]; then
        echo 0
    else
        echo $((completed * 100 / total))
    fi
}

# Function to draw progress bar
draw_progress_bar() {
    local percent=$1
    local width=20
    local filled=$((percent * width / 100))
    local empty=$((width - filled))
    
    printf "["
    printf "%${filled}s" | tr ' ' '█'
    printf "%${empty}s" | tr ' ' '░'
    printf "]"
}

# Main monitoring loop
monitor_agents() {
    clear
    echo "═══════════════════════════════════════════════════════════════"
    echo "           MULTI-AGENT EXECUTION MONITOR"
    echo "═══════════════════════════════════════════════════════════════"
    echo ""
    
    # Check each agent
    for i in 1 2 3 4; do
        task_file="$TASKS_DIR/agent-$i-tasks.md"
        if [ -f "$task_file" ]; then
            # Count tasks
            stats=$(count_tasks "$task_file")
            completed=$(echo $stats | cut -d'/' -f1)
            total=$(echo $stats | cut -d'/' -f2)
            percent=$(calc_percentage $completed $total)
            
            # Determine status
            if [ $completed -eq 0 ]; then
                if [ $i -eq 1 ]; then
                    status="${GREEN}🚀 Active${NC}"
                else
                    status="${YELLOW}⏸️  Waiting${NC}"
                fi
            elif [ $completed -eq $total ]; then
                status="${GREEN}✅ Complete${NC}"
            else
                status="${BLUE}🚧 Working${NC}"
            fi
            
            # Get agent name
            case $i in
                1) agent_name="Infrastructure & CLI" ;;
                2) agent_name="Core Systems       " ;;
                3) agent_name="Execution & Runtime" ;;
                4) agent_name="Analytics & Quality" ;;
            esac
            
            # Display agent status
            printf "Agent %d: %-20s " "$i" "$agent_name"
            draw_progress_bar $percent
            printf " %3d%% (%s) %b\n" "$percent" "$stats" "$status"
        fi
    done
    
    echo ""
    echo "───────────────────────────────────────────────────────────────"
    
    # Check dependencies
    echo -e "\n${YELLOW}Dependency Status:${NC}"
    
    # Check if infrastructure tasks are complete
    if grep -q "^\- \[x\] 1.3.2" "$TASKS_DIR/agent-1-tasks.md" 2>/dev/null && \
       grep -q "^\- \[x\] 1.4" "$TASKS_DIR/agent-1-tasks.md" 2>/dev/null; then
        echo -e "${GREEN}✓${NC} Infrastructure foundation ready - all agents unblocked"
    else
        echo -e "${RED}✗${NC} Waiting for Infrastructure Agent (tasks 1.3.2 & 1.4)"
    fi
    
    # Check recent commits
    echo -e "\n${YELLOW}Recent Activity:${NC}"
    cd "$PROJECT_DIR" 2>/dev/null && \
    git log --oneline --since="1 hour ago" --pretty=format:"  %h %s" 2>/dev/null | head -5
    
    # Check for blockers
    echo -e "\n\n${YELLOW}Blockers:${NC}"
    if [ -f "$TASKS_DIR/blockers.md" ]; then
        cat "$TASKS_DIR/blockers.md" | head -5
    else
        echo "  No blockers reported"
    fi
    
    echo -e "\n═══════════════════════════════════════════════════════════════"
    echo "Press Ctrl+C to exit | Refreshing every 30 seconds..."
}

# Create mock progress simulation (for demonstration)
simulate_progress() {
    # This would be replaced with actual task completion in real execution
    echo "Starting execution simulation..."
    
    # Simulate Agent 1 making progress
    # In real execution, agents would update their own task files
}

# Main execution
if [ "$1" == "--simulate" ]; then
    simulate_progress
else
    # Monitor loop
    while true; do
        monitor_agents
        sleep 30
    done
fi