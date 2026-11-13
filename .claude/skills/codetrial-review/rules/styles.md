# Styles and layout

`web/theme.css`, `web/styles.css` and page markup. The theme rules
[codetrial-web](../../codetrial-web/SKILL.md#what-review-checks-in-a-ui-change)
states are not repeated here.

## Use theme tokens and respect the cascade

Applies to: `web/theme.css`, `web/styles.css`, `web/task.css`, `web/peer.css`,
`src/web/setup.rs`

The editor has light and dark palettes (`.editor-stack[data-theme="light"]` and
the `prefers-color-scheme` twin). Flag hard-coded dark colors (`#d4d4d4`,
`#404040`, the page's `--muted`/`--hairline` used in the gutter), palettes
written out twice with no test comparing them, and `--hairline` used for a
control edge (about 1.3:1, while `--control-border` meets 3:1). In
`web/theme.css` literal values are the palette itself and expected; there,
flag a role defined in one scheme's block and missing from the other, or a
derived token left on `:root` while its input is overridden. Cascade traps:
`display: grid !important` beats the global `[hidden] { display:none !important
}`, an id selector outranks the class-level reduced-motion override, a value
with `safe` that older browsers drop entirely (declare the plain value first),
borders used to draw chevrons (solid blocks in forced-colors mode), and
`display` on `::marker` (ignored).

Seen: 8 PRs. Indent guides at `#404040` that turn into near-black rules on the
light editor.

Not a finding: a redundant `[hidden]` rule. That is cleanup, not a defect.

## Check every breakpoint and the compact Jim stage

Applies to: `web/styles.css`, `web/interview.html`, `web/index.html`

Layouts depend on stacked queries: `.jim-stage.compact` below 1200px and when
the sidebar is dragged wide, `#ending-overlay:has(~ .jim-stage.compact)` with a
fixed 7.5rem clearance, `.toolbar-end { display: contents }` below 34rem, the
`(min-width: 640px) and (max-height: 850px)` one-screen lobby budget. Flag new
rows or elements that grow the compact card past the clearance (the self view,
an extra ending button at 390x480), restore rules that start at a different
width than the rule they override, moved controls that lose their phone-row
ordering, and fixed sizes that clip (the gutter at 0.75rem clips 4-digit line
numbers, `min-height: 0` lets the editor collapse, 16:9 `cover` crops a 4:3
camera that the face check still reads in full).

Seen: 7 PRs. A Withdraw button pushed under the compact card on a phone,
failing the browser gate.

Not a finding: a spacing restore whose query now starts at 640px, with a lobby
test covering 639-1280px.
