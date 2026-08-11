# Cross-Session Messaging Protocol (the mesh substrate)

The same socket that carries `control`/`rename` is a general inbox — this is the substrate 
for orchestration between agents. Confirmed surface, protocol version `peerProtocol: 1`.

## Transport

- **Type**: unix socket, newline-delimited JSON, one message per line
- **Buffer limit**: 1 MiB without a newline drops the connection
- **Path**: `$XDG_RUNTIME_DIR/cc-socks/<pid>.sock`, falling back to `/tmp/cc-socks-<uid>/<pid>.sock`
- **Length constraint**: whole path must stay under ~104 bytes

## Message types

### User message injection

```json
{
  "type": "user",
  "message": {
    "role": "user",
    "content": "..."
  },
  "priority": "next",
  "from": "...",
  "msg_id": "...",
  "uuid": "...",
  "file_attachments": [...]
}
```

- Injects a prompt into the session's queue
- Optional fields: `priority` (`next` is default, `now` jumps the queue), `from`, `msg_id`, 
  `uuid`, `file_attachments`
- **Runs with `skipSlashCommands: true`** — you cannot drive `/rename`, `/compact` or any 
  other slash command by injecting it as a prompt
- Control actions are the only out-of-band commands

Example from CLI docs:
```bash
echo '{"type":"user",...}' | socat - UNIX-CONNECT:<sock>
```

### Control actions

```json
{
  "type": "control",
  "action": "rename",
  "name": "...",
  "session_id": "..."
}
```

See [04-session-rename.md](04-session-rename.md) for rename details.

Unknown actions are logged and ignored, so probing costs nothing.

### Delivery receipts

```json
{
  "type": "control",
  "action": "peer_message_status",
  "status": "held|denied|expired|delivered",
  "orig_msg_id": "..."
}
```

Peer messages can be **held for the recipient's approval** (permission-mode parity), so 
cross-session sends are not guaranteed delivery.

## Authorization

Senders are identified by peer credentials on the socket (uid check, pid + ancestry read), 
and a message carrying `session_id` is dropped unless it matches the receiving session.
