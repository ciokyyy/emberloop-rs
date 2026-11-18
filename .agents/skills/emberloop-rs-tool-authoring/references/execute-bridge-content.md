# Non-text tool results through the execute bridge

`McpServer::call` returns several `ContentBlock`s; the execute bridge cannot. A script's tool call travels `tools.py` → `serve_conn` → `Toolbox::call_inner` → `BridgeResponse.result`, a single string. A tool branch that builds an image (or any binary) block only in `mcp.rs` therefore works on a direct call and fails inside `execute`, where the same path is treated as binary text and refused.

## Rule

Build the payload once, beside the text, and append it to the execute call's own MCP result. Never base64-inline binary into the text reply.

## Procedure (the shape `read` images use)
1. Reuse the direct path's helper. `read` images come from `crate::read::image(&toolbox, &args)`, which applies `image_input`, the read jail, `max_read_bytes` and base64 once. Call the same helper; do not re-implement jail or size checks in the bridge.
2. Intercept in `serve_conn` (`crates/tools/src/execute.rs`) before `call_inner`: for the tool name whose payload is binary, store the payload on `BridgeState` (a `Mutex<Vec<Image>>`, `Image = (&'static str, String)` for MIME plus base64) and answer a short marker string such as `[image <mime>]` so the script sees something printable. Return the helper's error as the tool error; fall through to `call_inner` when the helper returns `None`.
3. Return parts from the runner: `run_parts(...) -> ToolResult<(String, Vec<Image>)>`. `run_in_bridge` drains `bridge.state.take_images()` after `flush`/`write_children`, so a timed-out script still delivers what it read. A background job outlives the call with no result to drain into: return no parts, with a `shortcut:` comment naming that limit.
4. Expose parts at the toolbox: `Toolbox::call_parts(...)` returns `(String, Vec<Image>)`, and `call` wraps it with `.map(|(text, _)| text)`. Route only the producing arm (`execute`) to `run_parts`; leave every other arm delegating to `call_inner`, which already wraps retry errors.
5. Emit the blocks in `McpServer::call`: `vec![ContentBlock::text(text)]` then `ContentBlock::image(data, mime)` per part, and keep `record_captured_output(name, args, &text)` for the text.
6. Test at the MCP surface, not the toolbox surface: `McpServer::call("execute", ...)` on a script that reads a fixture `.png`, then assert `content` contains `{"type":"image","mimeType":"image/png","data":...}` and that `data` starts with the base64 of the fixture bytes. A toolbox-level assertion on text passes with no image at all.
7. Cover both entry points for parity: a direct tool call and the same call from a script. The direct special case runs before `toolbox.call`, so only the pair proves them equal.

## Pitfalls
- Base64 of a tiny fixture: `\x89PNG\r\n\x1a\n` begins `iVBORw0KGg`; assert a short prefix, not a hand-computed full string.
- Keep the payload out of the marker text: the model already receives the image block, and a duplicated base64 string costs context twice.
- Do not widen `BridgeResponse` to carry blocks. The bridge is also the state channel for scripts (`store`/`save`/`children`); collecting images beside it keeps `call_inner`'s `String` contract and the existing request shapes unchanged.
