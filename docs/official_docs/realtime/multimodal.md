# Multimodal: Video Input

A realtime agent can **see**. Alongside the audio stream, you can push image
frames from a camera (or screen) to the model, so the user can *show* the agent
something — a damaged product for a return, an error on a screen, a document —
instead of describing it.

## The API

One method on the runner, mirroring `send_audio`:

```rust
runner.send_video_frame("image/jpeg", &base64_jpeg).await?;
```

It's also available on the lower-level `RealtimeRunner` and on `RealtimeSession`
directly. The default trait implementation is a **no-op**, so providers/models
that don't accept visual input simply ignore frames.

## How each provider handles frames

The two backends treat vision differently, and that should shape how often you
send frames:

| Provider | Mechanism | Cadence |
|----------|-----------|---------|
| **Gemini Live** | Continuous `realtimeInput` media chunks | Stream them — ~1–2 fps is natural |
| **OpenAI Realtime** | An `input_image` part on a conversation item | Snapshots — throttle to ~1 every few seconds |

Gemini is built for **continuous video**: frames flow like audio and the model
reasons over the moving picture. OpenAI's realtime vision is **image-in-context**:
each frame becomes a conversation item, so sending many per second floods the
context and costs more. Send OpenAI a frame when it's useful (e.g. the user said
"look at this"), not every tick.

The example UIs encode this:

```js
const intervalMs = provider === 'gemini' ? 700 : 2500;  // ~1.4 fps vs ~0.4 fps
```

## Capturing frames in a browser

Grab the camera, draw the `<video>` to a canvas, and ship JPEG frames over your
WebSocket (the [server-side bridge](building-web-apps.md) forwards them to
`send_video_frame`):

```js
const stream = await navigator.mediaDevices.getUserMedia({ video: { width: 640, height: 480 } });
video.srcObject = stream;

const canvas = document.createElement('canvas');
setInterval(() => {
  if (ws.readyState !== WebSocket.OPEN) return;
  canvas.width = 640; canvas.height = 480;
  canvas.getContext('2d').drawImage(video, 0, 0, 640, 480);
  const data = canvas.toDataURL('image/jpeg', 0.6).split(',')[1];  // strip data: prefix
  ws.send(JSON.stringify({ type: 'video_frame', mime: 'image/jpeg', data }));
}, intervalMs);
```

On the server, forward to the model:

```rust
ClientMsg::VideoFrame { mime, data } => {
    let _ = runner.send_video_frame(&mime, &data).await;  // non-fatal on error
}
```

Keep frames modest (640×480, JPEG quality ~0.6) — vision doesn't need full
resolution, and smaller frames mean lower latency and cost.

## Prompting for vision

Tell the agent it can see, so it uses the camera naturally:

```text
You can hear the customer and SEE what they show their camera. When they show
you an item (e.g. a damaged product for a return), briefly describe what you see
and use it to help resolve the issue.
```

## Try it

The [`customer_service`](examples.md#customer_service) example has a camera panel:
start the camera, hold up an object, and say *"I want to return this — can you
see it?"*. It works best on Gemini (continuous video); on OpenAI the agent reasons
over the periodic snapshots.

Next: [Affective dialogue →](affective-dialogue.md)
