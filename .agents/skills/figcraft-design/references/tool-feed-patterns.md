# Tool-feed patterns from reference products

Use when redesigning how an agent UI renders tool calls, grouped activity and thinking. Cite the pattern in one line when presenting a board.

- Counts alone fail. Collapsed lines such as "Read 1 file" drew heavy user complaints; show the target (file, pattern) inline, a single file by name, several files "space permitting".
- Group consecutive calls between two assistant messages into one dim summary line ("Ran 2 commands", "Explored files", "Edited 1 file +83 −40 ›") with two-stage drill-down: group, then calls, then detail. Offer a density setting (Detailed / Compact / Minimal); users repeatedly ask for expand-by-default, and hiding detail against their setting causes backlash.
- A collapsed row has constant height: cut long commands (heredocs) to one line. Collapsed cards that still print the whole input dominate the transcript.
- Thinking: auto-open while streaming then auto-collapse causes layout shift. Cap the height with auto-scroll while streaming, merge consecutive reasoning parts into one row, pair content with elapsed time ("Thought for 12s"). A duration visible only on hover counts as none.
- Run state belongs outside the thread too: a session-list row with the live step ("Reading x · now") is the strongest observed pattern.
- Name a stopped run "Stopped", keep the partial trace, never relabel an abort as completed.
- Per-model data (from stored session events, not from memory): Claude emits empty thinking spans most of the time (draw a pulse, drop the row if it ends empty); pi/deepseek thinking is plain prose with essentially no `###` headers (use the first sentence as title); agy emits no thinking but carries intent in ACP `call.title` / `toolAction` (use as row title, fall back to verb + target).

## This user's choices for the feed

- Every row starts closed, including the last one; only failed rows start open.
- Quiet colour: grey everything, colour only for running, failed and diff.
- No glyph per row: the chosen treatment is the dots board (5 px `--fg-4` dot, blue running, red failed, `›` chevron, targets on the group line). Keep Lucide for every other icon in the app, composer included; an icon-family specimen sheet does not mean a swap.
- Chat feed spans the full pane width; only the composer keeps its 760 px column.
- Fetch reference-product pages with `read` on the bare URL (HTML comes back as text); append `:raw` only when the page is not GitHub, because raw GitHub HTML is mostly script tags. Cut the nav chrome by slicing after a known marker such as `Insights More items`.
