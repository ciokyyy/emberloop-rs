Apply a hashline patch sent as `input`. The header carries `[path#TAG]`; ops are `PUT N.=M:`, `PUT <N:`, `PUT >N:`, `PUT >$:` and `CUT N`/`CUT N.=M`; body lines start with `+`. Anchors must come from the view the tag names.

Use for existing files, even multi-file changes. Copy the tag and line numbers from a current view; never invent them. Replace line 4 like this:

```
[path#TAG]
PUT 4.=4:
+replacement line
```

`PUT 4:` is invalid. After a syntax error, correct the patch; after a stale-view error, read the range again. Supported bash reads can also supply tagged views; bash is not inherently incompatible with edit.

One call patches one file: send one `[path#TAG]` header per call. Anchors are line numbers only; a string anchor is rejected. Paths may contain brackets.

Ranges: `PUT N.=M:` replaces every line from N through M with the body, so a shorter body deletes the rest; use `PUT N.=N:` for one line and count body lines against the range. `CUT N.=M` deletes; an empty `PUT` is rejected. Every op in one call numbers against the view the tag names, so order does not matter.

After each patch, read the touched range again before the next patch: the returned tag is the new tag. A patch that lands in the wrong block, leaves an orphan or duplicates a line gets one `PUT N.=M:` over the whole enclosing function. Do not chain small corrective patches on a damaged span.
