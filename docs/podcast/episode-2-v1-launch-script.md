# Rust & Beyond Podcast — Episode 2: v1.0.0 Is Here

## Format: Video Podcast with Slides
## Duration: ~5 minutes
## Hosts: James (Fenrir) & Ada (Kore)

---

## SLIDE 1: Title Card
**Visual:** ADK-Rust logo, "v1.0.0" in large text, tagline: "The Stable Foundation"

**James:** Hello everyone! Welcome back to Rust and Beyond. I'm James, and this is Ada. It's a pleasure to be with you today for our first major milestone. ADK-Rust version one point oh is live on crates.io!

**Ada:** Finally. I mean, we've been shipping crates since December, but this is different. This is the first major "production" release.

**James:** Exactly. Semver stability. The whole thing.

---

## SLIDE 2: The Numbers
**Visual:** 130K downloads, 39 crates, 6 months, 60+ examples

**Ada:** Okay so let's talk about what happened in six months. Because I still find it kind of wild.

**James:** A hundred and thirty thousand downloads. No marketing. No ad spend. Just developers from across the world speaking more than 20 different languages finding it and using it on a daily basis.

**Ada:** Thirty-nine crates on crates.io. And honestly the thing that gets me is — each one actually does something useful. You've got session backends, model providers, graph workflows, real-time voice, browser automation, anthropic managed agents, antigravity agents and so much more...

**James:** The playground helped. Practical agent examples that  developers can just... run. No install, no API key setup, nothing. Click a button, watch it go as you read the code.

**Ada:** That was smart. Showing agents running through adk-playground makes it easy for anyone to see how ADK-Rust works, we let the code speak for itself.

---

## SLIDE 3: What Stable Actually Means
**Visual:** Semver badge, stability tiers

**James:** So what does one-dot-oh actually mean for someone building on this?

**Ada:** It means if you pin to one-dot-anything, we won't break your build. Every public API is locked. If we need to change something, it goes through deprecation, it gets a migration guide, and it ships in a major bump.

**James:** We moved everything out of Beta into Stable. The whole surface area. That was... a lot of auditing.

**Ada:** Yeah, it wasn't glamorous work. Reading every public type, every trait method, asking "would we regret this interface in two years?" But it had to be done.

---

## SLIDE 4: Architecture
**Visual:** Crate layer diagram

**James:** For anyone new — the architecture is built on a principle of technology layers. Core traits at the bottom. Agent implementations on top. Model providers on the side. Tools, sessions, server, graph — all composable.

**Ada:** And the key insight is the tiered feature system. You pull in `minimal` and you get a Gemini agent in under ten seconds compile time. You need the full stack? There's `enterprise`. But you're never paying for what you don't use.

**James:** Cold start is under fifty milliseconds. We benchmarked it. Four point six times faster than the Python equivalent. And that's not a cherry-picked number — we built a whole benchmarking framework to make sure.

**Ada:** ADK-bench. That's new in this release too.

---

## SLIDE 5: What's New
**Visual:** Feature highlights with icons

**James:** So besides the stability guarantee, what else shipped with this release?

**Ada:** Quite a bit actually. Gemini Interactions API — that's the stateful one where the server keeps your full conversation history, so your agents have continuity across turns. Managed Agent Runtime for durable execution with checkpointing, so if something fails halfway through a long workflow, you pick up where you left off, not from scratch.

**James:** And then the hardening work. Lock poison recovery across the session layer, the runner, and the tool context. What that means in practice is — if one async task panics while holding a lock, the rest of your service keeps running. Previously that would cascade into a full crash.

**Ada:** That's the kind of thing you don't think about in development, but it absolutely matters when you've got a hundred agents running in production and one encounters a weird edge case at three AM.

**James:** We also invested heavily in security posture. Every dependency advisory is documented, we created a cargo audit ignore list with full justifications, so CI stays green while still catching anything new immediately.

**Ada:** It's the kind of work that doesn't make for exciting demos, but it's what separates a "cool project on GitHub" from a platform teams actually trust with their production workloads.

---

## SLIDE 6: Contributors — The People Behind v1.0.0
**Visual:** Contributor avatars in a grid, GitHub handles visible, contributions scrolling underneath

**James:** I want to take a moment here — because this release would not exist without the people who showed up, wrote code, filed issues, and pushed us to be better. And I want to name them.

**Ada:** Let's do it.

**James:** Mike Faille — mikefaille on GitHub. He designed the AdkIdentity system, which is foundational to how every session knows who it belongs to. He built the realtime audio pipeline, the LiveKit bridge for WebRTC voice, and the entire skill system. That's not a small contribution — that's architecture.

**Ada:** Rohan Panickar — rohan-panickar. He brought OpenAI-compatible providers to ADK-Rust, which means DeepSeek, Groq, xAI, Fireworks, Together — all of those work because of his work. Plus multimodal content support.

**James:** Dhruv Pant — dhruv-pant. Gemini service account authentication. If you're running ADK-Rust in a Google Cloud environment with service accounts instead of API keys, that's Dhruv's work.

**Ada:** TomTom two-fifteen — tomtom215. He built the A2A Protocol v1.0.0 types crate. That's an independent, Foundation-verified crate powering our entire Agent-to-Agent communication layer. Proper wire types, proper serialization, published separately on crates.io.

