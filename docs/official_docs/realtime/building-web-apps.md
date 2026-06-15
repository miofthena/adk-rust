# Building Web Apps

This is the practical guide to putting a realtime agent in a browser: the
**server-side bridge** pattern, the WebSocket protocol, and the Web Audio code
that captures the mic and plays the agent gaplessly. Every web example in this
section is built exactly this way; the [`customer_service`](examples.md#customer_service)
server is the reference implementation.

## Why a server-side bridge

A browser **cannot** hold the provider's realtime WebSocket directly:

- your `OPENAI_API_KEY` / `GEMINI_API_KEY` would ship to every client,
- tools would run in the browser, away from your data and credentials,
- you'd be locked to one provider's wire format in client code.

So the browser is a **thin audio/video device** and your Rust server owns the
session:

```text
  browser ──mic PCM16 + camera JPEG (base64 over your WS)──▶  Axum /ws
  browser ◀──agent PCM16 + transcripts + tool events───────   IntegratedRealtimeRunner ──▶ provider
```

The key stays on the server, tools run on the server, and you can switch provider
per connection (`/ws?provider=openai|gemini`) without touching the client.

## The WebSocket protocol

A tiny JSON protocol rides your own WebSocket. Browser → server:

| `type` | Fields | Meaning |
|--------|--------|---------|
| `input_audio` | `audio` (base64 PCM16) | A chunk of mic audio |
| `video_frame` | `mime`, `data` (base64) | A camera frame |
| `text` | `text` | A typed chat message |
| `hangup` | — | End the session |

Server → browser:

| `type` | Fields | Render as |
|--------|--------|-----------|
| `ready` | `provider`, `input_rate`, `output_rate` | Negotiated audio rates — create your `AudioContext`s |
| `audio` | `audio` (base64 PCM16) | Enqueue for gapless playback |
| `agent_transcript` | `delta` | Append to the agent's bubble |
| `user_transcript_delta` | `delta` | Live caption of the user's speech |
| `user_transcript` | `text` | Final user transcript |
| `user_speaking` / `user_stopped` | — | VAD state (drive a mic indicator; flush playback on `user_speaking` for barge-in) |
| `tool` | `name`, `args` | A "running tool…" chip |
| `response_done` | — | Turn finished |
| `error` | `message` | Show the error |

This is a thin, app-defined mapping over `ServerEvent` — see
[`server_event_to_client_json`](#mapping-server-events) below.

## Server: the Axum bridge

The handler builds an `IntegratedRealtimeRunner`, connects, sends `ready`, then
runs two concurrent loops — **outbound** (realtime events → browser) and
**inbound** (browser → session) — joined with `tokio::select!`.

```rust
async fn handle_ws(socket: WebSocket, provider: Provider) {
    let session_id = uuid::Uuid::new_v4().to_string();
    let (mut sender, mut receiver) = socket.split();

    let runner = Arc::new(build_runner(provider, &session_id).await.unwrap());
    runner.connect().await.unwrap();

    // Negotiate audio rates to the browser BEFORE any audio flows.
    let (input_rate, output_rate) = provider.audio_rates();
    sender.send(Message::Text(json!({
        "type": "ready", "provider": provider.name(),
        "input_rate": input_rate, "output_rate": output_rate,
    }).to_string().into())).await.ok();

    // Outbound: realtime events → browser.
    let out_runner = runner.clone();
    let outbound = async move {
        while let Some(event) = out_runner.next_event().await {
            if let Ok(ev) = event {
                if let Some(payload) = server_event_to_client_json(ev) {
                    if sender.send(Message::Text(payload.to_string().into())).await.is_err() { break; }
                }
            }
        }
    };

    // Inbound: browser mic/camera/text → session.
    let in_runner = runner.clone();
    let inbound = async move {
        while let Some(Ok(Message::Text(text))) = receiver.next().await {
            match serde_json::from_str::<ClientMsg>(&text) {
                Ok(ClientMsg::InputAudio { audio }) => { in_runner.send_audio(&audio).await.ok(); }
                Ok(ClientMsg::VideoFrame { mime, data }) => { in_runner.send_video_frame(&mime, &data).await.ok(); }
                Ok(ClientMsg::Text { text }) => {
                    if in_runner.send_text(&text).await.is_ok() { in_runner.create_response().await.ok(); }
                }
                Ok(ClientMsg::Hangup) => break,
                _ => {}
            }
        }
    };

    tokio::select! { _ = outbound => {}, _ = inbound => {} }
    runner.close().await.ok();
}
```

Note the two input asymmetries you must get right:

- **Text needs `create_response()`** — there's no VAD to trigger a reply (see
  [Architecture: turn lifecycle](architecture.md#the-turn-lifecycle)). Audio
  under server VAD does not.
- **Video errors are non-fatal** — log and continue; a dropped frame shouldn't
  kill the call.

### Mapping server events

The outbound loop converts each provider-agnostic `ServerEvent` into the compact
client JSON above, returning `None` for events the UI ignores:

```rust
fn server_event_to_client_json(event: ServerEvent) -> Option<serde_json::Value> {
    match event {
        ServerEvent::AudioDelta { delta, .. } =>
            Some(json!({ "type": "audio", "audio": BASE64.encode(&delta) })),
        ServerEvent::TranscriptDelta { delta, .. } =>
            Some(json!({ "type": "agent_transcript", "delta": delta })),
        ServerEvent::InputTranscriptDelta { delta, .. } =>
            Some(json!({ "type": "user_transcript_delta", "delta": delta })),
        ServerEvent::SpeechStarted { .. } => Some(json!({ "type": "user_speaking" })),
        ServerEvent::FunctionCallDone { name, arguments, .. } =>
            Some(json!({ "type": "tool", "name": name, "args": arguments })),
        ServerEvent::ResponseDone { .. } => Some(json!({ "type": "response_done" })),
        ServerEvent::Error { error, .. } => Some(json!({ "type": "error", "message": error.message })),
        _ => None,   // ServerEvent is #[non_exhaustive]
    }
}
```

## Browser: capturing the mic as PCM16

The provider wants **raw PCM16 mono** at the negotiated `input_rate`. Capture with
an `AudioContext` at that rate, downsample the float samples to 16-bit, base64
them, and send:

```js
let inputRate, outputRate;
ws.onmessage = (e) => {
  const msg = JSON.parse(e.data);
  if (msg.type === 'ready') { inputRate = msg.input_rate; outputRate = msg.output_rate; startMic(); }
  // …handle audio / transcripts / tool / etc.
};

async function startMic() {
  const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
  const ctx = new AudioContext({ sampleRate: inputRate });
  const src = ctx.createMediaStreamSource(stream);
  const node = ctx.createScriptProcessor(4096, 1, 1);  // or an AudioWorklet
  node.onaudioprocess = (ev) => {
    const f32 = ev.inputBuffer.getChannelData(0);
    const pcm16 = new Int16Array(f32.length);
    for (let i = 0; i < f32.length; i++) {
      const s = Math.max(-1, Math.min(1, f32[i]));
      pcm16[i] = s < 0 ? s * 0x8000 : s * 0x7fff;
    }
    ws.send(JSON.stringify({ type: 'input_audio', audio: bytesToBase64(new Uint8Array(pcm16.buffer)) }));
  };
  src.connect(node); node.connect(ctx.destination);
}
```

## Browser: gapless playback + barge-in

The agent's audio arrives as a stream of PCM16 chunks at `output_rate`. Decode
each to float, schedule it back-to-back on a playback `AudioContext`, and keep a
running cursor so chunks don't overlap or gap:

```js
const playCtx = new AudioContext({ sampleRate: outputRate });
let playHead = 0;
const sources = [];

function playChunk(base64) {
  const bytes = base64ToBytes(base64);
  const pcm16 = new Int16Array(bytes.buffer);
  const buf = playCtx.createBuffer(1, pcm16.length, outputRate);
  const ch = buf.getChannelData(0);
  for (let i = 0; i < pcm16.length; i++) ch[i] = pcm16[i] / 0x8000;

  const node = playCtx.createBufferSource();
  node.buffer = buf;
  node.connect(playCtx.destination);
  const startAt = Math.max(playCtx.currentTime, playHead);
  node.start(startAt);
  playHead = startAt + buf.duration;
  sources.push(node);
}

// Barge-in: when the user starts speaking, stop the agent immediately.
function flushPlayback() {
  for (const n of sources) { try { n.stop(); } catch {} }
  sources.length = 0;
  playHead = 0;
}
```

Call `flushPlayback()` on the `user_speaking` message so the agent stops talking
the instant the user interrupts — the natural feel of a real conversation.

## Camera frames

See [Multimodal](multimodal.md#capturing-frames-in-a-browser) for the canvas
capture loop — draw the `<video>` to a canvas, `toDataURL('image/jpeg', 0.6)`,
strip the prefix, and send a `video_frame` message at a provider-appropriate
cadence (~700 ms Gemini, ~2.5 s OpenAI).

## A checklist

- [ ] Key + tools live on the server, never the browser.
- [ ] Send `ready` with `input_rate`/`output_rate` **before** audio; build
      `AudioContext`s at those rates.
- [ ] Capture mic as PCM16 mono at `input_rate`.
- [ ] Play agent audio gaplessly with a scheduling cursor at `output_rate`.
- [ ] Flush playback on `user_speaking` (barge-in).
- [ ] Call `create_response()` after `send_text` (not for VAD audio).
- [ ] Treat `video_frame` send errors as non-fatal.
- [ ] `_ => {}` arm when matching `ServerEvent` (it's `#[non_exhaustive]`).

Next: [Examples →](examples.md)
