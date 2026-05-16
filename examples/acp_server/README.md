# ACP Server Example

Demonstrates exposing an ADK-Rust agent as an **ACP-compatible server** that IDEs can connect to via the [Agent Client Protocol](https://github.com/anthropics/agent-client-protocol).

## What This Shows

1. A **coding assistant agent** (Gemini) with `read_file` and `list_directory` tools
2. The **ACP Server** running on stdio transport (newline-delimited JSON)
3. How IDEs spawn this process and communicate via stdin/stdout
4. The full protocol flow: initialize → create session → prompt → close

## Architecture

```text
┌─────────────────┐         ┌──────────────────────────────────────────┐
│   IDE / Client  │  stdin   │            ACP Server                    │
│                 │ ───────► │                                          │
│  (Kiro, VSCode, │         │  StdioTransport → AcpSessionHandler      │
│   Claude Code)  │  stdout  │       │                                  │
│                 │ ◄─────── │       ▼                                  │
└─────────────────┘         │  Runner → LlmAgent (Gemini)              │
                            │              │                            │
                            │              ├── read_file tool           │
                            │              └── list_directory tool      │
                            └──────────────────────────────────────────┘
```

## Setup

```bash
cd examples/acp_server
cp .env.example .env
# Edit .env and add your GOOGLE_API_KEY
```

## Run

```bash
cargo run
```

The server starts and listens on stdin for ACP protocol messages. Instructions are printed to stderr.

## Testing with Manual JSON Messages

You can test the server by piping JSON messages to stdin. Each message is a single JSON object on one line.

### Step 1: Initialize the connection

```bash
echo '{"method": "initialize", "params": {"protocol_version": "1.0"}}' | cargo run
```

Expected response (on stdout):
```json
{"result":{"protocol_version":"1.0","capabilities":{"agent_name":"coding-assistant","agent_description":"ADK-Rust coding assistant with file reading and directory listing tools","streaming":true,"tool_use":true,"tool_names":["read_file","list_directory"]}}}
```

### Step 2: Full conversation flow

Create a file with multiple messages (one per line):

```bash
cat << 'EOF' > /tmp/acp_test.jsonl
{"method": "initialize", "params": {"protocol_version": "1.0"}}
{"method": "session/create", "params": {}}
EOF
```

Then pipe it:
```bash
cat /tmp/acp_test.jsonl | cargo run
```

### Step 3: Interactive session (with a prompt)

For a full interactive test including a prompt, you need all messages in sequence:

```bash
cat << 'EOF' > /tmp/acp_full.jsonl
{"method": "initialize", "params": {"protocol_version": "1.0"}}
{"method": "session/create", "params": {}}
EOF

# Run and capture the session_id from the response, then:
# {"method": "session/prompt", "params": {"session_id": "<id>", "text": "List the files in the current directory"}}
# {"method": "session/close", "params": {"session_id": "<id>"}}
```

### Using a script for full flow

```bash
#!/bin/bash
# Start the server in background, connected via pipes
mkfifo /tmp/acp_in /tmp/acp_out 2>/dev/null

cargo run < /tmp/acp_in > /tmp/acp_out &
SERVER_PID=$!

# Send initialize
echo '{"method": "initialize", "params": {"protocol_version": "1.0"}}' > /tmp/acp_in

# Read response
head -1 /tmp/acp_out

# Send session/create
echo '{"method": "session/create", "params": {}}' > /tmp/acp_in

# Read response (contains session_id)
head -1 /tmp/acp_out

# Clean up
kill $SERVER_PID
rm /tmp/acp_in /tmp/acp_out
```

## Protocol Reference

### Messages (Client → Server)

| Method | Params | Description |
|--------|--------|-------------|
| `initialize` | `{"protocol_version": "1.0"}` | Handshake, returns capabilities |
| `session/create` | `{}` | Create a new session |
| `session/prompt` | `{"session_id": "...", "text": "..."}` | Send a prompt to the agent |
| `session/close` | `{"session_id": "..."}` | Close and clean up a session |

### Responses (Server → Client)

All responses are JSON objects with either `result` or `error`:

```json
{"result": { ... }}
{"error": {"code": "...", "message": "..."}}
```

### Notifications in Prompt Response

The `session/prompt` response includes a `notifications` array with streaming events:

```json
{
  "result": {
    "notifications": [
      {"type": "agent_thought_chunk", "text": "Let me look at the files..."},
      {"type": "tool_call", "name": "list_directory", "args": {"path": "."}},
      {"type": "agent_message_chunk", "text": "Here are the files in your directory..."},
      {"type": "complete"}
    ]
  }
}
```

Notification types:
- `agent_message_chunk` — Text from the agent's response
- `agent_thought_chunk` — Reasoning/thinking from the agent
- `tool_call` — A tool invocation by the agent
- `complete` — Response is finished
- `error` — An error occurred during execution

## IDE Integration

To use this server from an IDE, configure it as an ACP agent:

```json
{
  "agents": [
    {
      "name": "coding-assistant",
      "command": "cargo",
      "args": ["run", "--manifest-path", "/path/to/examples/acp_server/Cargo.toml"],
      "env": {
        "GOOGLE_API_KEY": "your-key-here"
      }
    }
  ]
}
```

The IDE spawns the process, sends `initialize`, creates a session, and then sends prompts as the user interacts.

## Configuration

| Environment Variable | Required | Description |
|---------------------|----------|-------------|
| `GOOGLE_API_KEY` | Yes | Gemini API key |
| `RUST_LOG` | No | Tracing filter (default: `info,adk_acp=debug`) |

## How It Works

1. **Agent Creation** — An `LlmAgent` is built with Gemini and two tools
2. **Server Config** — `AcpServerConfigBuilder` wires the agent, session service, and transport
3. **Server Start** — `AcpServer::run()` spawns a background task reading stdin
4. **Message Routing** — `StdioTransport` parses JSON, routes to `AcpSessionHandler`
5. **Prompt Execution** — Handler creates a `Runner`, streams events, maps to notifications
6. **Response** — Notifications are serialized as JSON and written to stdout

## Extending This Example

- **Add more tools**: Implement the `Tool` trait and add to the agent builder
- **Switch to HTTP**: Use `TransportConfig::Http { bind_address, port }` for network access
- **Add persistence**: Replace `InMemorySessionService` with `DatabaseSessionService`
- **Enable auth**: Add authentication middleware for production deployments