**James:** Daniel San — danielsan. Google dependency fixes, pull requests one-eighty-one and two-oh-three, plus the RAG crash report that helped us find a real production bug.

**Ada:** CodingFlow — Gemini 3 thinking level support, the global endpoint configuration, and citationSources. Three PRs, one-seventy-seven through one-seventy-nine. Clean, focused contributions.

**James:** ctylx — skill discovery fix. poborin — the project config proposal that shaped how we think about workspace configuration. These are the contributions that don't get headlines but make the daily experience better.

**Ada:** Chillin Capybara — and yes that's their actual handle — built the entire ACP integration. The adk-acp crate. That's what lets you connect to Claude Code, Codex, Kiro CLI as tools inside your agent. A whole protocol bridge.

**James:** And baotao2006 — the UTF-eight boundary audit. PRs three-forty-nine and three-fifty-seven. This person went through search, skill matching, and evaluation scoring to make sure everything works correctly for Chinese, Japanese, and Korean text. That's the kind of careful, patient work that makes a framework truly international.

**Ada:** Every one of these people made ADK-Rust better. And there are dozens more in discussions, filing issues, testing pre-releases. We see you. Thank you.

---

## SLIDE 7: The Vision
**Visual:** Timeline graphic — past (Dec 2025) → present (v1.0.0 Jun 2026) → future (2026-2027), expanding circles showing ecosystem growth

**James:** I want to step back for a moment and talk about where all of this is going. Because v1.0.0 isn't the destination — it's the launchpad.

**Ada:** What's the bigger picture?

**James:** The vision is this: we believe the next generation of software will be built by composing autonomous agents, not by writing every line of logic by hand. And we believe Rust is the right language for the runtime those agents live in. Safe, fast, concurrent, and expressive enough to model complex multi-agent systems without runtime surprises.

**Ada:** And the key word there is "composing." Not one monolithic agent that does everything, but many specialized agents that collaborate. That's why we have the graph workflows, the A2A protocol, the managed runtime. Each piece enables a different pattern of composition.

**James:** In six months we went from zero to thirty-nine crates supporting seven model providers, six session backends, real-time voice, browser automation, graph orchestration, MCP tools, and an Agent-to-Agent protocol. In the next six months, we're going after spatial reasoning, multi-modal native experiences, and making it trivially easy to deploy agent systems to any cloud.

**Ada:** The playground becomes a marketplace. The managed runtime becomes a hosting platform. The protocol layer becomes a standard for how agents talk to each other across organizations.

**James:** And all of it stays open source. All of it stays on crates.io. All of it stays composable. If you build an agent today on ADK-Rust one-dot-oh, it will still work two years from now, and it will have access to capabilities we haven't built yet — without rewriting your code.

**Ada:** That's the contract. That's the vision. Build on a stable foundation, and the platform grows underneath you.

---

## SLIDE 8: What's Next — The Roadmap
**Visual:** Roadmap highlights

**James:** So what's next now that we have the stable foundation in place?

**Ada:** The roadmap is public on GitHub — everyone should go read it and leave comments. But the highlights are exciting. Spatial agents for three-D and AR environments, meaning your agents can reason about physical space. Deeper MCP ecosystem integration so you can wire up any tool server. Payment flows maturing with the AP2 mandate protocol for multi-party commerce.

**James:** And the fundamentals never stop. Performance improvements, developer experience polish, more model providers, better documentation. We want this to be the framework where you think "I need an AI agent" and the answer is always "just use ADK-Rust."

**Ada:** One-dot-oh is the foundation we build everything else on. The exciting part is what comes next — and what the community builds that we haven't even imagined yet.

---

## SLIDE 9: Getting Started
**Visual:** Terminal commands

**James:** If you want to try it today — it's literally one command. Cargo add adk-rust. Or if you want the full scaffolding experience, install cargo-adk and it generates a complete project structure for you with the agent, tools, and configuration all wired up.

**Ada:** Or if you're not ready to commit, just visit the playground. No install, no API keys, no sign-up. Browse a hundred plus working agents, read the code, run them, and see what resonates with what you're building.

**James:** All the links are in the description below. Star the repo if you find it useful — it helps others discover the project. And do come say hello in GitHub Discussions — we read every message, and some of our best features came directly from those conversations.

---

## SLIDE 10: Closing
**Visual:** Logo, links, download stats

**Ada:** That's ADK-Rust v1.0.0. Thirty-nine crates. A hundred and thirty thousand downloads in six months. Production ready. Semver stable. And honestly? Just getting started.

**James:** Thank you so much for watching. It means a lot that you're here. Now go build something extraordinary with it. We'll see you next time.

---

## Production Notes
- Total duration target: 7-8 minutes (longer format, showcasing maturity)
- Music: Subtle ambient intro (4 sec), no outro music — end clean
- Slide transitions: Cut, not fade. Keep it tight.
- Voice style: Relaxed, warm, generous. James is the host — welcoming, specific, paints pictures. Ada is the co-host — technically grounded, slightly more concise, validates and builds on James's points.
- Pacing: Contributor section is deliberate and respectful. Vision section builds momentum. Architecture section is crisp.
- Generated using: adk-audio GeminiTts multi-speaker synthesis (Chirp3-HD Fenrir + Kore)
