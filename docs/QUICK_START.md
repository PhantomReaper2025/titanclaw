# TitanClaw Quick Start

Get TitanClaw running in 5 minutes.

## Prerequisites

- Rust 1.92+ (for building)
- Ollama (for local LLM) or API keys (for cloud)
- Docker (optional, for sandboxed jobs)

## 1. Build

```bash
cd /path/to/titanclaw
cargo build --release
```

## 2. Configure

Create `.env` in the project root:

```env
# Database (libSQL embedded - no external DB needed)
DATABASE_BACKEND=libsql
LIBSQL_PATH=~/.ironclaw/ironclaw.db

# LLM Provider - Ollama (recommended for local)
LLM_BACKEND=ollama
OLLAMA_MODEL=llama3.2
OLLAMA_BASE_URL=http://localhost:11434

# Or use OpenAI-compatible API:
# LLM_BACKEND=openai_compatible
# LLM_BASE_URL=https://api.openai.com/v1
# LLM_API_KEY=sk-...

# Agent Settings
AGENT_NAME=titanclaw
AGENT_MAX_PARALLEL_JOBS=5
```

## 3. Run

```bash
# Start Ollama first
ollama serve &

# Run TitanClaw
./target/release/titanclaw run
```

First run will prompt for profile setup (name, preferences, tone). Takes ~2 minutes.

## 4. Access

- **CLI**: Interactive REPL in terminal
- **Web**: Open the gateway URL shown at startup (e.g., http://127.0.0.1:3000/?token=...)

## 5. Basic Usage

```bash
# Send a single message
./target/release/titanclaw -m "What files are in my workspace?"

# Skip onboarding
./target/release/titanclaw --no-onboard run

# Check dependencies
./target/release/titanclaw doctor

# View status
./target/release/titanclaw status
```

## 6. Enable Advanced Features

```env
# Autonomy Control Plane (planner, verifier, replanner)
AUTONOMY_POLICY_ENGINE_V1=true
AUTONOMY_VERIFIER_V1=true
AUTONOMY_REPLANNER_V1=true

# Memory Plane v2 (structured memory)
AUTONOMY_MEMORY_PLANE_V2=true
AUTONOMY_MEMORY_RETRIEVAL_V2=true
AUTONOMY_MEMORY_CONSOLIDATION_V2=true

# Tool Routing v2 (reliability, circuit breakers)
AUTONOMY_TOOL_ROUTING_V2=true
```

## Next Steps

- [CLI Reference](CLI_REFERENCE.md)
- [Web API](WEB_API.md)
- [Autonomy Guide](AUTONOMY.md)
- [Memory Plane Guide](MEMORY_PLANE.md)

## Troubleshooting

### "Ollama connection failed"
```bash
ollama serve
ollama pull llama3.2
```

### "Database error"
The libSQL database is created automatically. If corrupted:
```bash
rm ~/.ironclaw/ironclaw.db
```

### "Port already in use"
Kill existing processes or change ports in config.