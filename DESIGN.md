# agy-bridge Design

`agy-bridge` is a standalone, reusable crate providing:

- Agent lifecycle (create, chat, shutdown) with RAII semantics
- Async bridge: dedicated Python thread with asyncio loop, Rust communicates
  via mpsc channels
- Streaming: `async for token` → `mpsc::Receiver<String>`
- Custom tool registration via JSON schema
- Policies: Allow/Deny/WorkspaceOnly per tool
- Triggers: `Every(interval)` / `OnFileChange(path)`
- Hooks: Pre/post turn, pre/post tool call
- Rate limiting with quota backoff
- Python exceptions → typed Rust errors (unified `From<PyErr>` with full
  classification: Antigravity, Pydantic, ImportError)
